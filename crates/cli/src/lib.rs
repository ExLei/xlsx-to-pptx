//! CLI 前端：参数映射 + 结果渲染 + 退出码。
//!
//! 只依赖 `x2p-core`（ops / domain / diag / paths）。业务编排一律在用例层；
//! 这里只有 clap 解析、stdout 渲染、会话日志装配与退出码。
//!
//! 测试入口：[`run_status`]（不打印、返回结构化结果）；生产入口：[`run_from`]。

use std::ffi::OsString;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use x2p_core::diag::{self, query, Diag};
use x2p_core::domain::plan::{OutputPlan, OverwritePolicy};
use x2p_core::domain::schema::{Source, SourceKind};
use x2p_core::ops::{self, Finished, OpContext, OpObserver, OpResult, Progress, RendererChoice, SourceOptions};
use x2p_core::paths::Layout;

/// 失败分类：用法错误（退出码 2）与运行错误（退出码 1）。
#[derive(Debug)]
pub enum CliFailure {
    Usage(String),
    Run(String),
}

impl std::fmt::Display for CliFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliFailure::Usage(message) => write!(f, "{message}"),
            CliFailure::Run(message) => write!(f, "错误: {message}"),
        }
    }
}

/// 完整 CLI 入口（`args` 含 argv[0]）。
pub fn run_from<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match run_status(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliFailure::Usage(message)) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
        Err(CliFailure::Run(message)) => {
            eprintln!("错误: {message}");
            ExitCode::FAILURE
        }
    }
}

/// 测试入口：解析并执行，不打印错误（stdout 输出仍会打印）。
pub fn run_status<I, T>(args: I) -> Result<(), CliFailure>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let argv: Vec<String> = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() {
                return Err(CliFailure::Usage(error.render().to_string()));
            }
            // --help / --version：正常输出。
            print!("{error}");
            return Ok(());
        }
    };
    execute(cli, &argv).map_err(CliFailure::Run)
}

#[derive(Parser)]
#[command(name = "英语助记卡片生成", version, about = "Excel/CSV 词汇表 → 16:9 PPTX 单词卡")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 单文件生成：将词汇表转换为 PPTX
    Generate {
        /// 输入文件路径 (.xlsx / .xls / .csv)
        #[arg(short, long, value_name = "FILE")]
        input: PathBuf,
        /// 输出 PPTX 路径（缺省 → 数据目录/PPT/<输入名>.pptx）
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// PPTX 模板文件（含 {{占位符}} 的用户模板）
        #[arg(short = 't', long, value_name = "FILE")]
        template: Option<PathBuf>,
        /// Excel 工作表名称（缺省取第一个）
        #[arg(short = 'S', long, value_name = "NAME")]
        sheet: Option<String>,
        /// CSV 文件编码（默认 utf-8）
        #[arg(short, long, value_name = "ENCODING", default_value = "utf-8")]
        encoding: String,
        /// 强制覆盖已存在的输出文件
        #[arg(short, long)]
        force: bool,
    },
    /// 批量生成：遍历目录中的全部词汇表
    Batch {
        /// 输入目录路径
        #[arg(short, long, value_name = "DIR")]
        input: PathBuf,
        /// 输出目录路径（缺省 → 数据目录/PPT/）
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// Excel 工作表名称（缺省取第一个）
        #[arg(short = 'S', long, value_name = "NAME")]
        sheet: Option<String>,
        /// CSV 文件编码（默认 utf-8）
        #[arg(short, long, value_name = "ENCODING", default_value = "utf-8")]
        encoding: String,
        /// 强制覆盖已存在的输出文件
        #[arg(short, long)]
        force: bool,
        /// PPTX 模板文件（含 {{占位符}}）
        #[arg(short = 't', long, value_name = "FILE")]
        template: Option<PathBuf>,
    },
    /// 生成示例 PPTX 模板（含 {{占位符}} 供用户修改）
    TemplatePptx {
        /// 输出路径（缺省 → 数据目录/示例模板.pptx）
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// 导出 PPTX 为 PNG（模板模式需本机 Office / WPS / LibreOffice；无模板走内置管线）
    ExportPng {
        /// 输入文件路径 (.xlsx / .csv)
        #[arg(short, long, value_name = "FILE")]
        input: PathBuf,
        /// 输出目录（缺省 → 数据目录/PNG/）
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// PPTX 模板文件（提取排版）
        #[arg(short = 't', long, value_name = "FILE")]
        template: Option<PathBuf>,
        /// Excel 工作表名称
        #[arg(short = 'S', long, value_name = "NAME")]
        sheet: Option<String>,
        /// CSV 编码（默认 utf-8）
        #[arg(short, long, value_name = "ENCODING", default_value = "utf-8")]
        encoding: String,
        /// 外部渲染器（默认 auto：PowerPoint → LibreOffice → WPS）
        #[arg(long, value_name = "RENDERER", default_value = "auto", value_parser = parse_renderer)]
        renderer: RendererChoice,
    },
    /// 导出词汇表 Excel 模板
    Template {
        /// 输出路径（缺省 → 数据目录/单词表模板.xlsx）
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// 查看导出诊断日志
    Diag {
        /// 日志文件路径（缺省 → 数据目录/日志/ 中最新的一个）
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
        /// 显示汇总统计
        #[arg(short, long)]
        summary: bool,
        /// 显示空白 slide 检测事件
        #[arg(long)]
        blank_slides: bool,
        /// 显示字体探测追踪
        #[arg(long)]
        font_trace: bool,
        /// 显示所有错误和警告
        #[arg(long)]
        errors: bool,
        /// 查看特定 slide 的详细信息
        #[arg(long, value_name = "N")]
        slide: Option<usize>,
        /// 以 JSON 格式输出（脚本消费）
        #[arg(long)]
        json: bool,
    },
}

fn parse_renderer(value: &str) -> Result<RendererChoice, String> {
    RendererChoice::parse(value)
        .ok_or_else(|| format!("未知渲染器: {value}（可选 auto / powerpoint / libreoffice / wps）"))
}

fn execute(cli: Cli, argv: &[String]) -> Result<(), String> {
    let layout = Layout::resolve();
    match cli.command {
        Command::Diag { file, summary, blank_slides, font_trace, errors, slide, json } => {
            run_diag(&layout, file, summary, blank_slides, font_trace, errors, slide, json)
        }
        Command::Generate { input, output, template, sheet, encoding, force } => {
            with_session(&layout, "generate", argv, |diag| {
                let source = source_from(&input, sheet, encoding);
                let policy = force_policy(force);
                let plan = file_plan(output.as_deref(), || layout.ppt_file(&stem_of(&input)), policy);
                let mut observer = CliObserver::new();
                let mut cx = OpContext { diag, observer: &mut observer };
                let loaded = expect(ops::load_entries(&source, &mut cx))?;
                let Some(loaded) = loaded else {
                    println!("已取消");
                    return Ok(());
                };
                let artifact = expect(ops::generate_deck(&loaded.entries, template.as_deref(), plan, &mut cx))?;
                observer.finish();
                let Some(artifact) = artifact else {
                    println!("已取消");
                    return Ok(());
                };
                println!("完成: {} — {} 页", artifact.path.display(), artifact.slides);
                Ok(())
            })
        }
        Command::Batch { input, output, sheet, encoding, force, template } => {
            with_session(&layout, "batch", argv, |diag| {
                let output_dir = output.unwrap_or_else(|| layout.ppt_dir());
                let options = SourceOptions { sheet, encoding };
                let mut observer = CliObserver::new();
                let mut cx = OpContext { diag, observer: &mut observer };
                let summary = expect(ops::batch_generate(
                    &input,
                    options,
                    template.as_deref(),
                    output_dir,
                    force_policy(force),
                    &mut cx,
                ))?;
                observer.finish();
                let Some(summary) = summary else {
                    println!("已取消");
                    return Ok(());
                };
                println!("===== 批量生成汇总 =====");
                println!("成功: {} 个", summary.ok.len());
                for (name, artifact) in &summary.ok {
                    println!("[OK] {name} — {} 页", artifact.slides);
                }
                println!("失败: {} 个", summary.failed.len());
                for (name, error) in &summary.failed {
                    eprintln!("[FAIL] {name}: {error}");
                }
                if summary.failed.is_empty() {
                    Ok(())
                } else {
                    Err("部分文件生成失败".into())
                }
            })
        }
        Command::TemplatePptx { output } => with_session(&layout, "template-pptx", argv, |diag| {
            let plan = file_plan(output.as_deref(), || layout.template_pptx(), OverwritePolicy::Overwrite);
            let mut observer = CliObserver::new();
            let mut cx = OpContext { diag, observer: &mut observer };
            let artifact = expect(ops::make_example_template(plan, &mut cx))?;
            observer.finish();
            let Some(artifact) = artifact else {
                println!("已取消");
                return Ok(());
            };
            println!("完成: {}", artifact.path.display());
            Ok(())
        }),
        Command::ExportPng { input, output, template, sheet, encoding, renderer } => {
            with_session(&layout, "export", argv, |diag| {
                let source = source_from(&input, sheet, encoding);
                let dir = output.unwrap_or_else(|| layout.png_dir());
                let mut observer = CliObserver::new();
                let mut cx = OpContext { diag, observer: &mut observer };
                let loaded = expect(ops::load_entries(&source, &mut cx))?;
                let Some(loaded) = loaded else {
                    println!("已取消");
                    return Ok(());
                };
                let artifact = match ops::export_png(&loaded.entries, template.as_deref(), renderer, &dir, &mut cx) {
                    Ok(Finished::Completed(artifact)) => artifact,
                    Ok(Finished::Cancelled) => {
                        println!("已取消");
                        return Ok(());
                    }
                    Err(error) => return Err(render_op_error(&error)),
                };
                observer.finish();
                println!("完成: {} 张", artifact.slides.len());
                for slide in &artifact.slides {
                    println!("{}", slide.display());
                }
                Ok(())
            })
        }
        Command::Template { output } => with_session(&layout, "template", argv, |diag| {
            let plan = file_plan(output.as_deref(), || layout.template_xlsx(), OverwritePolicy::Overwrite);
            let mut observer = CliObserver::new();
            let mut cx = OpContext { diag, observer: &mut observer };
            let artifact = expect(ops::make_excel_template(plan, &mut cx))?;
            observer.finish();
            let Some(artifact) = artifact else {
                println!("已取消");
                return Ok(());
            };
            println!("完成: {}", artifact.path.display());
            Ok(())
        }),
    }
}

// ── 会话装配 ──────────────────────────────────────────────────────────────

/// 非 diag 命令：每进程一份日志会话（File + stderr 镜像），首行记录环境；
/// 运行后汇报日志写入故障，最后打印日志路径。
fn with_session(
    layout: &Layout,
    prefix: &str,
    argv: &[String],
    run: impl FnOnce(&Diag) -> Result<(), String>,
) -> Result<(), String> {
    let (diag, log_path) = diag::start_session(&layout.logs_dir(), prefix, true);
    diag.record_session(serde_json::json!({
        x2p_core::diag::field::VERSION: env!("CARGO_PKG_VERSION"),
        x2p_core::diag::field::COMMAND: prefix,
        x2p_core::diag::field::ARGS: argv,
        x2p_core::diag::field::DATA_DIR: layout.root().display().to_string(),
        x2p_core::diag::field::FALLBACK: layout.is_fallback(),
        x2p_core::diag::field::EXE_PATH: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        x2p_core::diag::field::OS: std::env::consts::OS,
    }));
    let outcome = run(&diag);
    if diag.write_failures() > 0 {
        eprintln!(
            "警告: 日志写入失败 {} 次（最近: {}）",
            diag.write_failures(),
            diag.last_failure().unwrap_or_default()
        );
    }
    if let Some(path) = &log_path {
        println!("日志: {}", path.display());
    }
    outcome
}

// ── diag 子命令（只读，不建会话）───────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_diag(
    layout: &Layout,
    file: Option<PathBuf>,
    summary: bool,
    blank_slides: bool,
    font_trace: bool,
    errors: bool,
    slide: Option<usize>,
    json: bool,
) -> Result<(), String> {
    let log = match file {
        Some(path) => path,
        None => diag::newest_log(&layout.logs_dir())
            .ok_or_else(|| format!("未找到日志: {}", layout.logs_dir().display()))?,
    };
    let query = if summary {
        query::Query::Summary
    } else if let Some(number) = slide {
        query::Query::Slide(number)
    } else if blank_slides {
        query::Query::BlankSlides
    } else if font_trace {
        query::Query::FontTrace
    } else if errors {
        query::Query::Errors
    } else {
        query::Query::All
    };
    let report = query::run(&log, &query).map_err(|error| error.to_string())?;
    if json {
        let value = report.to_json();
        println!("{}", serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?);
    } else {
        print!("{}", report.render_human());
    }
    Ok(())
}

// ── 辅助 ─────────────────────────────────────────────────────────────────

/// CLI 进度渲染（CLI 不可取消：观察者恒返回 Continue）。
struct CliObserver {
    printed: bool,
}

impl CliObserver {
    fn new() -> CliObserver {
        CliObserver { printed: false }
    }

    fn finish(&self) {
        if self.printed {
            println!();
        }
    }
}

impl OpObserver for CliObserver {
    fn on(&mut self, progress: &Progress) -> ControlFlow<()> {
        match progress {
            Progress::Stage(label) => print!("\r{label}…"),
            Progress::Slides { done, total } => print!("\r生成进度: {done}/{total}"),
            Progress::Files { done, total } => print!("\r批量进度: {done}/{total}"),
        }
        self.printed = true;
        if let Err(error) = std::io::Write::flush(&mut std::io::stdout()) {
            if error.kind() != std::io::ErrorKind::BrokenPipe {
                eprintln!("进度输出失败: {error}");
            }
        }
        ControlFlow::Continue(())
    }
}

/// 用例结果 → CLI 结果：取消 → `Ok(None)`。
fn expect<T>(result: OpResult<T>) -> Result<Option<T>, String> {
    match result {
        Ok(Finished::Completed(value)) => Ok(Some(value)),
        Ok(Finished::Cancelled) => Ok(None),
        Err(error) => Err(render_op_error(&error)),
    }
}

/// 错误渲染：渲染器缺失时附安装指引（与 GUI 共用同一份文案）。
fn render_op_error(error: &ops::OpError) -> String {
    match error.renderer_help() {
        Some(help) => format!("{error}\n\n{help}"),
        None => error.to_string(),
    }
}

fn force_policy(force: bool) -> OverwritePolicy {
    if force {
        OverwritePolicy::Overwrite
    } else {
        OverwritePolicy::Fail
    }
}

/// 输出计划：显式路径（目录 + 文件名）或默认落点。
fn file_plan(explicit: Option<&Path>, default: impl FnOnce() -> PathBuf, policy: OverwritePolicy) -> OutputPlan {
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => default(),
    };
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    OutputPlan::new(dir, name, policy)
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into())
}

/// 输入源构造（对等旧版：csv → CSV；其余一律当 Excel）。
fn source_from(path: &Path, sheet: Option<String>, encoding: String) -> Source {
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let kind = if extension == "csv" {
        SourceKind::Csv { encoding }
    } else {
        SourceKind::Excel { sheet }
    };
    Source { path: path.to_path_buf(), kind }
}
