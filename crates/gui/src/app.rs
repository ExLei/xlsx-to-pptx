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
use gpui_kit::component::table::TableState;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use vocab_core::diag::DiagStore;
use vocab_core::types::{GenerateError, InputSource, LoadError, TemplateError, WordEntry};
use vocab_core::{generator, png_export, reader, renderer, template, template_pptx};

use crate::panels::data_preview::PreviewDelegate;
use crate::panels::{data_preview, file_picker, output_config};

/// CSV 编码候选，与 reader::load_csv 接受的标签一致。
const CSV_ENCODINGS: [&str; 4] = ["UTF-8", "GBK", "GB2312", "GB18030"];

/// 后台任务进度的轮询间隔。
const PROGRESS_POLL: Duration = Duration::from_millis(80);

/// 输入文件外部修改的检查间隔。
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

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
    diag: DiagStore,
}

/// start_generate 的后台产物。
struct GenerateOutcome {
    result: Result<(), GenerateError>,
    output: PathBuf,
    count: usize,
    diag: DiagStore,
}

/// start_export_png 的后台产物。
struct ExportOutcome {
    result: Result<Vec<PathBuf>, String>,
    dir: PathBuf,
    diag: DiagStore,
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
    pub(crate) diag: DiagStore,
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

        // 路径输入框回车 → 打开该路径
        subscriptions.push(cx.subscribe_in(
            &input_file,
            window,
            |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let path = state.read(cx).value().to_string();
                    if !path.trim().is_empty() && path != this.last_input_path {
                        this.open_input(path, window, cx);
                    }
                }
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
            diag: DiagStore::new(),
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
        self.reload(window, cx);
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
        let parent = path.parent().unwrap_or(Path::new("."));
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("output");
        let output = parent.join(format!("{stem}.pptx"));
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
        self.last_modified = file_modified(&path);
        self.preview.update(cx, |state, cx| {
            state.delegate_mut().set_loading(true);
            state.refresh(cx);
        });
        cx.notify();

        let worker = cx.background_spawn(async move {
            let mut diag = DiagStore::new();
            let sheets = if mode == InputMode::Excel {
                reader::list_sheets(&path, &mut diag).unwrap_or_default()
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
                entries: reader::load(&source, &mut diag),
                sheets,
                sheet,
                diag,
            }
        });

        let task = Self::spawn_job(window, cx, None, worker, |this, outcome, window, cx| {
            this.finish_load(outcome, window, cx)
        });
        self.job = Some(task);
    }

    fn finish_load(&mut self, outcome: LoadOutcome, window: &mut Window, cx: &mut Context<Self>) {
        self.job = None;
        self.merge_diag(outcome.diag);

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
        let worker = cx.background_spawn(async move {
            let mut diag = DiagStore::new();
            let record = |current: usize, total: usize| {
                worker_progress.set(current, total);
                !worker_cancel.load(Ordering::Relaxed)
            };
            let result = match &template {
                Some(template) => generator::generate_from_template(
                    &entries, template, &output, record, &mut diag,
                ),
                None => generator::generate(&entries, &output, record, &mut diag),
            };
            GenerateOutcome {
                result,
                output,
                count,
                diag,
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
        self.merge_diag(outcome.diag);

        match outcome.result {
            Ok(()) => {
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
        let dir = Path::new(&self.output_path(cx))
            .parent()
            .unwrap_or(Path::new("."))
            .join("png_output");
        let template = self.template_path(cx);

        let cancel = Arc::new(AtomicBool::new(false));
        self.status = Status::ExportingPng;
        self.cancel = Some(cancel.clone());
        cx.notify();

        let worker = cx.background_spawn(async move {
            let mut diag = DiagStore::new();
            if let Err(error) = std::fs::create_dir_all(&dir) {
                return ExportOutcome {
                    result: Err(format!("创建输出目录失败: {error}")),
                    dir,
                    diag,
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
                        &mut diag,
                    ) {
                        Err(error) => Err(format!("生成 PPTX 失败: {error}")),
                        Ok(()) => match renderer::render_pptx(&pptx_path, &dir, &mut diag) {
                            Ok(pngs) => {
                                remove_generated_pptx(&pptx_path, &mut diag);
                                Ok(pngs)
                            }
                            Err(renderer::RenderError::NoRenderer) => {
                                remove_generated_pptx(&pptx_path, &mut diag);
                                Err(renderer::RENDERER_HELP.to_string())
                            }
                            Err(error) => {
                                remove_generated_pptx(&pptx_path, &mut diag);
                                Err(format!("外部渲染器失败: {error}"))
                            }
                        },
                    }
                }
                None => png_export::export_entries_to_png_with_cancel(
                    &entries,
                    &dir,
                    Some(cancel.as_ref()),
                )
                .map_err(|error| error.to_string()),
            };
            ExportOutcome { result, dir, diag }
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
        self.merge_diag(outcome.diag);

        match outcome.result {
            Ok(paths) => {
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

    pub fn cancel_job(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    // ── 内置模板 ──

    /// 导出 Excel 词汇表模板并用系统默认程序打开。
    pub fn open_excel_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = std::env::temp_dir().join("单词表模板.xlsx");
        let opened = template::export_template(&path).and_then(|()| {
            open::that(&path)
                .map_err(|error| TemplateError::IoError(std::io::Error::other(error.to_string())))
        });
        match opened {
            Ok(()) => window.push_notification(
                (
                    NotificationType::Success,
                    format!("已生成 {} 并打开", path.display()),
                ),
                cx,
            ),
            Err(error) => window.push_notification(
                (NotificationType::Error, format!("导出模板失败: {error}")),
                cx,
            ),
        }
    }

    /// 生成示例 PPTX 模板（含占位符）并用系统默认程序打开。
    pub fn open_example_pptx(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = std::env::temp_dir().join("示例模板.pptx");
        let opened = template_pptx::generate_example_pptx(&path)
            .map_err(|error| error.to_string())
            .and_then(|()| open::that(&path).map_err(|error| error.to_string()));
        match opened {
            Ok(()) => window.push_notification(
                (
                    NotificationType::Success,
                    format!("已生成 {} 并打开", path.display()),
                ),
                cx,
            ),
            Err(error) => window.push_notification(
                (
                    NotificationType::Error,
                    format!("生成示例模板失败: {error}"),
                ),
                cx,
            ),
        }
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

    /// 保留最近一次有事件的诊断；新任务未产出诊断时不清空已有信息。
    fn merge_diag(&mut self, diag: DiagStore) {
        if diag.event_count() > 0 {
            self.diag = diag;
        }
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.status.is_busy();
        let has_entries = self.entry_count(cx) > 0;
        let (current, total) = self.progress;
        let percent = if total > 0 {
            current as f32 / total as f32 * 100.
        } else {
            0.
        };

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
                    .when(busy && total > 0, |this| {
                        this.child(
                            ProgressBar::new("job-progress")
                                .value(percent)
                                .small()
                                .w_40(),
                        )
                    })
                    .when(busy, |this| {
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
                        Button::new("export-png")
                            .outline()
                            .label("导出 PNG")
                            .icon(IconName::GalleryVerticalEnd)
                            .disabled(!has_entries || busy)
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
                            .disabled(!has_entries || busy)
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
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(file_picker::render(self, window, cx))
            .child(output_config::render(self, window, cx))
            .child(data_preview::render(self, cx))
            .child(self.render_footer(cx))
    }
}

/// 自动重命名：若文件已存在，追加 (1) (2) … 直到找到可用名。
fn find_available_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("output");
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("pptx");
    let parent = path.parent().unwrap_or(Path::new("."));
    for n in 1..1000u32 {
        let candidate = parent.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // 极端情况：回退到带进程号的文件名
    parent.join(format!("{stem}_{}.{ext}", std::process::id()))
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

/// 模板模式导出 PNG 时的中间产物，渲染结束后删除。
///
/// 删除失败不中断导出，但必须留下可查询的诊断事件。
fn remove_generated_pptx(path: &Path, diag: &mut DiagStore) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            diag.warn(
                "export",
                &format!("无法删除中间文件 {}: {error}", path.display()),
                None,
            );
        }
    }
}
