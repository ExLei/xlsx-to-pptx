//! 外呼渲染器：PowerPoint（COM / PowerShell）/ LibreOffice / WPS。
//!
//! 三个后端是**数据表里的三行**（[`ROWS`]：名称 + 探测方式 + 渲染 argv 模板 + 转义方式），
//! 不是三份代码；探测顺序 PowerPoint → LibreOffice → WPS（对等旧版）。
//!
//! 取消贯通：等待子进程时每 [`POLL_INTERVAL`] 轮询观察者，取消即杀子进程并回收；
//! 渲染器直接往输出目录写 PNG（非原子），所以取消/失败后清理**本次新产出**的那些页，
//! 渲染前已存在的文件一律不动。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::diag::{field, Diag, Target};
use crate::ports::{OpObserver, Progress};

use super::{PngRenderer, RenderError, RendererChoice};

/// 等待循环的轮询间隔（取消检查粒度）。
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 正常退出后等管道读取线程收尾的上限；超时就丢下线程，绝不拖住返回。
const PIPE_SETTLE: Duration = Duration::from_millis(300);
/// 失败详情的截断长度（字符）。
const DETAIL_LIMIT: usize = 500;
/// stderr 转发的行数上限（防噪声淹没日志）。
const RELAY_LIMIT: usize = 5;
/// 输出文件名前缀。
const SLIDE_PREFIX: &str = "slide_";

/// 外呼后端（数据表的主键）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    PowerPoint,
    LibreOffice,
    Wps,
}

impl Backend {
    /// `Auto` 的探测顺序（对等旧版；设计 §5.2）。
    pub const AUTO_ORDER: [Backend; 3] = [Backend::PowerPoint, Backend::LibreOffice, Backend::Wps];

    /// 展示名（diag 与错误信息的统一来源）。
    pub fn name(self) -> &'static str {
        self.row().name
    }

    /// 探测用的可执行文件候选（顺序即优先级；PowerPoint 走 COM，故为空）。
    pub fn executables(self) -> &'static [&'static str] {
        self.row().executables
    }

    /// 数据表中的行（表的次序与 [`Backend::AUTO_ORDER`] 一致）。
    fn row(self) -> &'static Row {
        &ROWS[self.index()]
    }

    fn index(self) -> usize {
        match self {
            Backend::PowerPoint => 0,
            Backend::LibreOffice => 1,
            Backend::Wps => 2,
        }
    }
}

/// 探测方式（数据行的一部分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detection {
    /// PowerPoint：PowerShell 里建 COM 对象自检（仅 Windows 有效）。
    PowerPointCom,
    /// 候选可执行文件 + `--version` 能跑起来。
    Executable,
}

/// argv 模板里 `{pptx}` / `{dir}` 的转义方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quoting {
    /// 进程 argv 原样传递，不经 shell。
    Verbatim,
    /// 内嵌进 PowerShell 单引号字符串：`'` → `''`。
    PowerShell,
}

impl Quoting {
    fn quote(self, value: &str) -> String {
        match self {
            Quoting::Verbatim => value.to_string(),
            Quoting::PowerShell => value.replace('\'', "''"),
        }
    }
}

/// 一个后端的数据行。
struct Row {
    name: &'static str,
    detection: Detection,
    /// 探测用的可执行文件候选。
    executables: &'static [&'static str],
    /// 渲染 argv 模板（`{pptx}` / `{dir}` 占位符）。
    argv: &'static [&'static str],
    quoting: Quoting,
}

/// 数据表：三行，一行一个后端（加后端 = 加行）。
const ROWS: [Row; 3] = [
    Row {
        name: "Microsoft PowerPoint",
        detection: Detection::PowerPointCom,
        executables: &[],
        argv: &[
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-Command",
            POWERPOINT_RENDER_SCRIPT,
        ],
        quoting: Quoting::PowerShell,
    },
    Row {
        name: "LibreOffice",
        detection: Detection::Executable,
        executables: &["libreoffice", "soffice"],
        argv: &[
            "--headless",
            "--convert-to",
            "png",
            "--outdir",
            "{dir}",
            "{pptx}",
        ],
        quoting: Quoting::Verbatim,
    },
    Row {
        name: "WPS Office",
        detection: Detection::Executable,
        executables: &["wps"],
        argv: &[
            "--headless",
            "--convert-to",
            "png",
            "--outdir",
            "{dir}",
            "{pptx}",
        ],
        quoting: Quoting::Verbatim,
    },
];

/// PowerPoint COM 渲染脚本（照旧版 mirror；`{pptx}` / `{dir}` 由数据行替换）。
const POWERPOINT_RENDER_SCRIPT: &str = r#"$ErrorActionPreference = 'Stop'
$ppt = New-Object -ComObject PowerPoint.Application
try { $ppt.Visible = [Microsoft.Office.Core.MsoTriState]::msoFalse } catch { Write-Error "VISIBLE_WARN: $_" }
try {
    $pres = $ppt.Presentations.Open('{pptx}')
    foreach ($s in $pres.Slides) {
        $path = Join-Path '{dir}' "slide_$($s.SlideNumber).png"
        $s.Export($path, 'PNG', 1920, 1080)
    }
    $pres.Close()
} finally {
    $ppt.Quit()
    [System.Runtime.InteropServices.Marshal]::ReleaseComObject($ppt) | Out-Null
}"#;

/// PowerPoint COM 自检脚本（旧版同款）。
const POWERPOINT_PROBE_SCRIPT: &str = "try { $p = New-Object -ComObject PowerPoint.Application; \
     $p.Quit(); Write-Output 'OK' } catch { Write-Output 'FAIL' }";

/// 探测并选择后端；`Auto` 顺序：PowerPoint → LibreOffice(`libreoffice`/`soffice`) → WPS(`wps`)。
pub fn select(choice: RendererChoice, diag: &Diag) -> Result<Box<dyn PngRenderer>, RenderError> {
    select_with(choice, &system_probe, diag)
}

/// 选择逻辑（探测可注入：测试不需要真装 Office）。
///
/// 单个后端缺失 → [`RenderError::Missing`]；`Auto` 全缺 → [`RenderError::NoRenderer`]。
pub fn select_with(
    choice: RendererChoice,
    probe: &dyn Fn(Backend, &Diag) -> Option<String>,
    diag: &Diag,
) -> Result<Box<dyn PngRenderer>, RenderError> {
    let candidates: &[Backend] = match choice {
        RendererChoice::Auto => &Backend::AUTO_ORDER,
        RendererChoice::PowerPoint => &[Backend::PowerPoint],
        RendererChoice::LibreOffice => &[Backend::LibreOffice],
        RendererChoice::Wps => &[Backend::Wps],
    };

    for backend in candidates {
        match probe(*backend, diag) {
            Some(exe) => {
                diag.info(
                    Target::Renderer,
                    format!("渲染器可用：{}（{exe}）", backend.name()),
                    Some(json!({ field::RENDERER: backend.name(), field::EXE: exe })),
                );
                return Ok(Box::new(ExternalCli::new(*backend, exe)));
            }
            None => diag.info(
                Target::Renderer,
                format!("渲染器不可用：{}", backend.name()),
                Some(json!({ field::RENDERER: backend.name() })),
            ),
        }
    }

    if choice == RendererChoice::Auto {
        diag.warn(
            Target::Renderer,
            "未检测到可用的外部渲染器",
            Some(json!({
                field::MISSING: Backend::AUTO_ORDER.map(|backend| backend.name()),
            })),
        );
        Err(RenderError::NoRenderer)
    } else {
        let name = choice.name();
        diag.error(
            Target::Renderer,
            format!("指定的渲染器不可用：{name}"),
            Some(json!({ field::RENDERER: name })),
        );
        Err(RenderError::Missing {
            name: name.to_string(),
        })
    }
}

/// 真实探测：返回该后端可用的可执行文件（`None` = 不可用）。
pub fn system_probe(backend: Backend, diag: &Diag) -> Option<String> {
    let row = backend.row();
    match row.detection {
        Detection::PowerPointCom => powerpoint_probe(diag),
        Detection::Executable => backend
            .executables()
            .iter()
            .find(|exe| version_available(exe))
            .map(|exe| (*exe).to_string()),
    }
}

/// PowerPoint COM 探测（仅 Windows）：PowerShell 建 COM 对象，输出 `OK` 即可用。
fn powerpoint_probe(diag: &Diag) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let output = match command(Path::new("powershell"))
        .args(["-NoProfile", "-Command", POWERPOINT_PROBE_SCRIPT])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            diag.warn(
                Target::Renderer,
                format!("PowerPoint 探测失败：无法运行 PowerShell（{error}）"),
                Some(json!({ field::RENDERER: "Microsoft PowerPoint", field::EXE: "powershell" })),
            );
            return None;
        }
    };
    if String::from_utf8_lossy(&output.stdout).trim() == "OK" {
        Some("powershell".to_string())
    } else {
        diag.info(
            Target::Renderer,
            "PowerPoint COM 不可用",
            Some(json!({ field::RENDERER: "Microsoft PowerPoint", "com": false })),
        );
        None
    }
}

/// `exe --version` 能跑起来即认为可用（对等旧版探测）。
fn version_available(exe: &str) -> bool {
    command(Path::new(exe)).arg("--version").output().is_ok()
}

/// 装配子进程：Windows 下不闪控制台窗口（`CREATE_NO_WINDOW`）。
fn command(program: &Path) -> Command {
    let command = Command::new(program);
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(0x0800_0000);
        command
    };
    command
}

/// 数据表驱动的一个后端。
pub struct ExternalCli {
    name: &'static str,
    row: &'static Row,
    executable: PathBuf,
}

impl ExternalCli {
    /// 由数据行构造；`exe` 是探测到的可执行文件（PowerPoint 为 `powershell`）。
    pub fn new(backend: Backend, exe: impl Into<PathBuf>) -> ExternalCli {
        let row = backend.row();
        ExternalCli {
            name: row.name,
            row,
            executable: exe.into(),
        }
    }

    /// 渲染 argv：按行的转义方式替换占位符。
    fn argv(&self, pptx: &Path, dir: &Path) -> Vec<String> {
        let quote = |path: &Path| self.row.quoting.quote(&path.to_string_lossy());
        let (dir, pptx) = (quote(dir), quote(pptx));
        self.row
            .argv
            .iter()
            .map(|template| template.replace("{dir}", &dir).replace("{pptx}", &pptx))
            .collect()
    }

    /// 收集 `slide_N.png`（扩展名大小写不敏感），按 N 数值排序；一个都没有 → 失败。
    fn collect(&self, dir: &Path, diag: &Diag) -> Result<Vec<PathBuf>, RenderError> {
        let mut found = scan_slides(dir).map_err(|error| RenderError::Failed {
            renderer: self.name.to_string(),
            detail: format!("读取输出目录失败: {error}"),
        })?;
        if found.is_empty() {
            let detail = format!("未产出任何 slide_N.png（{}）", dir.display());
            diag.warn(
                Target::Renderer,
                format!("{}：{detail}", self.name),
                Some(json!({
                    field::RENDERER: self.name,
                    field::DIR: dir.display().to_string(),
                })),
            );
            return Err(RenderError::Failed {
                renderer: self.name.to_string(),
                detail,
            });
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        diag.info(
            Target::Renderer,
            format!("{}：收集到 {} 张 PNG", self.name, found.len()),
            Some(json!({
                field::RENDERER: self.name,
                field::COUNT: found.len(),
                field::DIR: dir.display().to_string(),
            })),
        );
        Ok(found.into_iter().map(|(_, path)| path).collect())
    }
}

impl PngRenderer for ExternalCli {
    fn name(&self) -> &'static str {
        self.name
    }

    fn render(
        &self,
        pptx: &Path,
        dir: &Path,
        observer: &mut dyn OpObserver,
        diag: &Diag,
    ) -> Result<Vec<PathBuf>, RenderError> {
        crate::platform::io::ensure_dir(dir, diag)?;
        let existing = snapshot(dir, diag);
        let argv = self.argv(pptx, dir);
        diag.info(
            Target::Renderer,
            format!("调用 {}：{} {}", self.name, self.executable.display(), argv.join(" ")),
            Some(json!({
                field::RENDERER: self.name,
                field::EXE: self.executable.display().to_string(),
                field::ARGS: argv.join(" "),
                field::PATH: pptx.display().to_string(),
                field::DIR: dir.display().to_string(),
            })),
        );

        let mut child = command(&self.executable)
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| RenderError::Failed {
                renderer: self.name.to_string(),
                detail: format!("启动渲染子进程失败: {error}"),
            })?;
        // 管道必须有人读：子进程写满缓冲会卡死在写（后台线程各管一路）。
        let stdout_pipe = child.stdout.take().map(|pipe| drain(pipe, "stdout", diag));
        let stderr_pipe = child.stderr.take().map(|pipe| drain(pipe, "stderr", diag));

        let outcome = wait(&mut child, observer);
        let exited = match &outcome {
            Outcome::Exited(_) => true,
            Outcome::Cancelled | Outcome::WaitFailed(_) => false,
        };
        if !exited {
            // 取消或等待失败：子进程可能还活着。
            kill_and_reap(&mut child, diag);
        }
        // 取消路径不等管道（子进程可能留下继承管道的孙进程），句柄自行结束。
        let (stdout, stderr) = if exited {
            (
                pipe_output(stdout_pipe, "stdout", diag),
                pipe_output(stderr_pipe, "stderr", diag),
            )
        } else {
            (Vec::new(), Vec::new())
        };

        match outcome {
            Outcome::Cancelled => {
                diag.warn(
                    Target::Renderer,
                    format!("{}：渲染已取消，已杀掉子进程", self.name),
                    Some(json!({
                        field::RENDERER: self.name,
                        field::DIR: dir.display().to_string(),
                    })),
                );
                cleanup_new(dir, &existing, diag);
                Err(RenderError::Cancelled)
            }
            Outcome::WaitFailed(error) => {
                diag.error(
                    Target::Renderer,
                    format!("{}：等待子进程失败（{error}）", self.name),
                    Some(json!({ field::RENDERER: self.name })),
                );
                cleanup_new(dir, &existing, diag);
                Err(RenderError::Failed {
                    renderer: self.name.to_string(),
                    detail: format!("等待子进程失败: {error}"),
                })
            }
            Outcome::Exited(status) if !status.success() => {
                let detail = failure_detail(&stderr, &stdout, status);
                diag.error(
                    Target::Renderer,
                    format!("{}：渲染失败（{status}）：{detail}", self.name),
                    Some(json!({ field::RENDERER: self.name })),
                );
                cleanup_new(dir, &existing, diag);
                Err(RenderError::Failed {
                    renderer: self.name.to_string(),
                    detail,
                })
            }
            Outcome::Exited(_) => {
                relay_stderr(&stderr, self.name, diag);
                diag.info(
                    Target::Renderer,
                    format!("{}：转换完成", self.name),
                    Some(json!({ field::RENDERER: self.name })),
                );
                self.collect(dir, diag)
            }
        }
    }
}

/// 等待结果。
enum Outcome {
    /// 子进程自行退出（含非零退出码）。
    Exited(ExitStatus),
    /// 观察者请求取消（子进程未必已退出）。
    Cancelled,
    /// `try_wait` 本身失败。
    WaitFailed(std::io::Error),
}

/// 等待循环：轮询退出码，同时给观察者取消的机会。
fn wait(child: &mut Child, observer: &mut dyn OpObserver) -> Outcome {
    loop {
        if observer.on(&Progress::Stage("渲染")).is_break() {
            return Outcome::Cancelled;
        }
        match child.try_wait() {
            Ok(Some(status)) => return Outcome::Exited(status),
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => return Outcome::WaitFailed(error),
        }
    }
}

/// 杀子进程并回收（清理路径同样记 diag，不留僵尸）。
fn kill_and_reap(child: &mut Child, diag: &Diag) {
    if let Err(error) = child.kill() {
        diag.warn(
            Target::Renderer,
            format!("杀渲染子进程失败: {error}"),
            None,
        );
    }
    if let Err(error) = child.wait() {
        diag.warn(
            Target::Renderer,
            format!("回收渲染子进程失败: {error}"),
            None,
        );
    }
}

/// 后台读一路管道（读失败记 warn，不静默）。
fn drain<R: Read + Send + 'static>(
    mut pipe: R,
    label: &'static str,
    diag: &Diag,
) -> JoinHandle<Vec<u8>> {
    let diag = diag.clone();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Err(error) = pipe.read_to_end(&mut buffer) {
            diag.warn(
                Target::Renderer,
                format!("读取子进程 {label} 失败: {error}"),
                None,
            );
        }
        buffer
    })
}

/// 等读取线程收尾（至多 [`PIPE_SETTLE`]），超时丢下线程——返回不能被它拖住。
fn pipe_output(handle: Option<JoinHandle<Vec<u8>>>, label: &'static str, diag: &Diag) -> Vec<u8> {
    let Some(handle) = handle else {
        return Vec::new();
    };
    let deadline = Instant::now() + PIPE_SETTLE;
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !handle.is_finished() {
        diag.warn(
            Target::Renderer,
            format!("子进程 {label} 未在 {}ms 内读完，放弃转发", PIPE_SETTLE.as_millis()),
            None,
        );
        return Vec::new();
    }
    match handle.join() {
        Ok(bytes) => bytes,
        Err(_panic) => {
            diag.warn(
                Target::Renderer,
                format!("子进程 {label} 读取线程异常退出"),
                None,
            );
            Vec::new()
        }
    }
}

/// 非致命 stderr 转发：`VISIBLE_WARN:` 记 warn，其余记 info；条数有上限。
fn relay_stderr(stderr: &[u8], renderer: &str, diag: &Diag) {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    for line in lines.iter().take(RELAY_LIMIT) {
        if line.contains("VISIBLE_WARN:") {
            diag.warn(
                Target::Renderer,
                format!("{renderer}: {line}"),
                Some(json!({ field::RENDERER: renderer })),
            );
        } else {
            diag.info(
                Target::Renderer,
                format!("{renderer}: {line}"),
                Some(json!({ field::RENDERER: renderer })),
            );
        }
    }
    if lines.len() > RELAY_LIMIT {
        diag.info(
            Target::Renderer,
            format!("{renderer}: stderr 共 {} 行，其余未转发", lines.len()),
            Some(json!({ field::RENDERER: renderer, field::COUNT: lines.len() })),
        );
    }
}

/// 失败详情：优先 stderr、其次 stdout，截断到 [`DETAIL_LIMIT`] 个字符。
fn failure_detail(stderr: &[u8], stdout: &[u8], status: ExitStatus) -> String {
    let has_text = |bytes: &[u8]| bytes.iter().any(|byte| !byte.is_ascii_whitespace());
    let source = if has_text(stderr) { stderr } else { stdout };
    let text = String::from_utf8_lossy(source);
    let text = text.trim();
    if text.is_empty() {
        return format!("退出码 {status}");
    }
    let mut detail: String = text.chars().take(DETAIL_LIMIT).collect();
    if text.chars().count() > DETAIL_LIMIT {
        detail.push('…');
    }
    detail
}

/// 渲染前已存在的 `slide_N.png`（取消/失败时只清理本次新产出的）。
fn snapshot(dir: &Path, diag: &Diag) -> Vec<PathBuf> {
    match scan_slides(dir) {
        Ok(found) => found.into_iter().map(|(_, path)| path).collect(),
        Err(error) => {
            diag.warn(
                Target::Renderer,
                format!("读取输出目录失败（取消时不清理）: {error}"),
                Some(json!({ field::DIR: dir.display().to_string() })),
            );
            Vec::new()
        }
    }
}

/// 清理本次渲染新产出的 PNG（渲染器直接写盘、不是原子写；I1 不留半成品）。
fn cleanup_new(dir: &Path, existing: &[PathBuf], diag: &Diag) {
    let found = match scan_slides(dir) {
        Ok(found) => found,
        Err(error) => {
            diag.warn(
                Target::Renderer,
                format!("清理本次产出失败：读取目录出错（{error}）"),
                Some(json!({ field::DIR: dir.display().to_string() })),
            );
            return;
        }
    };
    let mut removed = 0usize;
    for (_, path) in found {
        if existing.contains(&path) {
            continue;
        }
        crate::platform::io::remove_file(&path, diag);
        removed += 1;
    }
    if removed > 0 {
        diag.warn(
            Target::Renderer,
            format!("已清理本次中断产出的 {removed} 张 PNG"),
            Some(json!({
                field::COUNT: removed,
                field::DIR: dir.display().to_string(),
            })),
        );
    }
}

/// 目录下的 `slide_<N>.png`（扩展名大小写不敏感）：`(N, 路径)`，顺序未定义。
fn scan_slides(dir: &Path) -> std::io::Result<Vec<(u32, PathBuf)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if let Some(number) = slide_number(&path) {
            found.push((number, path));
        }
    }
    Ok(found)
}

/// `slide_<N>.png` → `N`（扩展名与前缀大小写不敏感；其它文件返回 `None`）。
fn slide_number(path: &Path) -> Option<u32> {
    if !path.extension()?.to_string_lossy().eq_ignore_ascii_case("png") {
        return None;
    }
    let stem = path.file_stem()?.to_string_lossy();
    if !stem.get(..SLIDE_PREFIX.len())?.eq_ignore_ascii_case(SLIDE_PREFIX) {
        return None;
    }
    stem[SLIDE_PREFIX.len()..].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 数据表次序 = `Auto` 探测顺序：行与主键一一对应。
    #[test]
    fn rows_follow_auto_order() {
        for (index, backend) in Backend::AUTO_ORDER.iter().enumerate() {
            assert_eq!(backend.index(), index, "{backend:?} 的行号不符");
            assert_eq!(ROWS[index].name, backend.name());
        }
    }

    /// 输出文件识别：只认 `slide_<N>.png`，扩展名大小写不敏感。
    #[test]
    fn slide_number_matches_only_numbered_png() {
        assert_eq!(slide_number(Path::new("/x/slide_7.png")), Some(7));
        assert_eq!(slide_number(Path::new("/x/slide_7.PNG")), Some(7));
        assert_eq!(slide_number(Path::new("/x/SLIDE_7.PnG")), Some(7));
        assert_eq!(slide_number(Path::new("/x/slide_x.png")), None);
        assert_eq!(slide_number(Path::new("/x/slide_.png")), None);
        assert_eq!(slide_number(Path::new("/x/cover.png")), None);
        assert_eq!(slide_number(Path::new("/x/slide_7.jpg")), None);
    }

    /// 占位符替换：PowerPoint 行做单引号转义，其余原样。
    #[test]
    fn argv_quoting_matches_row() {
        let libreoffice = ExternalCli::new(Backend::LibreOffice, "soffice");
        let argv = libreoffice.argv(Path::new("/tmp/deck's.pptx"), Path::new("/tmp/out"));
        assert_eq!(
            argv,
            vec![
                "--headless",
                "--convert-to",
                "png",
                "--outdir",
                "/tmp/out",
                "/tmp/deck's.pptx",
            ]
        );

        let powerpoint = ExternalCli::new(Backend::PowerPoint, "powershell");
        let argv = powerpoint.argv(Path::new("/tmp/deck's.pptx"), Path::new("/tmp/out"));
        let script = argv.last().expect("PowerPoint 行的最后一项是脚本");
        assert!(
            script.contains("'/tmp/deck''s.pptx'"),
            "PowerShell 单引号必须转义: {script}"
        );
        assert!(script.contains("Join-Path '/tmp/out'"), "输出目录未替换: {script}");
    }
}
