//! 应用视图：状态机、后台任务与窗口根布局。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::table::TableState;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use vocab_core::diag::{self, Diag};
use vocab_core::paths::Layout;
use vocab_core::types::{GenerateError, InputSource, LoadError, TemplateError, WordEntry};
use vocab_core::{
    generator, png_export, reader, renderer, template, template_pptx, template_reader,
};

use crate::panels::data_preview::PreviewDelegate;
use crate::panels::{data_preview, file_picker, output_config};
use crate::APP_TITLE;

/// CSV 编码候选，与 reader::load_csv 接受的标签一致。
const CSV_ENCODINGS: [&str; 4] = ["UTF-8", "GBK", "GB2312", "GB18030"];

/// 后台任务进度的轮询间隔。
const PROGRESS_POLL: Duration = Duration::from_millis(80);

/// 输入文件外部修改的检查间隔。
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

/// 生成快捷键的展示文本：secondary 在 macOS 是 ⌘，其他平台是 Ctrl。
const GENERATE_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "Cmd+Enter"
} else {
    "Ctrl+Enter"
};

/// 根元素按键上下文：Ctrl/Cmd+Enter 生成。
const APP_CONTEXT: &str = "VocabApp";

actions!(vocab, [GeneratePpt]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Excel,
    Csv,
}

impl InputMode {
    /// 按扩展名推断输入格式，未知扩展名按 Excel 处理。
    fn detect(path: &str) -> Self {
        match Path::new(path).extension().and_then(|e| e.to_str()) {
            Some(ext) if ext.eq_ignore_ascii_case("csv") => Self::Csv,
            _ => Self::Excel,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Excel => 0,
            Self::Csv => 1,
        }
    }

    pub fn from_index(ix: usize) -> Self {
        if ix == 1 {
            Self::Csv
        } else {
            Self::Excel
        }
    }
}

/// 视图状态机。
#[derive(Clone)]
pub enum Status {
    Idle,
    Loading,
    Ready,
    Generating,
    ExportingPng,
    Done(SharedString),
    Failed(SharedString),
}

impl Status {
    /// 有后台任务在跑：拒绝重复提交，显示进度与取消。
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Loading | Self::Generating | Self::ExportingPng)
    }
}

/// 选定模板的校验状态；决定「生成」是否可用，并在界面上给出理由。
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateState {
    /// 未选择模板：使用内置版式。
    Unset,
    /// 正在后台扫描模板文件。
    Scanning,
    /// 模板可用：这些占位符会被填入。
    Ready(Vec<String>),
    /// 模板不可用（不存在 / 损坏 / 缺 {{单词}}），附带可展示的原因。
    Invalid(String),
}

/// 外部渲染器（Office / LibreOffice / WPS）探测结果；模板模式下 PNG 导出依赖它。
#[derive(Debug, Clone, PartialEq)]
pub enum RendererStatus {
    /// 尚未探测——只在选定模板后才探测。
    Unprobed,
    /// 探测中。
    Probing,
    /// 已找到可用的渲染器。
    Available,
    /// 未找到任何渲染器。
    Missing,
}

/// 输出路径的校验结果。
#[derive(Debug, Clone, PartialEq)]
pub enum OutputState {
    /// 可以直接写入。
    Free,
    /// 目标已存在；生成时会自动改用该路径。
    Taken(PathBuf),
    /// 不可用，附带可直接展示的原因。
    Invalid(&'static str),
}

/// 工作线程写入、前台轮询读取的进度。
#[derive(Clone)]
struct JobProgress(Arc<Mutex<(usize, usize)>>);

impl JobProgress {
    fn new() -> Self {
        Self(Arc::new(Mutex::new((0, 0))))
    }

    fn set(&self, current: usize, total: usize) {
        if let Ok(mut progress) = self.0.lock() {
            *progress = (current, total);
        }
    }

    fn get(&self) -> (usize, usize) {
        self.0.lock().map(|progress| *progress).unwrap_or((0, 0))
    }
}

/// reload 的后台产物。
struct LoadOutcome {
    sheets: Vec<String>,
    sheet: String,
    entries: Result<Vec<WordEntry>, LoadError>,
}

/// start_generate 的后台产物。
struct GenerateOutcome {
    result: Result<(), GenerateError>,
    output: PathBuf,
    count: usize,
}

/// start_export_png 的后台产物。
struct ExportOutcome {
    result: Result<Vec<PathBuf>, String>,
    dir: PathBuf,
}

pub struct VocabApp {
    // ── 输入 ──
    pub(crate) input_file: Entity<InputState>,
    pub(crate) sheet_select: Entity<SelectState<Vec<SharedString>>>,
    /// 当前输入文件的工作表清单；选择器是它的受控视图。
    pub(crate) sheets: Vec<SharedString>,
    pub(crate) encoding_select: Entity<SelectState<Vec<SharedString>>>,
    pub(crate) input_mode: InputMode,
    // ── 输出 ──
    pub(crate) output_file: Entity<InputState>,
    pub(crate) template_file: Entity<InputState>,
    // ── 数据与状态 ──
    pub(crate) preview: Entity<TableState<PreviewDelegate>>,
    pub(crate) status: Status,
    pub(crate) progress: (usize, usize),
    pub(crate) diag: Diag,
    /// 数据目录布局：默认输出与日志落点的唯一来源。
    pub(crate) layout: Layout,
    /// 会话日志建立失败时的提示。
    log_note: Option<String>,
    /// 最近一次成功产物的路径；None = 尚无。
    last_artifact: Option<PathBuf>,
    /// 选定模板的校验状态；输出区提示与禁用逻辑共用。
    pub(crate) template_state: TemplateState,
    /// 上一次触发模板扫描的输入值；避免同一路径重复扫描。
    last_template_value: String,
    /// 外部渲染器探测结果。
    pub(crate) renderer: RendererStatus,
    cancel: Option<Arc<AtomicBool>>,
    /// 正在跑的后台任务；丢弃即中止轮询与工作线程。
    job: Option<Task<()>>,
    /// 输入文件变化监听；仅持有以保持任务存活。
    _watcher: Task<()>,
    last_input_path: String,
    last_modified: Option<SystemTime>,
    _subscriptions: Vec<Subscription>,
}

impl VocabApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_layout(window, cx, Layout::resolve())
    }

    /// 用指定的数据目录布局构造（测试与嵌入场景）。
    pub fn with_layout(window: &mut Window, cx: &mut Context<Self>, layout: Layout) -> Self {
        let (diag, log_note) = match diag::run_log_sink(&layout.logs_dir(), "gui") {
            Ok(sink) => {
                let diag = Diag::with_sinks(vec![Box::new(sink)]);
                diag::record_session(&diag, "gui", layout.root());
                (diag, None)
            }
            Err(error) => (Diag::new(), Some(format!("日志不可用: {error}"))),
        };
        let input_file =
            cx.new(|cx| InputState::new(window, cx).placeholder("选择 Excel 或 CSV 文件"));
        let output_file = cx.new(|cx| InputState::new(window, cx).placeholder("输出 .pptx 路径"));
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
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let preview =
            cx.new(|cx| TableState::new(PreviewDelegate::new(window.rem_size()), window, cx));

        let mut subscriptions = Vec::new();

        // Ctrl/Cmd+Enter → 生成（焦点在输入框时由 Input 的订阅处理）
        cx.bind_keys([KeyBinding::new(
            "secondary-enter",
            GeneratePpt,
            Some(APP_CONTEXT),
        )]);

        // 路径输入框回车 → 打开该路径
        subscriptions.push(cx.subscribe_in(
            &input_file,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter {
                    secondary: true, ..
                } => this.start_generate(window, cx),
                InputEvent::PressEnter { .. } => {
                    let path = state.read(cx).value().to_string();
                    if !path.trim().is_empty() && path != this.last_input_path {
                        this.open_input(path, window, cx);
                    }
                }
                _ => {}
            },
        ));

        // 工作表切换 → 按新表重新读取
        subscriptions.push(cx.subscribe_in(
            &sheet_select,
            window,
            |this, _state, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(_)) = event {
                    if this.input_mode == InputMode::Excel && !this.status.is_busy() {
                        this.reload(window, cx);
                    }
                }
            },
        ));

        // CSV 编码切换 → 按新编码重新读取
        subscriptions.push(cx.subscribe_in(
            &encoding_select,
            window,
            |this, _state, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(_)) = event {
                    this.reload_for_encoding_change(window, cx);
                }
            },
        ));

        // 输出 / 模板框里的 Ctrl/Cmd+Enter → 生成；模板框变动 → 重新校验
        subscriptions.push(cx.subscribe_in(
            &output_file,
            window,
            |this, _state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter {
                    secondary: true, ..
                } = event
                {
                    this.start_generate(window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &template_file,
            window,
            |this, _state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter {
                    secondary: true, ..
                } => this.start_generate(window, cx),
                InputEvent::Change => this.refresh_template_state(window, cx),
                _ => {}
            },
        ));

        let _watcher = Self::start_watcher(window, cx);

        Self {
            input_file,
            sheet_select,
            sheets: Vec::new(),
            encoding_select,
            input_mode: InputMode::Excel,
            output_file,
            template_file,
            preview,
            status: Status::Idle,
            progress: (0, 0),
            diag,
            layout,
            log_note,
            last_artifact: None,
            template_state: TemplateState::Unset,
            last_template_value: String::new(),
            renderer: RendererStatus::Unprobed,
            cancel: None,
            job: None,
            _watcher,
            last_input_path: String::new(),
            last_modified: None,
            _subscriptions: subscriptions,
        }
    }

    // ── 选择与路径 ──

    /// 打开指定输入文件：写入路径框、推断格式、推导输出路径并发起读取。
    pub fn open_input(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if path.trim().is_empty() {
            return;
        }
        self.input_file
            .update(cx, |state, cx| state.set_value(path.clone(), window, cx));
        self.input_mode = InputMode::detect(&path);
        self.derive_output_path(&path, window, cx);
        self.last_input_path = path.clone();
        self.last_modified = file_modified(Path::new(&path));
        window.set_window_title(&window_title(&path));
        self.reload(window, cx);
    }

    /// 处理拖入窗口的文件：取第一个 .xlsx / .xls / .csv 打开，其余给出提示。
    pub fn handle_drop(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = paths.paths().first() else {
            return;
        };
        let supported = ["xlsx", "xls", "csv"].iter().any(|candidate| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case(candidate))
        });
        if !supported {
            self.diag.warn(
                "gui",
                format!("拖入的路径不是 Excel/CSV 文件: {}", path.display()),
                None,
            );
            window.push_notification(
                (
                    NotificationType::Error,
                    "请拖入单个 .xlsx / .xls / .csv 文件",
                ),
                cx,
            );
            return;
        }
        self.open_input(path.display().to_string(), window, cx);
    }

    pub fn pick_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = rfd::FileDialog::new()
            .add_filter("Excel/CSV", &["xlsx", "xls", "csv"])
            .pick_file();
        if let Some(path) = picked {
            self.open_input(path.display().to_string(), window, cx);
        }
    }

    pub fn pick_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = rfd::FileDialog::new()
            .add_filter("PPTX", &["pptx"])
            .save_file();
        if let Some(path) = picked {
            self.output_file.update(cx, |state, cx| {
                state.set_value(path.display().to_string(), window, cx)
            });
            cx.notify();
        }
    }

    pub fn pick_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = rfd::FileDialog::new()
            .add_filter("PPTX 模板", &["pptx"])
            .pick_file();
        if let Some(path) = picked {
            self.template_file.update(cx, |state, cx| {
                state.set_value(path.display().to_string(), window, cx)
            });
            // set_value 不触发 InputEvent::Change：显式开始模板校验。
            self.refresh_template_state(window, cx);
            cx.notify();
        }
    }

    pub fn set_input_mode(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mode = InputMode::from_index(ix);
        if mode == self.input_mode {
            return;
        }
        self.input_mode = mode;
        if self.input_path(cx).trim().is_empty() {
            cx.notify();
        } else {
            self.reload(window, cx);
        }
    }

    fn derive_output_path(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = Path::new(input);
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("output");
        let output = self.layout.ppt_file(stem);
        self.output_file.update(cx, |state, cx| {
            state.set_value(output.display().to_string(), window, cx)
        });
    }

    pub fn input_path(&self, cx: &App) -> String {
        self.input_file.read(cx).value().to_string()
    }

    pub fn output_path(&self, cx: &App) -> String {
        self.output_file.read(cx).value().to_string()
    }

    /// 模板路径；未选择时为 None，即使用内置版式。
    fn template_path(&self, cx: &App) -> Option<PathBuf> {
        let path = self.template_file.read(cx).value().to_string();
        if path.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        }
    }

    pub fn selected_sheet(&self, cx: &App) -> String {
        self.sheet_select
            .read(cx)
            .selected_value()
            .map(SharedString::to_string)
            .unwrap_or_default()
    }

    pub fn selected_encoding(&self, cx: &App) -> String {
        self.encoding_select
            .read(cx)
            .selected_value()
            .map(SharedString::to_string)
            .unwrap_or_else(|| "UTF-8".to_string())
    }

    pub fn entry_count(&self, cx: &App) -> usize {
        self.preview.read(cx).delegate().entries().len()
    }

    /// 是否有后台任务在跑：外部观察者与集成测试读取。
    pub fn is_busy(&self) -> bool {
        self.status.is_busy()
    }

    // ── 读取 ──

    /// 读取输入文件：Excel 先列出工作表，完成后刷新选择器、预览与诊断。
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status.is_busy() {
            return;
        }
        let path = PathBuf::from(self.input_path(cx));
        if path.as_os_str().is_empty() {
            return;
        }
        let mode = self.input_mode;
        let sheet = self.selected_sheet(cx);
        let encoding = self.selected_encoding(cx);

        self.status = Status::Loading;
        self.progress = (0, 0);
        self.last_modified = file_modified(&path);
        self.preview.update(cx, |state, cx| {
            state.delegate_mut().set_loading(true);
            state.refresh(cx);
        });
        cx.notify();

        let worker_diag = self.diag.clone();
        let worker = cx.background_spawn(async move {
            let sheets = if mode == InputMode::Excel {
                reader::list_sheets(&path, &worker_diag).unwrap_or_default()
            } else {
                Vec::new()
            };
            let sheet = sheets
                .iter()
                .find(|name| **name == sheet)
                .cloned()
                .unwrap_or_else(|| {
                    sheets
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "Sheet1".to_string())
                });
            let source = match mode {
                InputMode::Excel => InputSource::Excel {
                    path,
                    sheet: sheet.clone(),
                },
                InputMode::Csv => InputSource::Csv { path, encoding },
            };
            LoadOutcome {
                entries: reader::load(&source, &worker_diag),
                sheets,
                sheet,
            }
        });

        let task = Self::spawn_job(window, cx, None, worker, |this, outcome, window, cx| {
            this.finish_load(outcome, window, cx)
        });
        self.job = Some(task);
    }

    fn finish_load(&mut self, outcome: LoadOutcome, window: &mut Window, cx: &mut Context<Self>) {
        self.job = None;

        self.sheets = outcome
            .sheets
            .iter()
            .map(|name| SharedString::from(name.clone()))
            .collect();
        let selected = self
            .sheets
            .iter()
            .position(|name| name.as_ref() == outcome.sheet)
            .map(|ix| IndexPath::default().row(ix));
        let sheets = self.sheets.clone();
        self.sheet_select.update(cx, |state, cx| {
            state.set_items(sheets, window, cx);
            state.set_selected_index(selected, window, cx);
        });

        let (entries, error) = match outcome.entries {
            Ok(entries) => (entries, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.preview.update(cx, |state, cx| {
            state.delegate_mut().set_loading(false);
            state.delegate_mut().set_entries(entries);
            state.refresh(cx);
        });

        self.status = match &error {
            None => Status::Ready,
            Some(error) => Status::Failed(format!("读取失败: {error}").into()),
        };
        if let Some(error) = error {
            window.push_notification((NotificationType::Error, format!("读取失败: {error}")), cx);
        }
        cx.notify();
    }

    // ── 生成 ──

    /// 生成 PPTX：后台线程执行，前台轮询进度，完成后通知。
    pub fn start_generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status.is_busy() {
            return;
        }
        let entries: Vec<WordEntry> = self.preview.read(cx).delegate().entries().to_vec();
        if entries.is_empty() {
            return;
        }
        if let OutputState::Invalid(reason) = self.output_state(cx) {
            window.push_notification((NotificationType::Error, reason), cx);
            return;
        }
        if let TemplateState::Invalid(reason) = &self.template_state {
            window.push_notification((NotificationType::Error, reason.clone()), cx);
            return;
        }

        // 自动重命名：输出文件已存在时追加 (1) (2) …，避免覆盖。
        let requested = PathBuf::from(self.output_path(cx));
        let output = find_available_path(&requested);
        self.output_file.update(cx, |state, cx| {
            state.set_value(output.display().to_string(), window, cx)
        });
        let template = self.template_path(cx);

        let progress = JobProgress::new();
        let cancel = Arc::new(AtomicBool::new(false));
        self.status = Status::Generating;
        self.progress = (0, entries.len());
        self.cancel = Some(cancel.clone());
        cx.notify();

        let worker_progress = progress.clone();
        let worker_cancel = cancel.clone();
        let count = entries.len();
        let worker_diag = self.diag.clone();
        let worker = cx.background_spawn(async move {
            let record = |current: usize, total: usize| {
                worker_progress.set(current, total);
                !worker_cancel.load(Ordering::Relaxed)
            };
            let result = match &template {
                Some(template) => generator::generate_from_template(
                    &entries,
                    template,
                    &output,
                    record,
                    &worker_diag,
                ),
                None => generator::generate(&entries, &output, record, &worker_diag),
            };
            GenerateOutcome {
                result,
                output,
                count,
            }
        });

        let task = Self::spawn_job(
            window,
            cx,
            Some(progress),
            worker,
            |this, outcome, window, cx| this.finish_generate(outcome, window, cx),
        );
        self.job = Some(task);
    }

    fn finish_generate(
        &mut self,
        outcome: GenerateOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.job = None;
        self.cancel = None;

        match outcome.result {
            Ok(()) => {
                self.last_artifact = Some(outcome.output.clone());
                self.status = Status::Done(format!("已生成 {} 张幻灯片", outcome.count).into());
                window.push_notification(
                    (
                        NotificationType::Success,
                        format!(
                            "已生成 {} 张幻灯片 → {}",
                            outcome.count,
                            outcome.output.display()
                        ),
                    ),
                    cx,
                );
            }
            Err(GenerateError::Cancelled) => {
                self.status = Status::Ready;
                window.push_notification((NotificationType::Info, "已取消生成"), cx);
            }
            Err(error) => {
                self.status = Status::Failed(format!("生成失败: {error}").into());
                window
                    .push_notification((NotificationType::Error, format!("生成失败: {error}")), cx);
            }
        }
        cx.notify();
    }

    // ── 导出 PNG ──

    /// 导出 PNG：模板模式调用外部渲染器，无模板时走内置 SVG 管线。
    pub fn start_export_png(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status.is_busy() {
            return;
        }
        let entries: Vec<WordEntry> = self.preview.read(cx).delegate().entries().to_vec();
        if entries.is_empty() {
            return;
        }
        if let TemplateState::Invalid(reason) = &self.template_state {
            window.push_notification((NotificationType::Error, reason.clone()), cx);
            return;
        }
        let template = self.template_path(cx);
        if template.is_some() && matches!(self.renderer, RendererStatus::Missing) {
            window.push_notification(
                (
                    NotificationType::Error,
                    "未检测到 Office / LibreOffice / WPS，PNG 导出不可用（生成 PPT 不受影响）",
                ),
                cx,
            );
            return;
        }
        let dir = self.layout.png_dir();

        let cancel = Arc::new(AtomicBool::new(false));
        self.status = Status::ExportingPng;
        self.progress = (0, 0);
        self.cancel = Some(cancel.clone());
        cx.notify();

        let worker_diag = self.diag.clone();
        let worker = cx.background_spawn(async move {
            if let Err(error) = std::fs::create_dir_all(&dir) {
                return ExportOutcome {
                    result: Err(format!("创建输出目录失败: {error}")),
                    dir,
                };
            }
            let result = match &template {
                Some(template) => {
                    let pptx_path = dir.join("_generated.pptx");
                    match generator::generate_from_template(
                        &entries,
                        template,
                        &pptx_path,
                        |_, _| !cancel.load(Ordering::Relaxed),
                        &worker_diag,
                    ) {
                        Err(error) => Err(format!("生成 PPTX 失败: {error}")),
                        Ok(()) => match renderer::render_pptx(&pptx_path, &dir, &worker_diag) {
                            Ok(pngs) => {
                                remove_generated_pptx(&pptx_path, &worker_diag);
                                Ok(pngs)
                            }
                            Err(renderer::RenderError::NoRenderer) => {
                                remove_generated_pptx(&pptx_path, &worker_diag);
                                Err(renderer::RENDERER_HELP.to_string())
                            }
                            Err(error) => {
                                remove_generated_pptx(&pptx_path, &worker_diag);
                                Err(format!("外部渲染器失败: {error}"))
                            }
                        },
                    }
                }
                None => png_export::export_entries_to_png(
                    &entries,
                    &dir,
                    Some(cancel.as_ref()),
                    &worker_diag,
                )
                .map_err(|error| error.to_string()),
            };
            ExportOutcome { result, dir }
        });

        let task = Self::spawn_job(window, cx, None, worker, |this, outcome, window, cx| {
            this.finish_export_png(outcome, window, cx)
        });
        self.job = Some(task);
    }

    fn finish_export_png(
        &mut self,
        outcome: ExportOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.job = None;
        self.cancel = None;

        match outcome.result {
            Ok(paths) => {
                self.last_artifact = Some(outcome.dir.clone());
                let message = format!("已导出 {} 张 PNG → {}", paths.len(), outcome.dir.display());
                self.status = Status::Done(message.clone().into());
                window.push_notification((NotificationType::Success, message), cx);
            }
            Err(error) => {
                self.status = Status::Failed(format!("PNG 导出失败: {error}").into());
                window.push_notification(
                    (NotificationType::Error, format!("PNG 导出失败: {error}")),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// 在浅色/深色之间切换。
    ///
    /// 主题模式属于框架的全局 `Theme`，这里只发一次切换请求，不缓存副本；
    /// 同时把原生窗口边框也切到同一侧，避免暗色界面配亮色标题栏。
    pub fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = cx.theme().is_dark();
        Theme::change(
            if dark {
                ThemeMode::Light
            } else {
                ThemeMode::Dark
            },
            Some(window),
            cx,
        );
        cx.set_window_appearance(Some(if dark {
            WindowAppearance::Light
        } else {
            WindowAppearance::Dark
        }));
    }

    pub fn cancel_job(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    // ── 内置模板 ──

    /// 导出 Excel 词汇表模板并用系统默认程序打开。
    pub fn open_excel_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.layout.template_xlsx();
        if !path.exists() {
            if let Err(error) = template::export_template(&path) {
                window.push_notification(
                    (NotificationType::Error, format!("导出模板失败: {error}")),
                    cx,
                );
                return;
            }
        }
        reveal_path(&path, &self.diag, window, cx);
    }

    /// 打开数据目录（日志、模板与默认输出的所在地）。
    pub fn open_data_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.layout.root().to_path_buf();
        if let Err(error) = std::fs::create_dir_all(&root) {
            self.diag
                .error("gui", format!("无法创建数据目录: {error}"), None);
            window.push_notification(
                (
                    NotificationType::Error,
                    format!("无法创建数据目录: {error}"),
                ),
                cx,
            );
            return;
        }
        reveal_path(&root, &self.diag, window, cx);
    }

    /// 生成示例 PPTX 模板（含占位符）并用系统默认程序打开。
    pub fn open_example_pptx(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.layout.template_pptx();
        if !path.exists() {
            if let Err(error) = template_pptx::generate_example_pptx(&path) {
                window.push_notification(
                    (
                        NotificationType::Error,
                        format!("生成示例模板失败: {error}"),
                    ),
                    cx,
                );
                return;
            }
        }
        if reveal_path(&path, &self.diag, window, cx) {
            window.push_notification(
                (
                    NotificationType::Info,
                    "示例模板已打开——编辑保存后在「PPTX 模板」里选择它",
                ),
                cx,
            );
        }
    }

    // ── 校验与提示 ──

    /// 输出路径的校验结果；输出区提示与「生成」可用性共用。
    pub fn output_state(&self, cx: &App) -> OutputState {
        let value = self.output_path(cx);
        let value = value.trim();
        if value.is_empty() {
            return OutputState::Invalid("输出路径不能为空");
        }
        let path = Path::new(value);
        let is_pptx = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pptx"));
        if !is_pptx {
            return OutputState::Invalid("输出需要 .pptx 扩展名");
        }
        if path.exists() {
            return OutputState::Taken(find_available_path(path));
        }
        OutputState::Free
    }

    /// 「生成 PPT」是否可用。
    pub fn can_generate(&self, cx: &App) -> bool {
        self.base_ready(cx) && !matches!(self.output_state(cx), OutputState::Invalid(_))
    }

    /// 「导出 PNG」是否可用；模板模式下还要求检测到外部渲染器。
    pub fn can_export_png(&self, cx: &App) -> bool {
        self.base_ready(cx)
            && !(self.template_path(cx).is_some()
                && matches!(self.renderer, RendererStatus::Missing))
    }

    /// 两个动作共同的前置条件：有数据、不忙、模板校验通过。
    fn base_ready(&self, cx: &App) -> bool {
        !self.status.is_busy()
            && self.entry_count(cx) > 0
            && !matches!(self.template_state, TemplateState::Invalid(_))
    }

    /// 最近一次成功产物的路径；状态栏与集成测试共用。
    pub fn last_artifact(&self) -> Option<&Path> {
        self.last_artifact.as_deref()
    }

    /// 选定模板的校验状态。
    pub fn template_state(&self) -> &TemplateState {
        &self.template_state
    }

    /// 预览表是否在加载中；预览区空态与加载态的切换依据。
    pub fn preview_loading(&self, cx: &App) -> bool {
        self.preview.read(cx).delegate().is_loading()
    }

    /// CSV 编码确认：按新编码重新读取（Excel 模式或读取中不响应）。
    pub fn reload_for_encoding_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input_mode == InputMode::Csv && !self.status.is_busy() {
            self.reload(window, cx);
        }
    }

    /// 模板框内容变化：为空 → 内置版式；否则后台扫描占位符。
    pub fn refresh_template_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.template_file.read(cx).value().to_string();
        if value == self.last_template_value {
            return;
        }
        self.last_template_value = value.clone();

        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.template_state = TemplateState::Unset;
            cx.notify();
            return;
        }
        let path = PathBuf::from(trimmed);
        if !path.exists() {
            self.template_state =
                TemplateState::Invalid(format!("模板文件不存在：{}", path.display()));
            cx.notify();
            return;
        }

        self.template_state = TemplateState::Scanning;
        cx.notify();
        self.ensure_renderer_probe(window, cx);

        let diag = self.diag.clone();
        let worker =
            cx.background_spawn(async move { template_reader::scan_pptx_template(&path, &diag) });
        let scanned_value = value;
        cx.spawn_in(window, async move |this, cx| {
            let result = worker.await;
            this.update_in(cx, |this, _window, cx| {
                this.finish_template_scan(&scanned_value, result, cx)
            })
            .ok();
        })
        .detach();
    }

    /// 模板扫描结果落地；扫描期间模板框被改动则丢弃过期结果。
    fn finish_template_scan(
        &mut self,
        scanned_value: &str,
        result: Result<template_reader::TemplateScan, TemplateError>,
        cx: &mut Context<Self>,
    ) {
        if self.template_file.read(cx).value().as_ref() != scanned_value {
            return;
        }
        self.template_state = match result {
            Err(error) => TemplateState::Invalid(error.to_string()),
            Ok(scan) if !scan.placeholders.iter().any(|name| name == "单词") => {
                TemplateState::Invalid("模板缺少必填占位符 {{单词}}，无法生成".to_string())
            }
            Ok(scan) => TemplateState::Ready(scan.placeholders),
        };
        cx.notify();
    }

    /// 惰性探测外部渲染器：只在第一次选定模板后执行一次。
    fn ensure_renderer_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.renderer, RendererStatus::Unprobed) {
            return;
        }
        self.renderer = RendererStatus::Probing;
        let diag = self.diag.clone();
        let worker = cx.background_spawn(async move { renderer::detect_renderer(&diag).is_some() });
        cx.spawn_in(window, async move |this, cx| {
            let available = worker.await;
            this.update_in(cx, |this, _window, cx| {
                this.renderer = if available {
                    RendererStatus::Available
                } else {
                    RendererStatus::Missing
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 打开最近产物（文件或目录）。
    pub fn open_last_artifact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.last_artifact.clone() {
            reveal_path(&path, &self.diag, window, cx);
        }
    }

    /// 打开日志目录；诊断计数入口。
    pub fn open_logs_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.layout.logs_dir();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.diag
                .error("gui", format!("无法创建日志目录: {error}"), None);
            window.push_notification(
                (
                    NotificationType::Error,
                    format!("无法创建日志目录: {error}"),
                ),
                cx,
            );
            return;
        }
        reveal_path(&dir, &self.diag, window, cx);
    }

    // ── 后台任务管线 ──

    /// 在后台执行 worker，可选地轮询进度，完成后回到前台交给 finish。
    fn spawn_job<T, F>(
        window: &mut Window,
        cx: &mut Context<Self>,
        progress: Option<JobProgress>,
        worker: Task<T>,
        finish: F,
    ) -> Task<()>
    where
        T: Send + 'static,
        F: FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    {
        cx.spawn_in(window, async move |this, cx| {
            if let Some(progress) = progress {
                while !worker.is_ready() {
                    cx.background_executor().timer(PROGRESS_POLL).await;
                    let (current, total) = progress.get();
                    this.update_in(cx, |this, _window, cx| {
                        this.progress = (current, total);
                        cx.notify();
                    })
                    .ok();
                }
            }
            let outcome = worker.await;
            this.update_in(cx, |this, window, cx| finish(this, outcome, window, cx))
                .ok();
        })
    }

    /// 输入文件被外部修改时自动刷新预览。
    fn start_watcher(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(WATCH_INTERVAL).await;
            let alive = this
                .update_in(cx, |this, window, cx| this.tick_watch(window, cx))
                .is_ok();
            if !alive {
                break;
            }
        })
    }

    fn tick_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status.is_busy() {
            return;
        }
        let path = self.input_path(cx);
        if path.is_empty() {
            return;
        }
        let modified = file_modified(Path::new(&path));
        if path != self.last_input_path {
            // 路径变化（手动输入或外部）→ 直接打开
            if modified.is_some() {
                self.open_input(path, window, cx);
            }
            return;
        }
        if modified != self.last_modified {
            self.last_modified = modified;
            if modified.is_some() {
                self.reload(window, cx);
            }
        }
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let can_generate = self.can_generate(cx);
        let can_export_png = self.can_export_png(cx);
        // 只有生成与 PNG 导出有取消通道；读取没有，别显示死按钮。
        let can_cancel = matches!(self.status, Status::Generating | Status::ExportingPng);
        let generating = matches!(self.status, Status::Generating);
        let exporting = matches!(self.status, Status::ExportingPng);
        let (current, total) = self.progress;
        let percent = if total > 0 {
            current as f32 / total as f32 * 100.
        } else {
            0.
        };
        // 核心把告警写进会话日志（例如 CSV 无效字节被替换），这里给出可见入口。
        let diag_issues = self.diag.warnings() + self.diag.errors();
        let (status_text, status_color) = match &self.status {
            Status::Idle => (
                "请选择 Excel 或 CSV 文件".to_string(),
                cx.theme().muted_foreground,
            ),
            Status::Loading => ("正在读取…".to_string(), cx.theme().muted_foreground),
            // 记录数已经在预览区标题上，状态栏只报操作状态。
            Status::Ready => ("就绪".to_string(), cx.theme().muted_foreground),
            Status::Generating => (
                format!("正在生成 {current}/{total}"),
                cx.theme().muted_foreground,
            ),
            Status::ExportingPng => ("正在导出 PNG…".to_string(), cx.theme().muted_foreground),
            Status::Done(message) => (message.to_string(), cx.theme().success),
            Status::Failed(message) => (message.to_string(), cx.theme().danger),
        };

        h_flex()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(cx.theme().border)
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
                            .text_color(status_color)
                            .child(status_text),
                    )
                    .when_some(self.last_artifact.clone(), |this, path| {
                        let is_dir = path.is_dir();
                        this.child(
                            Button::new("open-artifact")
                                .ghost()
                                .small()
                                .icon(if is_dir {
                                    IconName::FolderOpen
                                } else {
                                    IconName::ExternalLink
                                })
                                .label(if is_dir {
                                    "打开 PNG 目录"
                                } else {
                                    "打开 PPT"
                                })
                                .tooltip(path.display().to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_last_artifact(window, cx)
                                })),
                        )
                    })
                    .when(diag_issues > 0, |this| {
                        this.child(
                            Button::new("open-logs")
                                .ghost()
                                .small()
                                .icon(if self.diag.errors() > 0 {
                                    IconName::TriangleAlert
                                } else {
                                    IconName::Info
                                })
                                .label(format!(
                                    "诊断：警告 {} · 错误 {}",
                                    self.diag.warnings(),
                                    self.diag.errors()
                                ))
                                .tooltip("打开日志目录（最近 20 次运行的完整事件）")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_logs_dir(window, cx)
                                })),
                        )
                    })
                    .when(self.diag.write_failures() > 0, |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(format!("日志写入失败 ×{}", self.diag.write_failures())),
                        )
                    })
                    .when_some(self.log_note.clone(), |this, note| {
                        this.child(div().text_sm().text_color(cx.theme().danger).child(note))
                    })
                    .when(generating && total > 0, |this| {
                        this.child(
                            ProgressBar::new("job-progress")
                                .value(percent)
                                .small()
                                .w_40(),
                        )
                    })
                    .when(exporting, |this| {
                        // 外部渲染器不报进度：给不确定的旋转指示，避免假的百分比。
                        this.child(Spinner::new().small().color(cx.theme().muted_foreground))
                    })
                    .when(can_cancel, |this| {
                        this.child(
                            Button::new("cancel-job")
                                .ghost()
                                .small()
                                .label("取消")
                                .on_click(cx.listener(|this, _, _window, cx| this.cancel_job(cx))),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("open-data-dir")
                            .ghost()
                            .icon(IconName::FolderOpen)
                            .accessibility_label("打开数据目录")
                            .tooltip("打开数据目录（日志、模板与默认输出）")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_data_dir(window, cx)),
                            ),
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
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_theme(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("export-png")
                            .outline()
                            .label("导出 PNG")
                            .icon(IconName::GalleryVerticalEnd)
                            .tooltip(
                                "把词条导出为 PNG 图片；模板模式需要 Office / LibreOffice / WPS",
                            )
                            .disabled(!can_export_png)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.start_export_png(window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("generate")
                            .primary()
                            .label("生成 PPT")
                            .icon(IconName::Play)
                            .tooltip(format!("生成 PPT（{GENERATE_SHORTCUT}）"))
                            .disabled(!can_generate)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.start_generate(window, cx)),
                            ),
                    ),
            )
    }
}

impl Render for VocabApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .key_context(APP_CONTEXT)
            .on_action(cx.listener(|this, _: &GeneratePpt, window, cx| {
                this.start_generate(window, cx);
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.handle_drop(paths, window, cx);
            }))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(file_picker::render(self, window, cx))
            .child(output_config::render(self, window, cx))
            .child(data_preview::render(self, cx))
            .child(self.render_footer(cx))
    }
}

/// 自动重命名：若文件已存在，追加 (1) (2) … 直到找到可用名。
///
/// 名字以 ` (n)` 结尾时先剥离旧序号，避免连续生成叠成 `words (1) (1).pptx`。
pub fn find_available_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("output");
    let stripped = strip_index_suffix(stem);
    let base = if stripped.is_empty() { stem } else { stripped };
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("pptx");
    let parent = path.parent().unwrap_or(Path::new("."));
    for n in 1..1000u32 {
        let candidate = parent.join(format!("{base} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // 极端情况：回退到带进程号的文件名
    parent.join(format!("{base}_{}.{ext}", std::process::id()))
}

/// 剥离结尾的 ` (数字)`；没有该后缀时原样返回。
fn strip_index_suffix(stem: &str) -> &str {
    let Some(start) = stem.rfind(" (") else {
        return stem;
    };
    let Some(inner) = stem[start + 2..].strip_suffix(')') else {
        return stem;
    };
    if !inner.is_empty() && inner.bytes().all(|byte| byte.is_ascii_digit()) {
        &stem[..start]
    } else {
        stem
    }
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

/// 模板模式导出 PNG 时的中间产物，渲染结束后删除。
///
/// 删除失败不中断导出，但必须留下可查询的诊断事件。
fn remove_generated_pptx(path: &Path, diag: &Diag) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            diag.warn(
                "export",
                format!("无法删除中间文件 {}: {error}", path.display()),
                None,
            );
        }
    }
}

/// 窗口标题：应用名 + 当前输入文件名。
fn window_title(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{APP_TITLE} — {name}"))
        .unwrap_or_else(|| APP_TITLE.to_string())
}

/// 用系统默认程序打开路径；失败时记录诊断、推送错误通知并返回 false。
///
/// 通知只给路径与恢复动作；启动器的原始错误（可能是一整条命令）进日志，不进 toast。
fn reveal_path(path: &Path, diag: &Diag, window: &mut Window, cx: &mut App) -> bool {
    match open::that(path) {
        Ok(()) => true,
        Err(error) => {
            diag.error(
                "gui",
                format!("打开失败 {}: {error}", path.display()),
                Some(serde_json::json!({"path": path.display().to_string()})),
            );
            window.push_notification(
                (
                    NotificationType::Error,
                    format!("无法打开，请手动访问：{}", path.display()),
                ),
                cx,
            );
            false
        }
    }
}
