use std::path::Path;

use crate::types::TemplateError;

const TEMPLATE_BYTES: &[u8] = include_bytes!("../assets/template.xlsx");

/// 将内置模板导出到指定路径
pub fn export_template(path: &Path) -> Result<(), TemplateError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, TEMPLATE_BYTES)?;
    Ok(())
}
