//! 模板用例：示例 PPTX 模板 / Excel 词表模板。

use super::{Finished, OpContext, OpResult};
use crate::diag::Target;
use crate::domain::plan::{Artifact, OutputPlan};
use crate::ports::Progress;

/// 生成示例 PPTX 模板：内嵌版式填充为 `{{占位符}}` 形态，供用户编辑。
pub fn make_example_template(output: OutputPlan, cx: &mut OpContext) -> OpResult<Artifact> {
    if cx.cancelled(Progress::Stage("生成示例模板")) {
        return Ok(Finished::Cancelled);
    }
    let path = output.dir.join(&output.name);
    cx.diag.info(
        Target::Template,
        "make example template",
        Some(serde_json::json!({"path": path.display().to_string()})),
    );
    let bytes = examples::example_template_bytes(cx.diag)?;
    crate::platform::io::write_atomic(&path, &bytes, cx.diag)?;
    Ok(Finished::Completed(Artifact { path, slides: 1 }))
}

/// 生成 Excel 词表模板：表头来自字段表。
pub fn make_excel_template(output: OutputPlan, cx: &mut OpContext) -> OpResult<Artifact> {
    if cx.cancelled(Progress::Stage("生成词表模板")) {
        return Ok(Finished::Cancelled);
    }
    let path = output.dir.join(&output.name);
    cx.diag.info(
        Target::Template,
        "make excel template",
        Some(serde_json::json!({"path": path.display().to_string()})),
    );
    let bytes = examples::excel_template_bytes();
    crate::platform::io::write_atomic(&path, &bytes, cx.diag)?;
    Ok(Finished::Completed(Artifact { path, slides: 1 }))
}

/// 内置示例产物的字节构造（Excel 模板为静态资产；PPTX 模板由 `pptx` 包生成）。
mod examples {
    use crate::diag::Diag;
    use crate::platform::pptx::PptxError;

    /// 示例 PPTX：内嵌版式 → 标记替换为 `{{占位符}}`。
    pub(super) fn example_template_bytes(_diag: &Diag) -> Result<Vec<u8>, PptxError> {
        unimplemented!("M1：由 pptx::Package 将内嵌标记替换为 {{占位符}} 后导出")
    }

    /// Excel 词表模板：静态资产（表头与字段表一致）。
    pub(super) fn excel_template_bytes() -> Vec<u8> {
        include_bytes!("../../assets/词表模板.xlsx").to_vec()
    }
}
