//! 批量生成用例：目录遍历 + continue-on-error + 汇总。
//!
//! 失败是**数据**不是错误：`BatchSummary` 同时携带成功与失败；
//! 退出码由前端决定（CLI：有失败即非零）。

use std::fs;
use std::path::{Path, PathBuf};

use super::{Finished, OpContext, OpResult, SourceOptions};
use crate::diag::{field, Target};
use crate::domain::plan::OverwritePolicy;
use crate::domain::schema::{Source, SourceKind};
use crate::ports::Progress;

const WHITELIST: [&str; 3] = ["xlsx", "xls", "csv"];

/// 遍历 `input_dir` 中的词汇表（白名单 xlsx/xls/csv），逐个 加载 + 生成。
pub fn batch_generate(
    input_dir: &Path,
    options: SourceOptions,
    template: Option<&Path>,
    output_dir: PathBuf,
    policy: OverwritePolicy,
    cx: &mut OpContext,
) -> OpResult<super::BatchSummary> {
    cx.diag.info(
        Target::Batch,
        "batch start",
        Some(serde_json::json!({
            "path": input_dir.display().to_string(),
            "dir": output_dir.display().to_string(),
        })),
    );
    if cx.cancelled(Progress::Stage("扫描目录")) {
        return Ok(Finished::Cancelled);
    }
    crate::platform::io::ensure_dir(&output_dir, cx.diag)?;

    let mut inputs: Vec<PathBuf> = Vec::new();
    let entries = fs::read_dir(input_dir)
        .map_err(|e| crate::platform::sheets::LoadError::Open(e.to_string()))?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                cx.diag
                    .warn(Target::Batch, format!("目录项读取失败: {e}"), None);
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .map(|x| x.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if WHITELIST.contains(&ext.as_str()) {
            inputs.push(path);
        }
    }
    inputs.sort();

    let total = inputs.len();
    let mut summary = super::BatchSummary {
        ok: Vec::new(),
        failed: Vec::new(),
    };
    for (index, path) in inputs.iter().enumerate() {
        if cx.cancelled(Progress::Files { done: index, total }) {
            return Ok(Finished::Cancelled);
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".into());
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".into());
        let source = Source {
            path: path.clone(),
            kind: match path
                .extension()
                .map(|x| x.to_string_lossy().to_ascii_lowercase())
                .as_deref()
            {
                Some("csv") => SourceKind::Csv {
                    encoding: options.encoding.clone(),
                },
                _ => SourceKind::Excel {
                    sheet: options.sheet.clone(),
                },
            },
        };
        let result = (|| -> OpResult<crate::domain::plan::Artifact> {
            let loaded = super::load_entries(&source, cx)?;
            let loaded = match loaded {
                Finished::Completed(l) => l,
                Finished::Cancelled => return Ok(Finished::Cancelled),
            };
            super::generate_deck(
                &loaded.entries,
                template,
                crate::domain::plan::OutputPlan::new(&output_dir, format!("{stem}.pptx"), policy),
                cx,
            )
        })();
        match result {
            Ok(Finished::Completed(artifact)) => {
                cx.diag.info(
                    Target::Batch,
                    "file ok",
                    Some(serde_json::json!({"name": name, field::SLIDES: artifact.slides})),
                );
                summary.ok.push((name, artifact));
            }
            Ok(Finished::Cancelled) => return Ok(Finished::Cancelled),
            Err(error) => {
                cx.diag.error(
                    Target::Batch,
                    format!("file failed: {error}"),
                    Some(serde_json::json!({"name": name})),
                );
                summary.failed.push((name, error));
            }
        }
    }
    cx.diag.info(
        Target::Batch,
        "batch done",
        Some(serde_json::json!({
            "count": summary.ok.len(),
            "total": total,
        })),
    );
    Ok(Finished::Completed(summary))
}
