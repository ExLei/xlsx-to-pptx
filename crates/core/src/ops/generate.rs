//! 生成用例：词条 → PPTX。

use std::path::Path;

use super::{Finished, OpContext, OpResult};
use crate::diag::{field, Target};
use crate::domain::plan::{Artifact, OutputPlan};
use crate::domain::schema::WordEntry;
use crate::platform::pptx::{Package, TokenScheme, EMBEDDED_TEMPLATE};

/// 生成 PPTX：
/// 1. 打开版式（`template = None` → 内嵌资产）；
/// 2. 模板模式先校验 `{{单词}}` 必填；
/// 3. 逐页填充（观察者进度）；
/// 4. 原子落盘 + 结构自检（由 `Package::save_atomic` 承担）。
pub fn generate_deck(
    entries: &[WordEntry],
    template: Option<&Path>,
    output: OutputPlan,
    cx: &mut OpContext,
) -> OpResult<Artifact> {
    let target = output.dir.join(&output.name);
    cx.diag.info(
        Target::Pptx,
        "generate start",
        Some(serde_json::json!({
            "entries": entries.len(),
            "path": target.display().to_string(),
        })),
    );
    if entries.is_empty() {
        return Err(crate::ops::OpError::NoEntries);
    }

    let mut package = match template {
        Some(path) => {
            let p = Package::open(path, cx.diag)?;
            let fields = p.scan_tokens(TokenScheme::Placeholders);
            if !fields.contains(&crate::domain::schema::Field::Word) {
                cx.diag.error(
                    Target::Template,
                    "模板缺少必填占位符: {{单词}}",
                    Some(serde_json::json!({"path": path.display().to_string()})),
                );
                return Err(crate::platform::pptx::PptxError::MissingStructure("{{单词}}").into());
            }
            p
        }
        None => Package::open_bytes(EMBEDDED_TEMPLATE, cx.diag)?,
    };

    let scheme = if template.is_some() {
        TokenScheme::Placeholders
    } else {
        TokenScheme::Markers
    };
    match package.fill(entries, scheme, cx.observer, cx.diag)? {
        std::ops::ControlFlow::Continue(()) => {}
        std::ops::ControlFlow::Break(()) => return Ok(Finished::Cancelled),
    }

    let artifact = package.save_atomic(&output, cx.diag)?;
    cx.diag.info(
        Target::Pptx,
        "generate done",
        Some(serde_json::json!({"path": artifact.path.display().to_string(), field::SLIDES: artifact.slides})),
    );
    Ok(Finished::Completed(artifact))
}
