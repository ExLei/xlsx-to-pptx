//! 加载用例：列工作表 / 读词条。

use std::path::Path;

use super::{Finished, LoadSummary, OpContext, OpResult};
use crate::domain::schema::{SheetInfo, Source};

/// 列出工作表。
pub fn list_sheets(path: &Path, cx: &mut OpContext) -> OpResult<Vec<SheetInfo>> {
    cx.diag.info(
        crate::diag::Target::Sheets,
        "list sheets",
        Some(serde_json::json!({"path": path.display().to_string()})),
    );
    let sheets = crate::platform::sheets::list_sheets(path, cx.diag)?;
    cx.diag.info(
        crate::diag::Target::Sheets,
        "sheets listed",
        Some(serde_json::json!({"count": sheets.len()})),
    );
    Ok(Finished::Completed(sheets))
}

/// 读取词条：调用平台层读取器，产出过滤后的词条与跳过计数。
pub fn load_entries(source: &Source, cx: &mut OpContext) -> OpResult<LoadSummary> {
    cx.diag.info(
        crate::diag::Target::Sheets,
        "load entries",
        Some(serde_json::json!({"path": source.path.display().to_string()})),
    );
    if cx.cancelled(crate::ports::Progress::Stage("读取表格")) {
        return Ok(Finished::Cancelled);
    }
    let read = crate::platform::sheets::read(source, cx.diag)?;
    cx.diag.info(
        crate::diag::Target::Sheets,
        "entries loaded",
        Some(serde_json::json!({"entries": read.entries.len(), "skipped": read.skipped})),
    );
    Ok(Finished::Completed(LoadSummary {
        entries: read.entries,
        skipped: read.skipped,
    }))
}
