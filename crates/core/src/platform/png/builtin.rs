//! 内置 SVG 管线（无模板模式）：字体链 → 逐页 SVG → resvg 渲染 → verify → 原子写。
//!
//! 版式数据（1920×1080 坐标、标签、字号）由 [`crate::domain::schema::FIELDS`] 驱动；
//! verify 产生 `reason ∈ {blank, low_density}` 事件（`diag --blank-slides` 契约）。

use std::path::{Path, PathBuf};

use crate::diag::Diag;
use crate::domain::schema::WordEntry;
use crate::ports::OpObserver;

use super::PngError;

/// 渲染全部词条到 `dir`，逐页 `slide_N.png`（1 起）。
pub fn render(
    entries: &[WordEntry],
    dir: &Path,
    observer: &mut dyn OpObserver,
    diag: &Diag,
) -> Result<Vec<PathBuf>, PngError> {
    let _ = (entries, dir, observer, diag);
    unimplemented!("M1：见 docs/design.md §6.3")
}
