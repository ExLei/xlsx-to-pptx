//! 用例层：8 个入口，编排 `domain` 与 `platform`。
//!
//! 设计契约：进度 / 取消 / 诊断是这层接口的**自带能力**（`OpContext`），
//! 前端只做「参数映射 + 结果渲染」。取消是合法终态（[`Finished::Cancelled`]），不是错误。

pub mod batch;
pub mod check;
pub mod export_png;
pub mod generate;
pub mod load;
pub mod template;

use std::path::PathBuf;

use crate::diag::Diag;
use crate::domain::plan::{Artifact, OutputError, OutputPlan, OverwritePolicy};
use crate::domain::schema::{Field, WordEntry};
use crate::platform::png::{PngError, RenderError};
use crate::platform::pptx::PptxError;
use crate::platform::sheets::LoadError;

pub use crate::domain::schema::{SheetInfo, Source, SourceKind};
pub use crate::platform::png::RendererChoice;
pub use crate::ports::{NoopObserver, OpObserver, Progress};

/// 运行上下文：诊断句柄 + 观察者端口。
pub struct OpContext<'a> {
    pub diag: &'a Diag,
    pub observer: &'a mut dyn OpObserver,
}

impl OpContext<'_> {
    /// 上报进度；返回 `true` 表示观察者请求取消（循环里逐次检查）。
    pub(crate) fn cancelled(&mut self, progress: Progress) -> bool {
        self.observer.on(&progress).is_break()
    }
}

/// 作业终态：完成或取消。取消不是错误。
#[derive(Debug)]
pub enum Finished<T> {
    Completed(T),
    Cancelled,
}

pub type OpResult<T> = Result<Finished<T>, OpError>;

/// 顶层错误：聚合各层错误并保留结构（前端不做字符串化丢失）。
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error(transparent)]
    Pptx(#[from] PptxError),
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Png(#[from] PngError),
    #[error(transparent)]
    Output(#[from] OutputError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("没有可生成的条目")]
    NoEntries,
    #[error("{op}: {detail}")]
    Internal { op: &'static str, detail: String },
}

impl OpError {
    /// 渲染器缺失时附带的安装指引（CLI 与 GUI 共用一份文案）。
    pub fn renderer_help(&self) -> Option<&'static str> {
        if matches!(self, OpError::Render(RenderError::NoRenderer)) {
            Some(crate::platform::png::renderer_help())
        } else {
            None
        }
    }
}

/// 批量生成时的输入选项。
#[derive(Debug, Clone)]
pub struct SourceOptions {
    pub sheet: Option<String>,
    pub encoding: String,
}

impl Default for SourceOptions {
    fn default() -> Self {
        Self {
            sheet: None,
            encoding: "utf-8".into(),
        }
    }
}

/// 加载结果：词条 + 跳过计数。
#[derive(Debug, Clone)]
pub struct LoadSummary {
    pub entries: Vec<WordEntry>,
    pub skipped: usize,
}

/// 模板校验结果。
#[derive(Debug, Clone)]
pub struct TemplateCheck {
    pub slides: usize,
    pub fields: Vec<Field>,
    pub missing_word: bool,
}

/// 批量生成结果（失败是数据不是错误；退出码由前端决定）。
#[derive(Debug)]
pub struct BatchSummary {
    pub ok: Vec<(String, Artifact)>,
    pub failed: Vec<(String, OpError)>,
}

/// PNG 导出结果。
#[derive(Debug, Clone)]
pub struct PngArtifact {
    pub dir: PathBuf,
    pub slides: Vec<PathBuf>,
}

// ── 8 个用例入口（实现见各子模块）─────────────────────────────────────────

/// 列出工作表（`sheet = None` 时用例层取第一个）。
pub fn list_sheets(path: &std::path::Path, cx: &mut OpContext) -> OpResult<Vec<SheetInfo>> {
    load::list_sheets(path, cx)
}

/// 加载词条（已过滤：`英文单词` 为空的行跳过并计数）。
pub fn load_entries(source: &Source, cx: &mut OpContext) -> OpResult<LoadSummary> {
    load::load_entries(source, cx)
}

/// 校验模板：扫描占位符 + `{{单词}}` 必填判定。
pub fn check_template(template: &std::path::Path, cx: &mut OpContext) -> OpResult<TemplateCheck> {
    check::check_template(template, cx)
}

/// 生成 PPTX（`template = None` → 内嵌版式）。
pub fn generate_deck(
    entries: &[WordEntry],
    template: Option<&std::path::Path>,
    output: OutputPlan,
    cx: &mut OpContext,
) -> OpResult<Artifact> {
    generate::generate_deck(entries, template, output, cx)
}

/// 批量生成：遍历目录、continue-on-error、汇总。
pub fn batch_generate(
    input_dir: &std::path::Path,
    options: SourceOptions,
    template: Option<&std::path::Path>,
    output_dir: PathBuf,
    policy: OverwritePolicy,
    cx: &mut OpContext,
) -> OpResult<BatchSummary> {
    batch::batch_generate(input_dir, options, template, output_dir, policy, cx)
}

/// 导出 PNG：模板模式外呼渲染器；无模板模式走内置 SVG 管线。
pub fn export_png(
    entries: &[WordEntry],
    template: Option<&std::path::Path>,
    renderer: RendererChoice,
    dir: &std::path::Path,
    cx: &mut OpContext,
) -> OpResult<PngArtifact> {
    export_png::export_png(entries, template, renderer, dir, cx)
}

/// 生成示例 PPTX 模板（含 `{{占位符}}`）。
pub fn make_example_template(output: OutputPlan, cx: &mut OpContext) -> OpResult<Artifact> {
    template::make_example_template(output, cx)
}

/// 生成 Excel 词表模板（表头来自字段表）。
pub fn make_excel_template(output: OutputPlan, cx: &mut OpContext) -> OpResult<Artifact> {
    template::make_excel_template(output, cx)
}
