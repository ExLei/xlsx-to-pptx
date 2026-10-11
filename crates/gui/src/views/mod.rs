//! 视图层：**只读** `AppState` 渲染，把用户动作翻译成 [`Event`]（`docs/design.md` §8.5）。
//!
//! - 根视图（本文件）：标题行 / 内容区 / 状态栏；
//! - [`file_picker`]：输入文件、格式、工作表与 CSV 编码；
//! - [`output_config`]：输出路径、PPTX 模板、渲染器与模板提示、生成/导出/取消；
//! - [`data_preview`]：虚拟滚动词条表与空态。
//!
//! 受控控件的值来自状态机的**投影**（[`VocabApp::sync_views`]）：状态变了才写回，
//! 用户正在编辑的内容不被覆盖。视图不持有业务状态，也不直接调用用例层。

mod data_preview;
mod file_picker;
mod output_config;

use std::path::PathBuf;
use std::time::SystemTime;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::table::TableState;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use x2p_core::domain::schema::{CSV_ENCODINGS, SheetInfo};
use x2p_core::paths::Layout;
use x2p_core::ports::Progress;

use crate::exec::{is_csv, window_title, DiagView, Exec};
use crate::state::{AppState, ArtifactKind, ArtifactView, Event, Job, OutputField, Screen};
use data_preview::PreviewDelegate;

/// 根元素的按键上下文（Ctrl/Cmd+Enter → 生成）。
const APP_CONTEXT: &str = "X2pApp";

actions!(x2p_gui, [GeneratePpt]);

/// 根视图：状态机 + 执行层 + 受控控件。
///
/// 这是唯一持有 [`AppState`] 的地方；所有转移都经 [`VocabApp::dispatch`]。
pub struct VocabApp {
    /// 状态机（唯一真值）。
    pub(crate) state: AppState,
    /// 执行层：会话日志、取消与进度（见 [`crate::exec`]）。
    pub(crate) exec: Exec,

    // ── 受控控件：内容由 state 投影，动作翻译成 Event ──
    pub(crate) input_file: Entity<InputState>,
    pub(crate) output_file: Entity<InputState>,
    pub(crate) template_file: Entity<InputState>,
    pub(crate) sheet_select: Entity<SelectState<Vec<SharedString>>>,
    pub(crate) encoding_select: Entity<SelectState<Vec<SharedString>>>,
    pub(crate) preview: Entity<TableState<PreviewDelegate>>,

    // ── 视图侧簿记（不进状态机）──
    pub(crate) projection: Projection,
    pub(crate) watch: Watch,
    /// 上一次进过状态机的进度（只在变化时发 `JobProgress`）。
    pub(crate) shown_progress: Option<Progress>,
    /// 在飞作业的任务句柄（仅为保活；取消走观察者旗标）。
    pub(crate) job: Option<Task<()>>,
    /// 视图节拍（进度采样 / watcher / 摘要），构造时挂上。
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// state → 受控控件 的投影基线：值没变就不写回（否则会打断用户输入）。
#[derive(Default)]
pub(crate) struct Projection {
    source: Option<PathBuf>,
    sheets: Vec<SheetInfo>,
    sheet: Option<String>,
    encoding: String,
    output_text: String,
    template: Option<PathBuf>,
}

/// 文件 watcher 的基线：只报当前源的 mtime 变化。
#[derive(Default)]
pub(crate) struct Watch {
    pub(crate) source: Option<PathBuf>,
    pub(crate) mtime: Option<SystemTime>,
}

impl VocabApp {
    /// 生产入口：数据目录按 [`Layout::resolve`] 解析（程序目录优先，不可写回退用户目录）。
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_layout(window, cx, Layout::resolve())
    }

    /// 显式数据目录（测试与嵌入场景）。
    pub fn with_layout(window: &mut Window, cx: &mut Context<Self>, layout: Layout) -> Self {
        let input_file = cx
            .new(|cx| InputState::new(window, cx).placeholder("选择 Excel 或 CSV 文件"));
        let output_file = cx.new(|cx| {
            InputState::new(window, cx).placeholder("输出 .pptx 路径（留空按输入文件派生）")
        });
        let template_file = cx.new(|cx| {
            InputState::new(window, cx).placeholder("（可选）含 {{占位符}} 的自定义模板")
        });
        let sheet_select =
            cx.new(|cx| SelectState::new(Vec::<SharedString>::new(), None, window, cx));
        let encoding_select = cx.new(|cx| {
            SelectState::new(
                CSV_ENCODINGS
                    .iter()
                    .map(|encoding| SharedString::from(*encoding))
                    .collect::<Vec<_>>(),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let preview =
            cx.new(|cx| TableState::new(PreviewDelegate::new(window.rem_size()), window, cx));

        // Ctrl/Cmd+Enter → 生成（焦点在输入框时由 Input 的订阅处理；两者同一守卫）。
        cx.bind_keys([KeyBinding::new(
            "secondary-enter",
            GeneratePpt,
            Some(APP_CONTEXT),
        )]);

        let mut subscriptions = Vec::new();

        // 输入框回车 → 打开该路径；secondary 回车 → 生成（与按钮同一守卫）。
        subscriptions.push(cx.subscribe_in(
            &input_file,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { secondary: true, .. } => {
                    this.dispatch(Event::Generate, window, cx)
                }
                InputEvent::PressEnter { .. } => {
                    let path = state.read(cx).value().trim().to_string();
                    if !path.is_empty() {
                        this.dispatch(Event::PickInput(PathBuf::from(path)), window, cx);
                    }
                }
                _ => {}
            },
        ));

        // 输出框改动 → 输出字段（空串即非法，由状态机给出理由）。
        subscriptions.push(cx.subscribe_in(
            &output_file,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { secondary: true, .. } => {
                    this.dispatch(Event::Generate, window, cx)
                }
                InputEvent::Change => {
                    let path = state.read(cx).value().to_string();
                    this.dispatch(Event::OutputChanged(PathBuf::from(path)), window, cx);
                }
                _ => {}
            },
        ));

        // 模板框改动 → 探针校验（`Checking` 可见）；清空即回到内置版式。
        subscriptions.push(cx.subscribe_in(
            &template_file,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { secondary: true, .. } => {
                    this.dispatch(Event::Generate, window, cx)
                }
                InputEvent::Change => {
                    let value = state.read(cx).value().trim().to_string();
                    let next = (!value.is_empty()).then(|| PathBuf::from(value));
                    // 同一个值重复上报只会白白重启探针。
                    if next != this.state.document.template_path {
                        this.dispatch(Event::TemplateChanged(next), window, cx);
                    }
                }
                _ => {}
            },
        ));

        // 工作表 / 编码切换 → 按新选择重读（Busy 期由状态机拒绝并提示）。
        subscriptions.push(cx.subscribe_in(
            &sheet_select,
            window,
            |this, _state, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(name)) = event {
                    let name = name.to_string();
                    if this.state.document.sheet.as_deref() != Some(name.as_str()) {
                        this.dispatch(Event::SelectSheet(name), window, cx);
                    }
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &encoding_select,
            window,
            |this, _state, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(encoding)) = event {
                    let encoding = encoding.to_string();
                    if this.state.document.encoding != encoding {
                        this.dispatch(Event::SelectEncoding(encoding), window, cx);
                    }
                }
            },
        ));

        let ticker = Self::spawn_ticker(window, cx);
        let mut app = VocabApp {
            state: AppState::new(),
            exec: Exec::new(layout),
            input_file,
            output_file,
            template_file,
            sheet_select,
            encoding_select,
            preview,
            projection: Projection::default(),
            watch: Watch::default(),
            shown_progress: None,
            job: None,
            _ticker: ticker,
            _subscriptions: subscriptions,
        };
        // `AppState::new` 只把渲染器置为 Probing：真正的探测由执行层发起（字段级，不占作业槽）。
        app.start_renderer_probe(window, cx);
        app.sync_views(window, cx);
        app
    }

    /// 状态机只读投影（视图与测试共用）。
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// 是否有作业在飞（Busy）。
    pub fn is_busy(&self) -> bool {
        matches!(self.state.screen, Screen::Busy { .. })
    }

    /// 本会话的日志文件（`None` = 建不起来）。
    pub fn log_path(&self) -> Option<&std::path::Path> {
        self.exec.log_path()
    }

    /// 诊断摘要（状态栏数据源）。
    pub fn diag_summary(&self) -> &DiagView {
        self.exec.summary()
    }

    // ── state → 控件 的投影 ──

    /// 把状态机里该显示的东西写回受控控件。
    ///
    /// 每一处都先比对基线（或控件当前值），值没变不写——写回会重置光标与编辑历史。
    pub(crate) fn sync_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_input(window, cx);
        self.sync_sheets(window, cx);
        self.sync_encoding(window, cx);
        self.sync_output(window, cx);
        self.sync_template(window, cx);
        self.sync_preview(cx);
    }

    /// 输入框：源变了才写回，顺带同步窗口标题。
    fn sync_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.state.document.source.clone();
        if self.projection.source == source {
            return;
        }
        self.projection.source = source;
        let text = self
            .projection
            .source
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.write_input(&self.input_file, text, window, cx);
        let title = window_title(self.projection.source.as_deref());
        window.set_window_title(&title);
    }

    /// 工作表下拉：清单与选中项都来自状态机（仅 Excel 有内容）。
    fn sync_sheets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sheets = self.state.document.sheets.clone();
        let selected = self.state.document.sheet.clone();
        if self.projection.sheets != sheets {
            self.projection.sheets = sheets;
            let items: Vec<SharedString> = self
                .projection
                .sheets
                .iter()
                .map(|sheet| SharedString::from(sheet.name.clone()))
                .collect();
            self.sheet_select
                .update(cx, |select, cx| select.set_items(items, window, cx));
        }
        if self.projection.sheet == selected {
            return;
        }
        self.projection.sheet = selected.clone();
        let index = selected
            .as_deref()
            .and_then(|name| {
                self.projection
                    .sheets
                    .iter()
                    .position(|sheet| sheet.name == name)
            })
            .map(IndexPath::new);
        self.sheet_select
            .update(cx, |select, cx| select.set_selected_index(index, window, cx));
    }

    /// CSV 编码下拉：候选集是 `CSV_ENCODINGS`（与平台层校验共用一份）。
    fn sync_encoding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let encoding = self.state.document.encoding.clone();
        if self.projection.encoding == encoding {
            return;
        }
        self.projection.encoding = encoding.clone();
        let index = CSV_ENCODINGS
            .iter()
            .position(|candidate| *candidate == encoding)
            .map(IndexPath::new);
        self.encoding_select
            .update(cx, |select, cx| select.set_selected_index(index, window, cx));
    }

    /// 输出框：`Auto` / `Taken` 写回投影文本；`Invalid` 保持用户正在编辑的内容。
    fn sync_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = output_text(&self.state) else {
            return;
        };
        if self.projection.output_text == text {
            return;
        }
        self.projection.output_text = text.clone();
        self.write_input(&self.output_file, text, window, cx);
    }

    /// 模板框：路径来自状态机（选中文件、清空、换输入文件都经状态机复位）。
    fn sync_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let template = self.state.document.template_path.clone();
        if self.projection.template == template {
            return;
        }
        self.projection.template = template;
        let text = self
            .projection
            .template
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.write_input(&self.template_file, text, window, cx);
    }

    /// 预览表：行数据是 `document.entries` 的投影（`TableDelegate` 拿不到 `AppState`）。
    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        let loading = matches!(self.state.screen, Screen::Busy { job: Job::Load, .. });
        let stale = {
            let table = self.preview.read(cx);
            let delegate = table.delegate();
            delegate.entries() != self.state.document.entries.as_slice()
                || delegate.is_loading() != loading
        };
        if !stale {
            return;
        }
        let entries = self.state.document.entries.clone();
        self.preview.update(cx, |table, cx| {
            table.delegate_mut().set_entries(entries);
            table.delegate_mut().set_loading(loading);
            table.refresh(cx);
        });
    }

    /// 写回一个输入框：内容已相同就不写（`set_value` 会重置光标与编辑历史）。
    fn write_input(
        &self,
        input: &Entity<InputState>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if input.read(cx).value().as_ref() == text {
            return;
        }
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    }
}

impl Render for VocabApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = &*self;
        v_flex()
            .size_full()
            .key_context(APP_CONTEXT)
            .on_action(cx.listener(|this, _: &GeneratePpt, window, cx| {
                this.dispatch(Event::Generate, window, cx)
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.handle_drop(paths, window, cx)
            }))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(header(app, cx))
            .child(content(app, window, cx))
            .child(status_bar(app, cx))
    }
}

/// 内容区：输入 → 输出 → 预览（预览吃掉剩余高度）。
fn content(
    app: &VocabApp,
    window: &mut Window,
    cx: &mut Context<VocabApp>,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_h_0()
        .child(file_picker::render(app, window, cx))
        .child(output_config::render(app, window, cx))
        .child(data_preview::render(app, cx))
}

/// 标题行：应用名 + 当前输入文件名（窗口标题也带同一个名字）。
fn header(app: &VocabApp, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let file = app
        .state
        .document
        .source
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    h_flex()
        .justify_between()
        .gap_3()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(crate::APP_TITLE),
        )
        .when_some(file, |this, file| {
            this.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(file),
            )
        })
}

/// 状态栏：左侧状态、进度与结果动作；右侧日志路径、诊断摘要、数据目录与主题开关。
fn status_bar(app: &VocabApp, cx: &mut Context<VocabApp>) -> impl IntoElement {
    // 颜色与明暗先取出来（`Hsla` 是 Copy）：下面还要把 `cx` 可变地借给面板。
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    let dark = cx.theme().is_dark();
    let (text, color) = status_text(app, cx);
    let busy = app.is_busy();
    let ratio = progress_ratio(&app.state);
    let artifact = match &app.state.screen {
        Screen::Done { artifact } => Some(artifact.clone()),
        _ => None,
    };
    let failed = matches!(app.state.screen, Screen::Failed { .. });
    let retryable = matches!(
        &app.state.screen,
        Screen::Failed { retry: Some(_), .. }
    );

    h_flex()
        .justify_between()
        .gap_4()
        .px_4()
        .py_3()
        .border_t_1()
        .border_color(border)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(color)
                        .child(text),
                )
                .when_some(ratio, |this, percent| {
                    this.child(ProgressBar::new("job-progress").value(percent).small().w_40())
                })
                .when(busy && ratio.is_none(), |this| {
                    // 外呼渲染/读取没有确定的页数：给不确定的旋转指示，别报假百分比。
                    this.child(Spinner::new().small().color(muted))
                })
                .when_some(artifact, |this, artifact| {
                    let is_dir = artifact.kind == ArtifactKind::Png;
                    this.child(
                        Button::new("open-artifact")
                            .ghost()
                            .small()
                            .icon(if is_dir {
                                IconName::FolderOpen
                            } else {
                                IconName::ExternalLink
                            })
                            .label(if is_dir { "打开 PNG 目录" } else { "打开 PPT" })
                            .tooltip(artifact.path.display().to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch(Event::OpenArtifact, window, cx)
                            })),
                    )
                })
                .when(retryable, |this| {
                    this.child(
                        Button::new("retry-job")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label("重试")
                            .tooltip("用同一份参数重放这次作业")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch(Event::Retry, window, cx)
                            })),
                    )
                })
                .when(failed, |this| {
                    this.child(
                        Button::new("dismiss-error")
                            .ghost()
                            .small()
                            .label("关闭")
                            .tooltip("收起错误，回到词条预览")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch(Event::DismissError, window, cx)
                            })),
                    )
                }),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(log_entry(app, cx))
                .child(diag_entry(app, cx))
                .child(
                    Button::new("open-data-dir")
                        .ghost()
                        .icon(IconName::FolderOpen)
                        .accessibility_label("打开数据目录")
                        .tooltip("打开数据目录（日志、模板与默认输出）")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_data_dir(window, cx)
                        })),
                )
                .child(
                    Button::new("theme-toggle")
                        .ghost()
                        .icon(if dark { IconName::Sun } else { IconName::Moon })
                        .accessibility_label("切换明暗模式")
                        .tooltip(if dark {
                            "切换到浅色模式"
                        } else {
                            "切换到深色模式"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_theme(window, cx)
                        })),
                ),
        )
}

/// 日志入口：状态栏显示会话日志路径（截断），点击打开。
fn log_entry(app: &VocabApp, cx: &mut Context<VocabApp>) -> AnyElement {
    let theme = cx.theme();
    match app.exec.log_path() {
        Some(log) => h_flex()
            .gap_2()
            .items_center()
            .min_w_0()
            .child(
                div()
                    .w_40()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(log.display().to_string()),
            )
            .child(
                Button::new("open-log")
                    .ghost()
                    .small()
                    .icon(IconName::FileText)
                    .accessibility_label("打开日志")
                    .tooltip(format!(
                        "打开本会话日志（保留最新 {} 份）",
                        x2p_core::diag::MAX_LOGS
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.open_log(window, cx))),
            )
            .into_any_element(),
        None => div()
            .text_xs()
            .text_color(theme.danger)
            .child(app.exec.log_note().unwrap_or("日志不可用").to_string())
            .into_any_element(),
    }
}

/// 诊断摘要入口：计数来自查询引擎（与 CLI `diag --summary` 同一份实现）。
fn diag_entry(app: &VocabApp, cx: &mut Context<VocabApp>) -> AnyElement {
    let theme = cx.theme();
    match app.exec.summary() {
        DiagView::Summary {
            info,
            warn,
            error,
            bad_lines,
        } => {
            let (icon, summary) = if *error > 0 {
                (
                    IconName::CircleX,
                    format!("诊断：警告 {warn} · 错误 {error}"),
                )
            } else if *warn > 0 {
                (
                    IconName::TriangleAlert,
                    format!("诊断：警告 {warn} · 错误 {error}"),
                )
            } else {
                (IconName::Info, "诊断：无问题".to_string())
            };
            let mut tooltip = format!(
                "事件 {} · 警告 {warn} · 错误 {error}",
                info + warn + error
            );
            if *bad_lines > 0 {
                tooltip.push_str(&format!(" · 坏行 {bad_lines}"));
            }
            tooltip.push_str(&format!(
                "\n日志：{}",
                app.exec
                    .log_path()
                    .map(|log| log.display().to_string())
                    .unwrap_or_else(|| "不可用".to_string())
            ));
            Button::new("diag-summary")
                .ghost()
                .small()
                .icon(icon)
                .label(summary)
                .tooltip(tooltip)
                .on_click(cx.listener(|this, _, window, cx| this.open_log(window, cx)))
                .into_any_element()
        }
        // 摘要没算出来（首帧 / 日志不可读）：原因直接显示，不静默。
        DiagView::Pending => div()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("诊断：…")
            .into_any_element(),
        DiagView::Unavailable(reason) => div()
            .text_xs()
            .text_color(theme.danger)
            .child(reason.clone())
            .into_any_element(),
    }
}

/// 状态文本与颜色（`Done` 成功色、`Failed` 危险色，其余静音）。
fn status_text(app: &VocabApp, cx: &App) -> (String, Hsla) {
    let theme = cx.theme();
    match &app.state.screen {
        Screen::Empty => (
            "请选择 Excel 或 CSV 文件".to_string(),
            theme.muted_foreground,
        ),
        Screen::Loaded => (
            format!("就绪 · {} 条词条", app.state.document.entries.len()),
            theme.muted_foreground,
        ),
        Screen::Busy { job, .. } => (busy_text(*job, app.state.progress.as_ref()), theme.muted_foreground),
        Screen::Done { artifact } => (artifact_text(artifact), theme.success),
        // 完整文案（含渲染器安装指引）在通知里；状态栏只放首行。
        Screen::Failed { error, .. } => (
            error.lines().next().unwrap_or_default().to_string(),
            theme.danger,
        ),
    }
}

fn busy_text(job: Job, progress: Option<&Progress>) -> String {
    match job {
        Job::Load => "正在读取…".to_string(),
        Job::Generate => match progress {
            Some(Progress::Slides { done, total }) => format!("正在生成 {done}/{total}"),
            _ => "正在生成…".to_string(),
        },
        // 外呼渲染器不报页数进度；无模板走内置管线时进度由 Slides 提供。
        Job::ExportPng => "正在导出 PNG…".to_string(),
    }
}

fn artifact_text(artifact: &ArtifactView) -> String {
    match artifact.kind {
        ArtifactKind::Pptx => format!(
            "已生成 {} 张幻灯片 → {}",
            artifact.slides,
            artifact.path.display()
        ),
        ArtifactKind::Png => format!(
            "已导出 {} 张 PNG → {}",
            artifact.slides,
            artifact.path.display()
        ),
    }
}

/// 进度条比例：只有逐页进度有确定值（其余阶段给 Spinner）。
fn progress_ratio(state: &AppState) -> Option<f32> {
    match &state.progress {
        Some(Progress::Slides { done, total }) if *total > 0 => {
            Some(*done as f32 / *total as f32 * 100.)
        }
        _ => None,
    }
}

/// 输出框应显示的文本（`None` = 保持用户正在编辑的内容）。
///
/// `Auto` 的派生规则与状态机的输出计划同一条（同目录、同名 `.pptx`）；这里只用于**显示**，
/// 真正的落点决策始终在状态机 + `OutputPlan`。
fn output_text(state: &AppState) -> Option<String> {
    match &state.document.output {
        OutputField::Taken { path } => Some(path.display().to_string()),
        OutputField::Auto => Some(
            state
                .document
                .source
                .as_ref()
                .map(|source| source.with_extension("pptx").display().to_string())
                .unwrap_or_default(),
        ),
        OutputField::Invalid { .. } => None,
    }
}

/// 输入格式是否按 Excel 处理（扩展名派生；无输入时按 Excel 展示）。
pub(crate) fn is_excel(state: &AppState) -> bool {
    !state.document.source.as_deref().is_some_and(is_csv)
}

/// 面板通用的小字提示（静音）。
pub(crate) fn muted(text: impl Into<String>, color: Hsla) -> impl IntoElement {
    div().text_xs().text_color(color).child(text.into())
}
