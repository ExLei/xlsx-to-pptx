//! 层次状态机：**事件进、命令出**，纯逻辑、无窗口可测（见 `docs/design.md` §8）。
//!
//! 骨架已冻结：类型与签名是契约（exec 层与视图消费）；`reduce` 的落地实现与
//! 转移表测试见 `crates/gui/tests/state_test.rs`。
//!
//! 铁律（§8.4）：
//! - Busy 期只有取消有效，其余输入显式拒绝并提示（不排队、不静默丢弃）；
//! - 按钮守卫与事件守卫是同一个函数（键盘路径不得绕过）；
//! - 取消是合法终态：回到发起前 Screen + 提示，绝不进入 Failed；
//! - 探针是字段级子状态，可被新探针取代（过期结果按序号丢弃）；
//! - 同一时刻至多一个文档作业。

use std::path::{Path, PathBuf};

use x2p_core::domain::plan::{OutputPlan, OverwritePolicy};
use x2p_core::domain::schema::{CSV_ENCODINGS, Field, WordEntry};
use x2p_core::ops::{OpError, RendererChoice, SheetInfo};
use x2p_core::ports::Progress;

// ── 状态树 ────────────────────────────────────────────────────────────────

/// 应用状态（唯一真值；视图只读，exec 只按命令做事）。
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub screen: Screen,
    pub document: Document,
    pub renderer: RendererField,
    /// 最近一次进度（进度条数据源）。
    pub progress: Option<Progress>,
    /// 模板探针序号（过期结果丢弃）。
    pub template_probe: u64,
    /// 渲染器探针序号（过期结果丢弃）。
    pub renderer_probe: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// 无输入。
    Empty,
    /// 有词条，可生成/导出。
    Loaded,
    /// 单一文档作业在飞；唯一可用动作是取消。
    Busy {
        job: Job,
        spec: JobSpec,
        /// 发起前的位置（取消后回到这里）。
        prior: Prior,
        cancel_requested: bool,
    },
    /// 产物卡。
    Done {
        artifact: ArtifactView,
    },
    /// 失败，可重试（`retry = None` 表示不可重试）。
    Failed {
        error: String,
        retry: Option<RetryAction>,
    },
}

/// 取消后回到的 Screen。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prior {
    Empty,
    Loaded,
}

/// 文档作业（单槽串行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    Load,
    Generate,
    ExportPng,
}

/// 作业发起参数的快照（exec 层据此调用 ops；Retry 重放同一份）。
#[derive(Debug, Clone, PartialEq)]
pub struct JobSpec {
    pub source: Option<PathBuf>,
    pub sheet: Option<String>,
    pub encoding: String,
    pub template: Option<PathBuf>,
    pub output: OutputPlan,
    pub renderer: RendererChoice,
    /// Generate / ExportPng 的预览真值。
    pub entries: Vec<WordEntry>,
}

/// 可重放动作。
#[derive(Debug, Clone, PartialEq)]
pub struct RetryAction {
    pub job: Job,
    pub spec: JobSpec,
}

/// 文档（输入源与它的派生状态）。
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub source: Option<PathBuf>,
    pub sheet: Option<String>,
    pub sheets: Vec<SheetInfo>,
    pub encoding: String,
    pub entries: Vec<WordEntry>,
    pub template: TemplateField,
    /// 当前模板文件路径（`TemplateChanged` 写入；生成/导出的快照与模板探针取这里）。
    pub template_path: Option<PathBuf>,
    pub output: OutputField,
}

/// 模板字段级子状态（探针写入）。
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateField {
    Unset,
    Checking,
    Ready { slides: usize, fields: Vec<Field> },
    Invalid { reason: String },
}

/// 渲染器字段级子状态（探针写入）。
#[derive(Debug, Clone, PartialEq)]
pub enum RendererField {
    Probing,
    Available { name: String },
    Missing,
}

/// 输出字段级子状态。
#[derive(Debug, Clone, PartialEq)]
pub enum OutputField {
    Auto,
    Taken { path: PathBuf },
    Invalid { reason: String },
}

/// 产物视图（Done 卡）。
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactView {
    pub kind: ArtifactKind,
    pub path: PathBuf,
    pub slides: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Pptx,
    Png,
}

// ── 事件与命令 ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    PickInput(PathBuf),
    DropInput(PathBuf),
    InputCleared,
    SelectSheet(String),
    SelectEncoding(String),
    TemplateChanged(Option<PathBuf>),
    OutputChanged(PathBuf),
    Generate,
    ExportPng,
    Cancel,
    JobProgress(Progress),
    JobFinished { job: Job, result: JobResult },
    ProbeFinished { probe: Probe, seq: u64, result: ProbeResult },
    FileChanged { path: PathBuf },
    DismissError,
    Retry,
    OpenArtifact,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobResult {
    Loaded(LoadOutcome),
    Generated(ArtifactView),
    Exported(ArtifactView),
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadOutcome {
    pub sheets: Vec<SheetInfo>,
    pub sheet: Option<String>,
    pub entries: Vec<WordEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Template,
    Renderer,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProbeResult {
    Template { slides: usize, fields: Vec<Field>, missing_word: bool },
    TemplateInvalid { reason: String },
    Renderer(RendererField),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    SpawnJob(JobSpec),
    CancelJob,
    SpawnProbe { probe: Probe, seq: u64, template: Option<PathBuf> },
    OpenPath(PathBuf),
    Notify { level: NotifyLevel, text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    Info,
    Warn,
    Error,
}

// ── 接口 ──────────────────────────────────────────────────────────────────

impl AppState {
    /// 初始状态（Empty + 渲染器探测在飞）。
    pub fn new() -> AppState {
        AppState {
            screen: Screen::Empty,
            document: Document {
                source: None,
                sheet: None,
                sheets: Vec::new(),
                encoding: CSV_ENCODINGS[0].to_string(),
                entries: Vec::new(),
                template: TemplateField::Unset,
                template_path: None,
                output: OutputField::Auto,
            },
            renderer: RendererField::Probing,
            progress: None,
            template_probe: 0,
            renderer_probe: 0,
        }
    }
}

impl Default for AppState {
    fn default() -> AppState {
        AppState::new()
    }
}

/// 生成按钮与 Generate 事件共用的守卫（键盘路径不得绕过）。
pub fn can_generate(state: &AppState) -> bool {
    generate_refusal(state).is_none()
}

/// 导出按钮与 ExportPng 事件共用的守卫。
pub fn can_export_png(state: &AppState) -> bool {
    export_refusal(state).is_none()
}

/// 唯一的转移函数：事件进、命令出。
pub fn reduce(state: &mut AppState, event: Event) -> Vec<Command> {
    if is_busy(state) {
        reduce_busy(state, event)
    } else {
        reduce_ready(state, event)
    }
}

// ── 内部：守卫 ────────────────────────────────────────────────────────────

/// Busy 期统一拒绝文案（§8.4 #1：不排队、不静默丢弃）。
const BUSY_TEXT: &str = "忙碌中，请先取消或等待";

fn is_busy(state: &AppState) -> bool {
    matches!(state.screen, Screen::Busy { .. })
}

/// 通知：本层没有 diag 句柄，用户可见的拒绝与恢复只走 `Command::Notify`。
fn notify(level: NotifyLevel, text: impl Into<String>) -> Vec<Command> {
    vec![Command::Notify { level, text: text.into() }]
}

/// 拒绝原因（`None` = 守卫通过）；按钮与事件共用，键盘路径不得绕过。
fn generate_refusal(state: &AppState) -> Option<(NotifyLevel, String)> {
    if is_busy(state) {
        return Some((NotifyLevel::Warn, BUSY_TEXT.to_string()));
    }
    if state.document.entries.is_empty() {
        return Some((NotifyLevel::Warn, "尚无词条，无法生成——请先选择并加载词表".to_string()));
    }
    if let TemplateField::Invalid { reason } = &state.document.template {
        return Some((NotifyLevel::Error, reason.clone()));
    }
    if let OutputField::Invalid { reason } = &state.document.output {
        return Some((NotifyLevel::Error, reason.clone()));
    }
    None
}

fn export_refusal(state: &AppState) -> Option<(NotifyLevel, String)> {
    if is_busy(state) {
        return Some((NotifyLevel::Warn, BUSY_TEXT.to_string()));
    }
    if state.document.entries.is_empty() {
        return Some((NotifyLevel::Warn, "尚无词条，无法导出 PNG——请先选择并加载词表".to_string()));
    }
    if let TemplateField::Invalid { reason } = &state.document.template {
        return Some((NotifyLevel::Error, reason.clone()));
    }
    if let OutputField::Invalid { reason } = &state.document.output {
        return Some((NotifyLevel::Error, reason.clone()));
    }
    // 模板模式外呼渲染器；无模板走内置管线，不需要渲染器。
    if state.document.template_path.is_some() {
        match state.renderer {
            RendererField::Available { .. } => {}
            RendererField::Missing => {
                return Some((
                    NotifyLevel::Warn,
                    "未检测到可用的渲染器（Office / LibreOffice / WPS），模板模式下 PNG 导出不可用"
                        .to_string(),
                ))
            }
            RendererField::Probing => {
                return Some((NotifyLevel::Warn, "正在探测渲染器，请稍候再导出 PNG".to_string()))
            }
        }
    }
    None
}

// ── 内部：转移 ────────────────────────────────────────────────────────────

/// Busy 期：唯一可用动作是取消，其余输入显式拒绝（§8.4 #1/#7）。
fn reduce_busy(state: &mut AppState, event: Event) -> Vec<Command> {
    match event {
        Event::Cancel => {
            if let Screen::Busy { cancel_requested, .. } = &mut state.screen {
                *cancel_requested = true;
            }
            vec![Command::CancelJob]
        }
        Event::JobProgress(progress) => {
            state.progress = Some(progress);
            vec![]
        }
        Event::JobFinished { job, result } => finish_job(state, job, result),
        Event::ProbeFinished { probe, seq, result } => apply_probe(state, probe, seq, result),
        // 文档输入一律拒绝（不排队、不静默丢弃）
        Event::PickInput(_)
        | Event::DropInput(_)
        | Event::InputCleared
        | Event::SelectSheet(_)
        | Event::SelectEncoding(_)
        | Event::TemplateChanged(_)
        | Event::OutputChanged(_)
        | Event::FileChanged { .. }
        | Event::Generate
        | Event::ExportPng => notify(NotifyLevel::Warn, BUSY_TEXT),
        // 与当前屏幕无关的动作（视图在 Busy 不提供它们）：空操作
        Event::DismissError | Event::Retry | Event::OpenArtifact => vec![],
    }
}

/// 非 Busy：文档输入、动作守卫与作业收尾。
fn reduce_ready(state: &mut AppState, event: Event) -> Vec<Command> {
    match event {
        // 新文档：清派生状态、模板复位、输出回到自动派生
        Event::PickInput(path) | Event::DropInput(path) => {
            let prior = prior_of(&state.document);
            clear_document(&mut state.document);
            state.document.source = Some(path);
            state.template_probe += 1; // 在飞的模板探针作废
            start_load(state, prior)
        }
        Event::InputCleared => {
            let encoding = CSV_ENCODINGS[0].to_string();
            clear_document(&mut state.document);
            state.document.encoding = encoding;
            state.template_probe += 1;
            state.screen = Screen::Empty;
            state.progress = None;
            vec![]
        }
        Event::SelectSheet(sheet) => {
            if state.document.source.is_none() {
                return notify(NotifyLevel::Warn, "尚未选择输入文件，无法切换工作表");
            }
            state.document.sheet = Some(sheet);
            let prior = prior_of(&state.document);
            start_load(state, prior)
        }
        Event::SelectEncoding(encoding) => {
            if state.document.source.is_none() {
                return notify(NotifyLevel::Warn, "尚未选择输入文件，无法切换编码");
            }
            state.document.encoding = encoding;
            let prior = prior_of(&state.document);
            start_load(state, prior)
        }
        Event::TemplateChanged(None) => {
            state.document.template = TemplateField::Unset;
            state.document.template_path = None;
            state.template_probe += 1; // 作废在飞探针：结果不得写回已清空的字段
            vec![]
        }
        Event::TemplateChanged(Some(template)) => {
            state.document.template = TemplateField::Checking;
            state.document.template_path = Some(template.clone());
            state.template_probe += 1;
            let seq = state.template_probe;
            vec![Command::SpawnProbe { probe: Probe::Template, seq, template: Some(template) }]
        }
        Event::OutputChanged(path) => {
            state.document.output = match checked_output(&path) {
                Ok(field) => field,
                Err(reason) => OutputField::Invalid { reason },
            };
            vec![]
        }
        Event::Generate => {
            if let Some((level, text)) = generate_refusal(state) {
                return notify(level, text);
            }
            let spec = preview_spec(&state.document, OverwritePolicy::Suffix);
            enter_busy(state, Job::Generate, spec.clone(), Prior::Loaded);
            vec![Command::SpawnJob(spec)]
        }
        Event::ExportPng => {
            if let Some((level, text)) = export_refusal(state) {
                return notify(level, text);
            }
            let spec = preview_spec(&state.document, OverwritePolicy::Overwrite);
            enter_busy(state, Job::ExportPng, spec.clone(), Prior::Loaded);
            vec![Command::SpawnJob(spec)]
        }
        Event::Cancel => vec![], // 无在飞作业
        Event::FileChanged { path } => {
            if state.document.source.as_deref() != Some(path.as_path()) {
                return vec![]; // 别的文件（陈旧 watcher / 产物）：忽略
            }
            let prior = prior_of(&state.document);
            start_load(state, prior)
        }
        Event::DismissError => {
            if matches!(state.screen, Screen::Done { .. } | Screen::Failed { .. }) {
                state.screen = restored_screen(&state.document);
            }
            vec![]
        }
        Event::Retry => {
            let Screen::Failed { retry: Some(action), .. } = &state.screen else {
                return vec![]; // 无可重放动作
            };
            let action = action.clone();
            let prior = if state.document.source.is_some() { Prior::Loaded } else { Prior::Empty };
            enter_busy(state, action.job, action.spec.clone(), prior);
            vec![Command::SpawnJob(action.spec)]
        }
        Event::OpenArtifact => match &state.screen {
            Screen::Done { artifact } => vec![Command::OpenPath(artifact.path.clone())],
            _ => vec![], // 只有 Done 才有产物
        },
        // 非 Busy 的作业信号：陈旧，丢弃
        Event::JobProgress(_) | Event::JobFinished { .. } => vec![],
        Event::ProbeFinished { probe, seq, result } => apply_probe(state, probe, seq, result),
    }
}

// ── 内部：转移助手 ────────────────────────────────────────────────────────

/// 清空文档的源与派生状态（编码是用户选择，由调用方单独决定）。
fn clear_document(document: &mut Document) {
    document.source = None;
    document.sheet = None;
    document.sheets.clear();
    document.entries.clear();
    document.template = TemplateField::Unset;
    document.template_path = None;
    document.output = OutputField::Auto;
}

/// 发起前的位置（取消后回到这里）：有词条 → Loaded，否则 Empty。
fn prior_of(document: &Document) -> Prior {
    if document.entries.is_empty() {
        Prior::Empty
    } else {
        Prior::Loaded
    }
}

/// 位置 → Screen（取消后回到发起前的位置）。
fn restored_screen_at(prior: Prior) -> Screen {
    match prior {
        Prior::Loaded => Screen::Loaded,
        Prior::Empty => Screen::Empty,
    }
}

/// 返回 Loaded/Failed 之后的位置（词条仍在 → Loaded）。
fn restored_screen(document: &Document) -> Screen {
    restored_screen_at(prior_of(document))
}

/// 进入 Busy：单作业不变式（I7）用 debug 断言看住；进度归零。
fn enter_busy(state: &mut AppState, job: Job, spec: JobSpec, prior: Prior) {
    debug_assert!(!is_busy(state), "单作业不变式（I7）：上一个作业未结束不得发起新作业");
    state.progress = None;
    state.screen = Screen::Busy { job, spec, prior, cancel_requested: false };
}

/// 派发 Load（PickInput / SelectSheet / SelectEncoding / FileChanged 共用）。
fn start_load(state: &mut AppState, prior: Prior) -> Vec<Command> {
    let spec = JobSpec {
        source: state.document.source.clone(),
        sheet: state.document.sheet.clone(),
        encoding: state.document.encoding.clone(),
        template: None, // Load 不用模板
        output: output_plan(&state.document, OverwritePolicy::Suffix),
        renderer: RendererChoice::Auto,
        entries: Vec::new(), // 预览真值属于生成/导出作业
    };
    enter_busy(state, Job::Load, spec.clone(), prior);
    vec![Command::SpawnJob(spec)]
}

/// Generate / ExportPng 的参数快照（词条是值拷贝，与后续状态解耦）。
fn preview_spec(document: &Document, policy: OverwritePolicy) -> JobSpec {
    JobSpec {
        source: document.source.clone(),
        sheet: document.sheet.clone(),
        encoding: document.encoding.clone(),
        template: document.template_path.clone(),
        output: output_plan(document, policy),
        renderer: RendererChoice::Auto, // GUI 不做渲染器选择（对等 auto）
        entries: document.entries.clone(),
    }
}

/// 输出计划：显式路径直接用它；`Auto` 派生自输入（同目录、同名 .pptx）。
fn output_plan(document: &Document, policy: OverwritePolicy) -> OutputPlan {
    let path = match &document.output {
        OutputField::Taken { path } => path.clone(),
        OutputField::Auto | OutputField::Invalid { .. } => match &document.source {
            Some(source) => source.with_extension("pptx"),
            None => PathBuf::from("output.pptx"),
        },
    };
    plan_for(&path, policy)
}

/// 路径 → 计划（父目录为空落当前目录；与 CLI 的 `file_plan` 同一规则）。
fn plan_for(path: &Path, policy: OverwritePolicy) -> OutputPlan {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "output.pptx".to_string(),
    };
    OutputPlan::new(dir, name, policy)
}

/// 输出路径校验（合法即 `Taken`；最终落点由 `OutputPlan` 的策略解析）。
fn checked_output(path: &Path) -> Result<OutputField, String> {
    if path.as_os_str().is_empty() {
        return Err("输出路径不能为空".to_string());
    }
    let is_pptx = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pptx"));
    if !is_pptx {
        return Err("输出需要 .pptx 扩展名".to_string());
    }
    Ok(OutputField::Taken { path: path.to_path_buf() })
}

/// 作业收尾：只在 Busy 且作业/结果对得上时生效，其余是陈旧或错配信号。
fn finish_job(state: &mut AppState, job: Job, result: JobResult) -> Vec<Command> {
    let Screen::Busy { job: current, spec, prior, .. } = &state.screen else {
        return vec![];
    };
    if *current != job || !result_matches(job, &result) {
        return vec![];
    }
    let (spec, prior) = (spec.clone(), *prior);
    state.progress = None; // 离开 Busy
    match result {
        // 取消是合法终态：回到发起前的位置 + 提示，绝不进入 Failed（§8.4 #3）
        JobResult::Cancelled => {
            state.screen = restored_screen_at(prior);
            notify(NotifyLevel::Info, "已取消")
        }
        JobResult::Loaded(outcome) => {
            state.document.source = spec.source.clone();
            state.document.sheet = outcome.sheet;
            state.document.sheets = outcome.sheets;
            state.document.entries = outcome.entries;
            state.screen = Screen::Loaded;
            vec![]
        }
        JobResult::Generated(artifact) => done(state, ArtifactKind::Pptx, artifact),
        JobResult::Exported(artifact) => done(state, ArtifactKind::Png, artifact),
        // 失败：保留重放语义（同一份 spec）
        JobResult::Failed(error) => {
            state.screen = Screen::Failed { error, retry: Some(RetryAction { job, spec }) };
            vec![]
        }
    }
}

/// 结果与作业种类是否匹配（不匹配的信号一律丢弃）。
fn result_matches(job: Job, result: &JobResult) -> bool {
    matches!(
        (job, result),
        (Job::Load, JobResult::Loaded(_))
            | (Job::Generate, JobResult::Generated(_))
            | (Job::ExportPng, JobResult::Exported(_))
            | (_, JobResult::Cancelled)
            | (_, JobResult::Failed(_))
    )
}

/// 产物卡：作业种类是 kind 的权威（Generated → Pptx / Exported → Png）。
fn done(state: &mut AppState, kind: ArtifactKind, artifact: ArtifactView) -> Vec<Command> {
    state.screen = Screen::Done { artifact: ArtifactView { kind, ..artifact } };
    vec![]
}

/// 探针结果落地：序号对不上就丢弃（探针可被新探针取代，不占作业槽）。
fn apply_probe(state: &mut AppState, probe: Probe, seq: u64, result: ProbeResult) -> Vec<Command> {
    match probe {
        Probe::Template => {
            if seq != state.template_probe {
                return vec![];
            }
            state.document.template = match result {
                ProbeResult::Template { missing_word: true, .. } => TemplateField::Invalid {
                    reason: "模板缺少必填占位符 {{单词}}，无法生成".to_string(),
                },
                ProbeResult::Template { slides, fields, .. } => {
                    TemplateField::Ready { slides, fields }
                }
                ProbeResult::TemplateInvalid { reason } => TemplateField::Invalid { reason },
                ProbeResult::Renderer(_) => return vec![], // 载荷与探针不符
            };
        }
        Probe::Renderer => {
            if seq != state.renderer_probe {
                return vec![];
            }
            match result {
                ProbeResult::Renderer(field) => state.renderer = field,
                _ => return vec![],
            }
        }
    }
    vec![]
}

/// 失败文案：附加渲染器安装指引（与 CLI 共用 `OpError::renderer_help`）。
pub fn render_error(error: &OpError) -> String {
    match error.renderer_help() {
        Some(help) => format!("{error}\n\n{help}"),
        None => error.to_string(),
    }
}
