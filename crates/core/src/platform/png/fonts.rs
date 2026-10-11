//! 字体链探测：latin / cjk / emoji 三链（`diag --font-trace` 的数据来源）。

use crate::diag::Diag;

/// 探测到的字体配置。
#[derive(Debug, Clone, Default)]
pub struct FontConfig {
    pub latin: Option<String>,
    pub cjk: Option<String>,
    pub emoji: Option<String>,
}

/// 探测系统字体：逐链找第一个可用字体；整链缺失记 warn（含 `chain` / `missing` 字段）。
pub fn probe(diag: &Diag) -> FontConfig {
    let _ = diag;
    unimplemented!("M1：见 docs/design.md §6.4")
}
