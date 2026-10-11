//! 命令执行层：状态机的 [`Command`] 在这里落地，后台结果再以 [`Event`] 回流。
//!
//! 分工（`docs/design.md` §8.1/§8.5）：
//! - **本层不改状态**：只按命令做事（spawn 作业/探针、取消、打开路径、通知），后台结果
//!   一律翻译成 `Event` 交回 `state::reduce`——状态的唯一写入口始终是状态机。
//! - **会话装配**：每会话一份 NDJSON 日志（`diag::start_session` + `record_session`，
//!   字段与 CLI 相同）；状态栏的摘要走同一查询引擎（`diag::query`）。
//! - **视图侧节拍**：进度采样 ~100ms，输入文件 watcher 与摘要刷新 ~1s。
//! - **零静默失败**（I2）：可失败操作一律 `diag` 留痕，用户可见时再补一条窗口通知。
//!
//! 作业与探针都在后台执行（`cx.background_spawn`），只有事件的回流在主线。

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::json;

use x2p_core::diag::{self, field, query, Diag, Target};
use x2p_core::domain::plan::{Artifact, OutputPlan, OverwritePolicy};
use x2p_core::domain::schema::{Source, SourceKind};
use x2p_core::ops::{self, Finished, OpContext, OpObserver, OpResult, Progress};
use x2p_core::paths::Layout;
use x2p_core::platform::png;

use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, WindowExt as _};
use gpui_kit::{AppContext as _, Context, Task, Window, WindowAppearance};

use crate::state::{
    self, ArtifactKind, ArtifactView, Command, Event, Job, JobResult, JobSpec, LoadOutcome,
    NotifyLevel, Probe, ProbeResult, RendererField, Screen,
};
use crate::views::VocabApp;

/// 视图节拍：进度采样间隔（只影响观感）。
const TICK: Duration = Duration::from_millis(100);
/// 每 N 拍做一次慢活（watcher + 摘要刷新）≈ 1s。
const SLOW_TICK_EVERY: u32 = 10;

// ── 会话与共享单元 ────────────────────────────────────────────────────────

/// 取消旗标：`CancelJob` 置位，观察者在下一个检查点返回 `Break`。
type CancelFlag = Arc<AtomicBool>;
/// 进度共享单元：观察者写、视图定时读（只在变化时进状态机）。
type ProgressCell = Arc<Mutex<Option<Progress>>>;

/// 状态栏的诊断摘要（`diag::query` 的 `Summary` 投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagView {
    /// 尚未刷新（首帧）。
    Pending,
    /// 会话日志不可读（原因可见，不静默）。
    Unavailable(String),
    /// 事件计数与坏行数。
    Summary {
        info: usize,
        warn: usize,
        error: usize,
        bad_lines: usize,
    },
}

impl DiagView {
    /// WARN + ERROR 计数（状态栏的"是否值得看一眼"判据）。
    pub fn issues(&self) -> usize {
        match self {
            DiagView::Summary { warn, error, .. } => warn + error,
            _ => 0,
        }
    }

    /// 全部事件数。
    pub fn total(&self) -> usize {
        match self {
            DiagView::Summary { info, warn, error, .. } => info + warn + error,
            _ => 0,
        }
    }
}

/// 会话级执行环境：诊断日志、数据目录、取消与进度。
///
/// 可克隆：后台任务拿一份，用完之后它的 diag 句柄仍写进同一份会话日志。
#[derive(Clone)]
pub struct Exec {
    diag: Diag,
    layout: Layout,
    log: Option<PathBuf>,
    log_note: Option<String>,
    cancel: CancelFlag,
    progress: ProgressCell,
    summary: DiagView,
}

impl Exec {
    /// 打开一个 GUI 会话：`<数据目录>/日志/gui_<时间戳>.log`，首行 session 事件
    /// （字段与 CLI 相同：版本 / 命令 / 参数 / 数据目录 / 回退标志 / 可执行文件 / 系统）。
    ///
    /// 日志建不起来**不阻塞运行**（`start_session` 已记一条 ERROR）：这里把原因留给状态栏。
    pub fn new(layout: Layout) -> Exec {
        let (diag, log) = diag::start_session(&layout.logs_dir(), "gui", true);
        diag.record_session(json!({
            field::VERSION: env!("CARGO_PKG_VERSION"),
            field::COMMAND: "gui",
            field::ARGS: std::env::args().collect::<Vec<String>>(),
            field::DATA_DIR: layout.root().display().to_string(),
            field::FALLBACK: layout.is_fallback(),
            field::EXE_PATH: std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            field::OS: std::env::consts::OS,
        }));
        let log_note = log
            .is_none()
            .then(|| format!("日志不可用（{}）", layout.logs_dir().display()));
        let mut exec = Exec {
            diag,
            layout,
            log,
            log_note,
            cancel: CancelFlag::default(),
            progress: ProgressCell::default(),
            summary: DiagView::Pending,
        };
        exec.refresh_summary();
        exec
    }

    /// 诊断句柄（后台任务与用例层共用同一个）。
    pub fn diag(&self) -> &Diag {
        &self.diag
    }

    /// 数据目录布局（默认落点与日志位置）。
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// 本会话的日志文件（`None` = 建不起来，原因见 [`Exec::log_note`]）。
    pub fn log_path(&self) -> Option<&Path> {
        self.log.as_deref()
    }

    /// 会话日志建立失败时的提示（状态栏可见）。
    pub fn log_note(&self) -> Option<&str> {
        self.log_note.as_deref()
    }

    /// 当前摘要缓存。
    pub fn summary(&self) -> &DiagView {
        &self.summary
    }

    /// 用查询引擎重算摘要——与 CLI `diag --summary` 是同一份实现。
    pub fn refresh_summary(&mut self) {
        let Some(log) = self.log.clone() else {
            self.summary = DiagView::Unavailable(
                self.log_note
                    .clone()
                    .unwrap_or_else(|| "日志不可用".to_string()),
            );
            return;
        };
        match query::run(&log, &query::Query::Summary) {
            // 键名是 `query::Report::to_json` 的契约（见 `diag/query.rs` 文档）。
            Ok(report) => {
                let object = report.to_json();
                let counts = &object["counts"];
                self.summary = DiagView::Summary {
                    info: count_of(counts, "info"),
                    warn: count_of(counts, "warn"),
                    error: count_of(counts, "error"),
                    bad_lines: object["bad_lines"].as_u64().unwrap_or(0) as usize,
                };
            }
            Err(error) => self.summary = DiagView::Unavailable(error.to_string()),
        }
    }

    /// 当前进度（`None` = 无作业或作业尚未报进度）。
    pub fn progress(&self) -> Option<Progress> {
        match self.progress.lock() {
            Ok(cell) => cell.clone(),
            // 锁中毒只可能来自别的线程 panic；取回内部值继续（不 panic、不丢数据）。
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 新作业开始：清进度、复位取消旗标（上一次的取消不得穿透到下一次）。
    fn begin_job(&self) {
        store_progress(&self.progress, None);
        self.cancel.store(false, Ordering::Relaxed);
    }

    /// 用户请求取消：旗标置位，观察者在下一个检查点让用例返回 `Finished::Cancelled`。
    fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// `counts.<key>` 取计数（缺失按 0；结构由查询引擎契约保证）。
fn count_of(counts: &serde_json::Value, key: &str) -> usize {
    counts.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0) as usize
}

/// 进度落盘到共享单元（锁中毒时取回内部值继续写，不 panic、不丢更新）。
fn store_progress(cell: &ProgressCell, value: Option<Progress>) {
    let mut guard = cell
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = value;
}

/// 作业观察者：进度进共享单元，取消让用例在下一个检查点返回 `Cancelled`（I3）。
struct JobObserver {
    progress: ProgressCell,
    cancel: CancelFlag,
}

impl OpObserver for JobObserver {
    fn on(&mut self, progress: &Progress) -> ControlFlow<()> {
        store_progress(&self.progress, Some(progress.clone()));
        if self.cancel.load(Ordering::Relaxed) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

// ── 后台：作业与探针（纯逻辑，不碰 gpui）──────────────────────────────────

/// 跑一个作业，返回状态机要的结果。
///
/// 错误一律经 [`state::render_error`] 渲染（渲染器缺失时附安装指引，与 CLI 共用文案）。
fn run_job(job: Job, spec: &JobSpec, exec: &Exec) -> JobResult {
    let mut observer = JobObserver {
        progress: exec.progress.clone(),
        cancel: exec.cancel.clone(),
    };
    let mut cx = OpContext {
        diag: &exec.diag,
        observer: &mut observer,
    };
    match job {
        Job::Load => load_job(spec, &mut cx),
        Job::Generate => generate_job(spec, &mut cx),
        Job::ExportPng => export_job(spec, &mut cx),
    }
}

/// 读取作业：Excel 先列工作表（下拉数据源），再按所选表读词条；CSV 走编码。
fn load_job(spec: &JobSpec, cx: &mut OpContext) -> JobResult {
    let Some(path) = spec.source.clone() else {
        // 状态机不会在没有源时发起读取；真到了这里就明说，不静默失败。
        cx.diag.error(Target::Gui, "读取作业缺少输入文件", None);
        return JobResult::Failed("读取作业缺少输入文件".to_string());
    };
    let is_excel = !is_csv(&path);
    let sheets = if is_excel {
        match ops::list_sheets(&path, cx) {
            Ok(Finished::Completed(sheets)) => sheets,
            Ok(Finished::Cancelled) => return JobResult::Cancelled,
            Err(error) => return JobResult::Failed(state::render_error(&error)),
        }
    } else {
        Vec::new()
    };
    let sheet = if is_excel {
        // 请求的表若已不存在（工作簿被改过），退回第一张——与平台层的取表规则一致。
        let wanted = spec
            .sheet
            .clone()
            .filter(|name| sheets.iter().any(|sheet| &sheet.name == name));
        match wanted.or_else(|| sheets.first().map(|sheet| sheet.name.clone())) {
            Some(name) => Some(name),
            None => return JobResult::Failed("工作簿没有任何工作表".to_string()),
        }
    } else {
        None
    };
    let kind = if is_excel {
        SourceKind::Excel { sheet: sheet.clone() }
    } else {
        SourceKind::Csv {
            encoding: spec.encoding.clone(),
        }
    };
    let source = Source { path, kind };
    match ops::load_entries(&source, cx) {
        Ok(Finished::Completed(summary)) => JobResult::Loaded(LoadOutcome {
            sheets,
            sheet,
            entries: summary.entries,
        }),
        Ok(Finished::Cancelled) => JobResult::Cancelled,
        Err(error) => JobResult::Failed(state::render_error(&error)),
    }
}

/// 生成作业：模板可选（`None` → 内嵌版式）。
fn generate_job(spec: &JobSpec, cx: &mut OpContext) -> JobResult {
    match ops::generate_deck(
        &spec.entries,
        spec.template.as_deref(),
        spec.output.clone(),
        cx,
    ) {
        Ok(Finished::Completed(artifact)) => JobResult::Generated(artifact_view(ArtifactKind::Pptx, artifact)),
        Ok(Finished::Cancelled) => JobResult::Cancelled,
        Err(error) => JobResult::Failed(state::render_error(&error)),
    }
}

/// 导出作业：落点是计划里的目录（`slide_N.png`）。模板模式外呼渲染器，无模板走内置管线。
fn export_job(spec: &JobSpec, cx: &mut OpContext) -> JobResult {
    let dir = spec.output.dir.clone();
    match ops::export_png(
        &spec.entries,
        spec.template.as_deref(),
        spec.renderer,
        &dir,
        cx,
    ) {
        Ok(Finished::Completed(artifact)) => JobResult::Exported(ArtifactView {
            kind: ArtifactKind::Png,
            path: artifact.dir,
            slides: artifact.slides.len(),
        }),
        Ok(Finished::Cancelled) => JobResult::Cancelled,
        Err(error) => JobResult::Failed(state::render_error(&error)),
    }
}

fn artifact_view(kind: ArtifactKind, artifact: Artifact) -> ArtifactView {
    ArtifactView {
        kind,
        path: artifact.path,
        slides: artifact.slides,
    }
}

/// 跑一个探针；返回 `None` 表示这次结果作废（不发给状态机）。
fn run_probe(
    probe: Probe,
    seq: u64,
    template: Option<&Path>,
    exec: &Exec,
) -> Option<(Probe, u64, ProbeResult)> {
    let result = match probe {
        Probe::Template => {
            let Some(path) = template else {
                // 模板探针必须带路径；没有就是状态机缺陷，明说不静默。
                exec.diag
                    .error(Target::Gui, "模板探针缺少模板路径", None);
                return None;
            };
            probe_template(path, exec)?
        }
        Probe::Renderer => probe_renderer(exec),
    };
    Some((probe, seq, result))
}

/// 模板校验（`ops::check_template`）→ 字段级子状态载荷。
///
/// 只在路径确实是一个文件时才进用例层：探针每次改动都会派发，逐字符跑一遍会把
/// 「不存在」「是目录」这些打字中间态记成假 ERROR（旧版也先判存在性再扫）。
/// 理由照样回到界面，不静默。
fn probe_template(path: &Path, exec: &Exec) -> Option<ProbeResult> {
    if !path.is_file() {
        exec.diag.info(
            Target::Template,
            "模板路径还不是文件（等待输入完成）",
            Some(json!({ field::PATH: path.display().to_string() })),
        );
        let reason = if path.exists() {
            format!("模板不是文件：{}", path.display())
        } else {
            format!("模板文件不存在：{}", path.display())
        };
        return Some(ProbeResult::TemplateInvalid { reason });
    }
    // 探针可被新探针取代（序号判废），不需要取消通道。
    let mut observer = ops::NoopObserver;
    let mut cx = OpContext {
        diag: &exec.diag,
        observer: &mut observer,
    };
    match ops::check_template(path, &mut cx) {
        Ok(Finished::Completed(check)) => Some(ProbeResult::Template {
            slides: check.slides,
            fields: check.fields,
            missing_word: check.missing_word,
        }),
        // 观察者恒 Continue：取消不会发生；真收到就当结果作废。
        Ok(Finished::Cancelled) => None,
        Err(error) => Some(ProbeResult::TemplateInvalid {
            reason: state::render_error(&error),
        }),
    }
}

/// 渲染器探测（`Auto` = PowerPoint → LibreOffice → WPS）。
///
/// 一个都没有**不是错误**：模板模式的 PNG 导出据此被拦下并给出安装指引（`renderer_help`）。
fn probe_renderer(exec: &Exec) -> ProbeResult {
    match png::select(png::RendererChoice::Auto, &exec.diag) {
        Ok(renderer) => ProbeResult::Renderer(RendererField::Available {
            name: renderer.name().to_string(),
        }),
        Err(png::RenderError::NoRenderer) => ProbeResult::Renderer(RendererField::Missing),
        Err(error) => {
            exec.diag
                .warn(Target::Gui, format!("渲染器探测失败: {error}"), None);
            ProbeResult::Renderer(RendererField::Missing)
        }
    }
}

/// 输入格式由扩展名决定（与 CLI 的 `source_from` 同一规则）：csv → CSV，其余按 Excel。
pub(crate) fn is_csv(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
}

/// 拖放只接受这三种扩展名。
fn is_supported_input(path: &Path) -> bool {
    ["xlsx", "xls", "csv"].iter().any(|candidate| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(candidate))
    })
}

/// 路径 → 输出计划（**视图侧动作专用**：示例模板 / 词表模板的固定落点）。
///
/// 与状态机的 `plan_for` 同一规则（父目录为空落当前目录）；作业用的计划一律由状态机
/// 写进 `JobSpec`，这里不参与。
fn file_plan(path: &Path, policy: OverwritePolicy) -> OutputPlan {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    OutputPlan::new(dir, name, policy)
}

/// 文件修改时间（取不到就是 `None`：不存在、无权限、或平台不支持）。
fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|meta| meta.modified().ok())
}

/// 窗口标题：应用名 + 当前输入文件名（没有输入时只有应用名）。
pub(crate) fn window_title(source: Option<&Path>) -> String {
    match source.and_then(Path::file_name).and_then(|name| name.to_str()) {
        Some(name) => format!("{} — {name}", crate::APP_TITLE),
        None => crate::APP_TITLE.to_string(),
    }
}

/// 后台结果回流：视图已被释放（窗口关闭）是正常终态，不是失败。
///
/// 实体释放后没有任何可写的状态、也没有可展示的通知——此刻唯一正确的动作就是结束。
/// 用泛型是因为 gpui 的错误类型不在本 crate 的依赖里（`anyhow::Error`）。
fn view_released<T, E>(_result: Result<T, E>) {}

// ── 命令执行 ──────────────────────────────────────────────────────────────

impl VocabApp {
    /// 唯一的入口：事件进、命令出、副作用落地，随后刷新受控控件的投影。
    pub(crate) fn dispatch(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let commands = state::reduce(&mut self.state, event);
        for command in commands {
            self.execute(command, window, cx);
        }
        self.sync_views(window, cx);
        cx.notify();
    }

    /// 命令 → 副作用（spawn 作业/探针、置取消旗标、打开路径、通知）。
    fn execute(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::SpawnJob(spec) => self.spawn_job(spec, window, cx),
            Command::CancelJob => self.exec.request_cancel(),
            Command::SpawnProbe {
                probe,
                seq,
                template,
            } => self.spawn_probe(probe, seq, template, window, cx),
            Command::OpenPath(path) => self.open_path(&path, window, cx),
            Command::Notify { level, text } => self.notify(level, text, window, cx),
        }
    }

    /// 作业：后台执行，完成后把 `JobFinished` 送回状态机（单作业，I7）。
    fn spawn_job(&mut self, spec: JobSpec, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Busy { job, .. } = &self.state.screen else {
            // 单作业不变式：SpawnJob 只在进入 Busy 时发出；真到了这里明说不静默。
            self.exec.diag.error(
                Target::Gui,
                "SpawnJob 到达时状态机不在 Busy（单作业不变式）",
                None,
            );
            self.notify(
                NotifyLevel::Error,
                "内部错误：作业在非忙碌状态被派发".to_string(),
                window,
                cx,
            );
            return;
        };
        let job = *job;
        self.exec.begin_job();
        self.shown_progress = None;

        let exec = self.exec.clone();
        let spec_worker = spec.clone();
        let worker = cx.background_spawn(async move { run_job(job, &spec_worker, &exec) });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = worker.await;
            view_released(this.update_in(cx, |this, window, cx| {
                this.finish_job(job, result, window, cx)
            }));
        });
        // 任务句柄只为保活；取消走观察者旗标，不靠丢弃任务。
        self.job = Some(task);
    }

    /// 作业收尾：结果进状态机，只有被接受的结果才产生用户可见的通知。
    fn finish_job(&mut self, job: Job, result: JobResult, window: &mut Window, cx: &mut Context<Self>) {
        self.job = None;
        self.shown_progress = None;
        // 状态机按（作业, 结果）对匹配：陈旧或错配的信号不会落地。
        let accepted = matches!(&self.state.screen, Screen::Busy { job: current, .. } if *current == job);
        self.dispatch(Event::JobFinished { job, result: result.clone() }, window, cx);
        if accepted {
            self.notify_job(&result, window, cx);
        }
        // 作业留下的诊断事件（读取/生成/渲染各阶段）立即进摘要。
        self.exec.refresh_summary();
    }

    /// 作业结果的用户可见后果（状态机只管屏幕，通知归属本层）。
    ///
    /// 产物落地用 Success（对等旧版），失败用 Error（文案含渲染器安装指引），
    /// 取消不在这里提示——状态机已经发了「已取消」。
    fn notify_job(&mut self, result: &JobResult, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            JobResult::Generated(artifact) => self.success(
                format!(
                    "已生成 {} 张幻灯片 → {}",
                    artifact.slides,
                    artifact.path.display()
                ),
                window,
                cx,
            ),
            JobResult::Exported(artifact) => self.success(
                format!(
                    "已导出 {} 张 PNG → {}",
                    artifact.slides,
                    artifact.path.display()
                ),
                window,
                cx,
            ),
            JobResult::Loaded(outcome) => {
                // 读取成功不用 toast（预览区已经显示条数）；进日志即可。
                self.exec.diag.info(
                    Target::Gui,
                    "词条已加载",
                    Some(json!({ field::ENTRIES: outcome.entries.len() })),
                );
            }
            // 失败文案已经过 `state::render_error`（含渲染器安装指引）。
            JobResult::Failed(error) => {
                self.notify(NotifyLevel::Error, error.clone(), window, cx)
            }
            // 取消由状态机提示（「已取消」），这里不重复。
            JobResult::Cancelled => {}
        }
    }

    /// 探针：后台执行，结果按序号回流（过期结果由状态机丢弃）。
    fn spawn_probe(
        &mut self,
        probe: Probe,
        seq: u64,
        template: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exec = self.exec.clone();
        let worker = cx.background_spawn(async move {
            run_probe(probe, seq, template.as_deref(), &exec)
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let Some((probe, seq, result)) = worker.await else {
                return; // 结果作废（缺路径 / 被取消）
            };
            view_released(this.update_in(cx, |this, window, cx| {
                this.dispatch(Event::ProbeFinished { probe, seq, result }, window, cx)
            }));
        });
        // 探针不占作业槽，也不提供取消入口：让它自己跑完即可。
        task.detach();
    }

    /// 启动探针：`AppState::new` 只把渲染器置为 `Probing`，真正的探测由这里发起。
    pub(crate) fn start_renderer_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.state.renderer_probe;
        self.spawn_probe(Probe::Renderer, seq, None, window, cx);
    }

    /// 通知：`Command::Notify` 的落地（先留痕再弹窗，两条通道同一句文案）。
    pub(crate) fn notify(
        &mut self,
        level: NotifyLevel,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let diag = self.exec.diag();
        match level {
            NotifyLevel::Info => diag.info(Target::Gui, text.clone(), None),
            NotifyLevel::Warn => diag.warn(Target::Gui, text.clone(), None),
            NotifyLevel::Error => diag.error(Target::Gui, text.clone(), None),
        }
        self.toast(notification_type(level), text, window, cx);
    }

    /// 产物落地：进日志 + 成功色通知。
    fn success(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.exec.diag.info(Target::Gui, text.clone(), None);
        self.toast(NotificationType::Success, text, window, cx);
    }

    /// 只弹窗（文案已经进过日志的调用点用这个）。
    pub(crate) fn toast(
        &self,
        kind: NotificationType,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.push_notification((kind, text), cx);
    }

    /// 用系统默认程序打开路径；失败时留痕 + 给出手动路径。
    pub(crate) fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = open::that(path) {
            self.exec.diag.error(
                Target::Gui,
                format!("打开失败 {}: {error}", path.display()),
                Some(json!({ field::PATH: path.display().to_string() })),
            );
            self.toast(
                NotificationType::Error,
                format!("无法打开，请手动访问：{}", path.display()),
                window,
                cx,
            );
        }
    }

    /// 打开数据目录（状态栏入口）：目录不在就建，建不出来明说。
    pub(crate) fn open_data_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.exec.layout().root().to_path_buf();
        if let Err(error) = std::fs::create_dir_all(&root) {
            self.exec.diag.error(
                Target::Gui,
                format!("无法创建数据目录: {error}"),
                Some(json!({ field::DIR: root.display().to_string() })),
            );
            self.toast(
                NotificationType::Error,
                format!("无法创建数据目录：{error}"),
                window,
                cx,
            );
            return;
        }
        self.open_path(&root, window, cx);
    }

    /// 打开本会话日志（状态栏入口）；日志建不起来时退到日志目录。
    pub(crate) fn open_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = match self.exec.log_path() {
            Some(log) => log.to_path_buf(),
            None => self.exec.layout().logs_dir(),
        };
        if path.is_dir() {
            self.open_data_dir(window, cx);
            return;
        }
        self.open_path(&path, window, cx);
    }

    /// 明暗切换（视图侧：主题不进状态机），同时切原生窗口边框，避免界面与标题栏不同侧。
    pub(crate) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = cx.theme().is_dark();
        Theme::change(
            if dark { ThemeMode::Light } else { ThemeMode::Dark },
            Some(window),
            cx,
        );
        cx.set_window_appearance(Some(if dark {
            WindowAppearance::Light
        } else {
            WindowAppearance::Dark
        }));
        cx.notify();
    }

    /// 生成示例 PPTX 模板并打开（视图侧动作：状态机没有对应事件，产物落地仍走用例层）。
    pub(crate) fn open_example_pptx(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.exec.layout().template_pptx();
        let hint = "示例模板已打开——编辑保存后在「PPTX 模板」里选择它".to_string();
        if path.exists() {
            self.open_path(&path, window, cx);
            self.toast(NotificationType::Info, hint, window, cx);
            return;
        }
        let plan = file_plan(&path, OverwritePolicy::Overwrite);
        self.spawn_template_build(ops::make_example_template, plan, Some(hint), window, cx);
    }

    /// 生成 Excel 词表模板并打开（用户已经改过的模板不覆盖，直接打开）。
    pub(crate) fn open_excel_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.exec.layout().template_xlsx();
        if path.exists() {
            self.open_path(&path, window, cx);
            return;
        }
        let plan = file_plan(&path, OverwritePolicy::Overwrite);
        self.spawn_template_build(
            ops::make_excel_template,
            plan,
            Some("词表模板已打开——填好后在「输入文件」里选择它".to_string()),
            window,
            cx,
        );
    }

    /// 模板生成的共用流程：后台跑用例 → 成功打开 + 提示；失败通知（用例层已记 diag）。
    fn spawn_template_build(
        &mut self,
        op: fn(OutputPlan, &mut OpContext) -> OpResult<Artifact>,
        plan: OutputPlan,
        hint: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let exec = self.exec.clone();
        let worker = cx.background_spawn(async move {
            let mut observer = ops::NoopObserver;
            let mut context = OpContext {
                diag: &exec.diag,
                observer: &mut observer,
            };
            op(plan, &mut context)
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            match worker.await {
                Ok(Finished::Completed(artifact)) => {
                    view_released(this.update_in(cx, |this, window, cx| {
                        let path = artifact.path.clone();
                        this.open_path(&path, window, cx);
                        if let Some(hint) = hint {
                            this.toast(NotificationType::Info, hint, window, cx);
                        }
                    }));
                }
                // 观察者恒 Continue：模板生成不可取消，真收到就是没产物，明说一句。
                Ok(Finished::Cancelled) => {
                    view_released(this.update_in(cx, |this, window, cx| {
                        this.notify(
                            NotifyLevel::Info,
                            "模板生成已取消（没有产物）".to_string(),
                            window,
                            cx,
                        );
                    }));
                }
                Err(error) => {
                    let text = state::render_error(&error);
                    view_released(this.update_in(cx, |this, window, cx| {
                        this.notify(NotifyLevel::Error, text, window, cx)
                    }));
                }
            }
        });
        task.detach();
    }

    /// 拖入窗口的文件：第一个 .xlsx / .xls / .csv 进状态机，其余给出可见提示。
    pub(crate) fn handle_drop(
        &mut self,
        paths: &gpui_kit::ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = paths.paths().first().cloned() else {
            return; // 空拖放：没有可做的事
        };
        if !is_supported_input(&path) {
            self.exec.diag.warn(
                Target::Gui,
                format!("拖入的路径不是 Excel/CSV 文件: {}", path.display()),
                Some(json!({ field::PATH: path.display().to_string() })),
            );
            self.toast(
                NotificationType::Error,
                "请拖入单个 .xlsx / .xls / .csv 文件".to_string(),
                window,
                cx,
            );
            return;
        }
        self.dispatch(Event::DropInput(path), window, cx);
    }

    // ── 视图侧节拍：进度采样 / watcher / 摘要 ──

    /// 挂上节拍任务；窗口关闭（视图释放）时循环自行退出。
    pub(crate) fn spawn_ticker(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            let mut ticks: u32 = 0;
            loop {
                cx.background_executor().timer(TICK).await;
                ticks = ticks.wrapping_add(1);
                let slow = ticks % SLOW_TICK_EVERY == 0;
                if this
                    .update_in(cx, |this, window, cx| this.on_tick(slow, window, cx))
                    .is_err()
                {
                    break; // 视图已释放
                }
            }
        })
    }

    /// 一拍：进度采样（快）；watcher 与摘要刷新（慢）。
    fn on_tick(&mut self, slow: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut dirty = false;
        if matches!(self.state.screen, Screen::Busy { .. }) {
            let current = self.exec.progress();
            if current != self.shown_progress {
                self.shown_progress = current;
                if let Some(progress) = self.shown_progress.clone() {
                    self.dispatch(Event::JobProgress(progress), window, cx);
                    dirty = true;
                }
            }
        }
        if slow {
            self.tick_watch(window, cx);
            self.exec.refresh_summary();
            dirty = true;
        }
        if dirty {
            cx.notify();
        }
    }

    /// 输入文件的外部修改：约 1s 轮询 mtime，变化即以 `FileChanged` 进状态机。
    ///
    /// 只报**当前源**（别的文件一概忽略）；Busy 期不打断作业——基线不更新，
    /// 作业结束后下一拍自然会把变化读进来。
    fn tick_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.state.document.source.clone();
        if source != self.watch.source {
            // 源刚换过：加载已在飞，同一拍不再重复报。
            self.watch.mtime = source.as_deref().and_then(file_mtime);
            self.watch.source = source;
            return;
        }
        if matches!(self.state.screen, Screen::Busy { .. }) {
            return;
        }
        let Some(path) = source else {
            self.watch.mtime = None;
            return;
        };
        let modified = file_mtime(&path);
        if modified == self.watch.mtime {
            return;
        }
        self.watch.mtime = modified;
        if modified.is_some() {
            self.dispatch(Event::FileChanged { path }, window, cx);
        }
    }
}

/// `NotifyLevel` → 窗口通知类型（含 Warn → Warning 的名称差）。
fn notification_type(level: NotifyLevel) -> NotificationType {
    match level {
        NotifyLevel::Info => NotificationType::Info,
        NotifyLevel::Warn => NotificationType::Warning,
        NotifyLevel::Error => NotificationType::Error,
    }
}
