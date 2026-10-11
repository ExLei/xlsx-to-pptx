//! 输入适配器：xlsx（calamine）/ csv（csv + encoding_rs）。
//!
//! 两个格式共用同一条流水线：表头校验（对照 [`crate::domain::schema::FIELDS`]）
//! → 逐行解析 → 跳过「英文单词为空」的行（记 warn + 计数）。

use std::path::Path;

use crate::diag::Diag;
use crate::domain::schema::{Field, SheetInfo, Source, WordEntry};

/// 读取结果。
#[derive(Debug, Clone)]
pub struct ReadResult {
    pub entries: Vec<WordEntry>,
    pub skipped: usize,
    pub columns: Vec<Field>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("无法打开文件: {0}")]
    Open(String),
    #[error("缺少列: {0:?}")]
    MissingColumns(Vec<&'static str>),
    #[error("第 {row} 行解析失败: {detail}")]
    RowParse { row: usize, detail: String },
    #[error("编码错误: {label}")]
    Encoding { label: String },
    #[error("工作表为空或没有表头")]
    EmptySheet,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 列出工作表（Excel）。
pub fn list_sheets(path: &Path, diag: &Diag) -> Result<Vec<SheetInfo>, LoadError> {
    let _ = (path, diag);
    unimplemented!("M1：见 docs/design.md §6.2")
}

/// 读取词条；`SourceKind::Excel { sheet: None }` 时取第一个工作表。
pub fn read(source: &Source, diag: &Diag) -> Result<ReadResult, LoadError> {
    let _ = (source, diag);
    unimplemented!("M1")
}
