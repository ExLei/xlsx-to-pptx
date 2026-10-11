use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

/// 内嵌的 v1 模板 PPTX（PowerPoint 验证过的 OOXML 结构）
const V1_TEMPLATE: &[u8] = include_bytes!("../assets/template.pptx");

/// 生成包含 6 个标准占位符的示例模板 PPTX
///
/// 直接克隆 v1 内嵌模板的完整 OOXML 结构，
/// 仅替换幻灯片中的 `__WORD__` 等标记为 `{{占位符}}`，
/// 保证结构 100% 合规，无需 PowerPoint 修复。
pub fn generate_example_pptx(output: &Path) -> Result<(), std::io::Error> {
    let reader = Cursor::new(V1_TEMPLATE);
    let mut archive = ZipArchive::new(reader)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("ZIP: {e}")))?;

    // Collect all entries
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("ZIP: {e}"))
        })?;
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        files.push((name, data));
    }

    // Replace slide XMLs: __PLACEHOLDER__ → {{中文占位符}}
    for (name, data) in &mut files {
        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            let xml = String::from_utf8_lossy(data).into_owned();
            let xml = xml
                .replace("__WORD__", "{{单词}}")
                .replace("__PHONETIC__", "{{音标}}")
                .replace("__DEF__", "单词释义：{{单词释义}}")
                .replace("__MORPH__", "词根词缀：{{词根词缀}}")
                .replace("__EX__", "例句：{{例句}}\n例句释义：{{例句释义}}");
            *data = xml.into_bytes();
        }
    }

    // Write output
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file = fs::File::create(output)?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default();
    for (name, data) in &files {
        zip.start_file(name, opts)
            .map_err(|e| std::io::Error::other(format!("ZIP: {e}")))?;
        zip.write_all(data)?;
    }
    zip.finish()
        .map_err(|e| std::io::Error::other(format!("ZIP: {e}")))?;

    Ok(())
}
