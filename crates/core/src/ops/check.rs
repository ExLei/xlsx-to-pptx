//! 模板校验用例：扫描占位符 + `{{单词}}` 必填判定。

use std::path::Path;

use super::{Finished, OpContext, OpResult, TemplateCheck};
use crate::diag::Target;
use crate::domain::schema::Field;
use crate::platform::pptx::{Package, TokenScheme};

/// 校验模板：打开包 → 扫描占位符 → 判定 `{{单词}}` 是否存在。
pub fn check_template(template: &Path, cx: &mut OpContext) -> OpResult<TemplateCheck> {
    cx.diag.info(
        Target::Template,
        "check template",
        Some(serde_json::json!({"path": template.display().to_string()})),
    );
    if cx.cancelled(crate::ports::Progress::Stage("校验模板")) {
        return Ok(Finished::Cancelled);
    }
    let package = Package::open(template, cx.diag)?;
    let slides = package.slide_count();
    let fields = package.scan_tokens(TokenScheme::Placeholders);
    let missing_word = !fields.contains(&Field::Word);
    cx.diag.info(
        Target::Template,
        "template checked",
        Some(serde_json::json!({
            "slides": slides,
            "count": fields.len(),
            "missing": if missing_word { vec!["{{单词}}"] } else { vec![] },
        })),
    );
    Ok(Finished::Completed(TemplateCheck {
        slides,
        fields,
        missing_word,
    }))
}
