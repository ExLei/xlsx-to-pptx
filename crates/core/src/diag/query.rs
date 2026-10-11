//! 诊断查询引擎：CLI `diag` 子命令与 GUI 摘要共用。
//!
//! 语义（`docs/design.md` §7.2 + 旧版 `diag` 子命令的兼容面）：
//! - [`Query::Summary`]：总数 / 等级计数 / 按 target 计数 / 最近若干 WARN|ERROR；
//! - [`Query::BlankSlides`]：`reason ∈ {blank, low_density}` 的 verify 事件；
//! - [`Query::FontTrace`]：`fonts` 探针事件；
//! - [`Query::Errors`]：全部 WARN|ERROR；
//! - [`Query::Slide(n)`]：`field::SLIDE == n` 的事件；
//! - [`Query::All`]：全量。
//!
//! 修复（旧版缺陷）：坏行计数并报告（不静默丢弃）；`--summary --json` 可共存（由前端组合）。

use std::path::Path;

/// 查询类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Summary,
    BlankSlides,
    FontTrace,
    Errors,
    Slide(usize),
    All,
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("未找到日志: {0}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 查询结果。
#[derive(Debug)]
pub struct Report {/* M1 实现 */}

impl Report {
    /// 是否没有匹配事件。
    pub fn is_empty(&self) -> bool {
        unimplemented!("M1")
    }

    /// 人类可读文本。
    pub fn render_human(&self) -> String {
        unimplemented!("M1")
    }

    /// 脚本消费（`--json`）与 GUI 摘要共用的结构化形态。
    pub fn to_json(&self) -> serde_json::Value {
        unimplemented!("M1")
    }
}

/// 读取并解析日志，运行查询。
pub fn run(log: &Path, query: &Query) -> Result<Report, QueryError> {
    let _ = (log, query);
    unimplemented!("M1：见 docs/design.md §7.2")
}
