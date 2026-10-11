//! `pptx::Package`：ZIP 与 OOXML 结构手术的唯一入口（深模块）。
//!
//! 对外只暴露四件事：打开 / 扫描 / 填充 / 落盘。
//! OOXML 的一切细节（`[Content_Types].xml`、`ppt/presentation.xml`、rels、
//! 复制末页的结构修补）都藏在实现里；调用方永远不碰 zip 与 XML。
//!
//! 不变量（见 `docs/design.md` §6.1）：
//! - `open` 失败必给具体原因（不是 ZIP / 缺结构 / 无幻灯片）；
//! - `fill` 页数守恒（词条数 > 页数时复制末页）；
//! - `save_atomic` 落盘后重开自检；任何失败不产出残损包。

use std::ops::ControlFlow;
use std::path::Path;

use crate::diag::Diag;
use crate::domain::plan::{Artifact, OutputError, OutputPlan};
use crate::domain::schema::{Field, WordEntry};
use crate::ports::OpObserver;

/// 内嵌版式资产（由 [`crate::domain::schema::FIELDS`] 的标记生成）。
pub const EMBEDDED_TEMPLATE: &[u8] = include_bytes!("../../assets/template.pptx");

/// 模板里的字段标记体系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScheme {
    /// 内嵌版式：`__WORD__` 等 ASCII 标记（用户不可见）。
    Markers,
    /// 用户模板：`{{单词}}` 等中文占位符（兼容面）。
    Placeholders,
}

#[derive(Debug, thiserror::Error)]
pub enum PptxError {
    #[error("不是有效的 PPTX（ZIP 解析失败）: {0}")]
    NotZip(String),
    #[error("PPTX 缺少必需结构: {0}")]
    MissingStructure(&'static str),
    #[error("演示文稿没有任何幻灯片")]
    NoSlides,
    #[error("XML 处理失败: {0}")]
    Xml(String),
    #[error("ZIP 处理失败: {0}")]
    Zip(String),
    #[error(transparent)]
    Output(#[from] OutputError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 打开后的 PPTX 包。
pub struct Package {/* M1：zip 条目表 + 幻灯片清单（实现私有） */}

impl Package {
    /// 打开文件。
    pub fn open(path: &Path, diag: &Diag) -> Result<Package, PptxError> {
        let _ = (path, diag);
        unimplemented!("M1：见 docs/design.md §6.1")
    }

    /// 从字节打开（内嵌资产 / 测试 fixture）。
    pub fn open_bytes(bytes: &[u8], diag: &Diag) -> Result<Package, PptxError> {
        let _ = (bytes, diag);
        unimplemented!("M1")
    }

    /// 幻灯片页数。
    pub fn slide_count(&self) -> usize {
        unimplemented!("M1")
    }

    /// 扫描包中出现的字段标记（按 [`TokenScheme`] 选择的词表）。
    pub fn scan_tokens(&self, scheme: TokenScheme) -> Vec<Field> {
        let _ = scheme;
        unimplemented!("M1")
    }

    /// 逐页填充：按 [`crate::domain::schema::FIELDS`] 替换该页字段文本；
    /// 页数不足时复制最后一页并修补 `[Content_Types].xml` / `presentation.xml` / rels。
    ///
    /// 返回 [`ControlFlow::Break`] 表示观察者请求取消（尚未落盘，无需清理）。
    pub fn fill(
        &mut self,
        entries: &[WordEntry],
        scheme: TokenScheme,
        observer: &mut dyn OpObserver,
        diag: &Diag,
    ) -> Result<ControlFlow<()>, PptxError> {
        let _ = (entries, scheme, observer, diag);
        unimplemented!("M1")
    }

    /// 原子落盘 + 落盘后重开自检（页数守恒）。
    pub fn save_atomic(&self, plan: &OutputPlan, diag: &Diag) -> Result<Artifact, PptxError> {
        let _ = (plan, diag);
        unimplemented!("M1")
    }
}
