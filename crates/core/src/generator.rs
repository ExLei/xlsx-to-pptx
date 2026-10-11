use crate::diag::Diag;
use crate::template_reader::{replace_placeholders, scan_placeholders};
use crate::types::{GenerateError, TemplateError, WordEntry};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

const TEMPLATE: &[u8] = include_bytes!("../assets/template.pptx");

pub fn generate(
    entries: &[WordEntry],
    output: &Path,
    progress: impl Fn(usize, usize) -> bool,
    diag: &Diag,
) -> Result<(), GenerateError> {
    diag.info(
        "generator",
        format!(
            "generating {} entries → {}",
            entries.len(),
            output.display()
        ),
        Some(serde_json::json!({
            "entries": entries.len(),
            "output": output.display().to_string(),
        })),
    );
    if entries.is_empty() {
        diag.error(
            "generator",
            "没有可生成的条目",
            Some(serde_json::json!({"entries": 0})),
        );
        return Err(GenerateError::NoEntries);
    }
    if output.exists() {
        diag.error(
            "generator",
            format!("输出文件已存在: {}", output.display()),
            Some(serde_json::json!({"output": output.display().to_string()})),
        );
        return Err(GenerateError::FileExists(
            output.to_string_lossy().into_owned(),
        ));
    }

    let total = entries.len();
    let reader = Cursor::new(TEMPLATE);
    let mut archive = ZipArchive::new(reader).map_err(|e| zip_error(diag, "ZIP 操作", e))?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| zip_error(diag, "ZIP 操作", e))?;
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|e| zip_error(diag, "ZIP 操作", e))?;
        files.push((name, data));
    }

    for (i, entry) in entries.iter().enumerate() {
        let slide_name = format!("ppt/slides/slide{}.xml", i + 1);
        if let Some((_, data)) = files.iter_mut().find(|(n, _)| n == &slide_name) {
            let xml = String::from_utf8_lossy(data).into_owned();
            let xml = xml
                .replace("__WORD__", &entry.word)
                .replace("__PHONETIC__", &entry.phonetic)
                .replace("__DEF__", &format!("单词释义：{}", entry.definition))
                .replace("__MORPH__", &format!("词根词缀：{}", entry.morphology))
                .replace(
                    "__EX__",
                    &format!(
                        "例句：{}\n例句释义：{}",
                        entry.example, entry.example_definition
                    ),
                );
            *data = xml.into_bytes();
        }
        if !progress(i + 1, total) {
            diag.warn(
                "generator",
                format!("生成已取消（{}/{}）", i + 1, total),
                Some(serde_json::json!({
                    "cancelled": true,
                    "processed": i + 1,
                    "total": total,
                })),
            );
            return Err(GenerateError::Cancelled);
        }
    }

    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = ZipWriter::new(&mut buf);
        let o = SimpleFileOptions::default();
        for (name, data) in &files {
            z.start_file(name, o)
                .map_err(|e| zip_error(diag, "ZIP 操作", e))?;
            z.write_all(data)
                .map_err(|e| zip_error(diag, "ZIP 操作", e))?;
        }
        z.finish().map_err(|e| zip_error(diag, "ZIP 操作", e))?;
    }
    ensure_parent_dir(output, diag)?;
    fs::write(output, buf.into_inner()).map_err(|error| {
        diag.error(
            "generator",
            format!("写入失败 {}: {error}", output.display()),
            Some(serde_json::json!({"output": output.display().to_string()})),
        );
        GenerateError::IoError(error)
    })?;
    diag.info(
        "generator",
        format!("done: {} slides → {}", total, output.display()),
        Some(serde_json::json!({
            "slides": total,
            "output": output.display().to_string(),
        })),
    );
    Ok(())
}

/// 从用户提供的模板 PPTX 生成词汇课件
///
/// 模板中的 `{{占位符}}` 会被替换为 WordEntry 对应字段值。
/// 模板必须包含 `{{单词}}` 占位符（必填字段）。
/// 如果条目数超过模板幻灯片数，会复制最后一张幻灯片。
pub fn generate_from_template(
    entries: &[WordEntry],
    template_path: &Path,
    output: &Path,
    progress: impl Fn(usize, usize) -> bool,
    diag: &Diag,
) -> Result<(), GenerateError> {
    diag.info(
        "generator",
        format!(
            "generate_from_template: {} entries → {}（模板 {}）",
            entries.len(),
            output.display(),
            template_path.display()
        ),
        Some(serde_json::json!({
            "entries": entries.len(),
            "output": output.display().to_string(),
            "template": template_path.display().to_string(),
        })),
    );
    if entries.is_empty() {
        diag.error(
            "generator",
            "没有可生成的条目",
            Some(serde_json::json!({"entries": 0})),
        );
        return Err(GenerateError::NoEntries);
    }
    if output.exists() {
        diag.error(
            "generator",
            format!("输出文件已存在: {}", output.display()),
            Some(serde_json::json!({"output": output.display().to_string()})),
        );
        return Err(GenerateError::FileExists(
            output.to_string_lossy().into_owned(),
        ));
    }
    if !template_path.exists() {
        diag.error(
            "generator",
            format!("模板文件不存在: {}", template_path.display()),
            Some(serde_json::json!({"template": template_path.display().to_string()})),
        );
        return Err(GenerateError::PptxError(format!(
            "模板文件不存在: {}",
            template_path.display()
        )));
    }

    let total = entries.len();
    let template_data = fs::read(template_path)?;
    let reader = Cursor::new(&template_data);
    let mut archive = ZipArchive::new(reader).map_err(|e| zip_error(diag, "ZIP 操作", e))?;

    // Extract all files from the template ZIP
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut slide_indices: Vec<usize> = Vec::new(); // indices into `files` for slide XMLs
    let mut slide_names: Vec<String> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| zip_error(diag, "ZIP 操作", e))?;
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|e| zip_error(diag, "ZIP 操作", e))?;

        if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
            slide_indices.push(files.len());
            slide_names.push(name.clone());
        }
        files.push((name, data));
    }

    if slide_indices.is_empty() {
        diag.error(
            "generator",
            "模板不包含任何幻灯片",
            Some(serde_json::json!({"template": template_path.display().to_string()})),
        );
        return Err(GenerateError::PptxError("模板不包含任何幻灯片".to_string()));
    }

    // Validate that at least one slide has {{单词}} placeholder
    let mut has_word = false;
    for &si in &slide_indices {
        let (_, data) = &files[si];
        let xml = String::from_utf8_lossy(data);
        let placeholders = scan_placeholders(&xml, diag);
        if placeholders.iter().any(|p| p.name == "单词") {
            has_word = true;
            break;
        }
    }
    if !has_word {
        diag.error(
            "template",
            "required placeholder {{单词}} missing",
            Some(serde_json::json!({"required": "单词"})),
        );
        return Err(TemplateError::MissingPlaceholder.into());
    }

    // Build placeholder → value mapping
    let placeholder_map: Vec<(&str, &dyn Fn(&WordEntry) -> String)> = vec![
        ("{{单词}}", &|e: &WordEntry| e.word.clone()),
        ("{{音标}}", &|e: &WordEntry| e.phonetic.clone()),
        ("{{词根词缀}}", &|e: &WordEntry| e.morphology.clone()),
        ("{{例句}}", &|e: &WordEntry| e.example.clone()),
        ("{{例句释义}}", &|e: &WordEntry| {
            e.example_definition.clone()
        }),
        ("{{单词释义}}", &|e: &WordEntry| e.definition.clone()),
    ];

    // Find the max existing rId in presentation.xml.rels for unique numbering
    let mut max_rid: u32 = 0;
    if let Some((_, rels_data)) = files
        .iter()
        .find(|(n, _)| n == "ppt/_rels/presentation.xml.rels")
    {
        let rels_xml = String::from_utf8_lossy(rels_data);
        for cap in rels_xml.match_indices("rId") {
            let after = &rels_xml[cap.0 + 3..];
            if let Some(end) = after.find(|c: char| !c.is_ascii_digit()) {
                if let Ok(n) = after[..end].parse::<u32>() {
                    max_rid = max_rid.max(n);
                }
            }
        }
    }

    // Cache the LAST template slide's original data for reuse.
    let last_tpl_idx = slide_indices[slide_indices.len() - 1];
    let last_tpl_original = files[last_tpl_idx].1.clone();

    // Cache the slide1 rels template for new slides
    let slide_rels_template = files
        .iter()
        .find(|(n, _)| n == "ppt/slides/_rels/slide1.xml.rels")
        .map(|(_, d)| d.clone());
    let template_slide_count = slide_indices.len();
    if total > template_slide_count {
        diag.info(
            "generator",
            format!(
                "条目数({total})超过模板幻灯片数({template_slide_count})，将复制最后一张幻灯片填充剩余 {extra} 页",
                extra = total - template_slide_count,
            ),
            Some(serde_json::json!({
                "entries": total,
                "template_slides": template_slide_count,
                "extra": total - template_slide_count,
            })),
        );
    }

    // Process each entry
    for (i, word_entry) in entries.iter().enumerate() {
        // For slides within template count: use the corresponding template slide.
        // For extras: clone from the last template slide (always fresh original).
        let tpl_xml = if i < slide_indices.len() {
            let si = slide_indices[i];
            String::from_utf8_lossy(&files[si].1).into_owned()
        } else {
            String::from_utf8_lossy(&last_tpl_original).into_owned()
        };
        // Replace all placeholders using cross-tag aware replacer
        // (handles PowerPoint's split {{...}} across <a:t> elements)
        let replacements: Vec<(&str, String)> = placeholder_map
            .iter()
            .map(|(ph, getter)| (*ph, getter(word_entry)))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let replacement_refs: Vec<(&str, &str)> = replacements
            .iter()
            .map(|(ph, v)| (*ph, v.as_str()))
            .collect();
        let new_xml = replace_placeholders(&tpl_xml, &replacement_refs, diag);

        let slide_name = format!("ppt/slides/slide{}.xml", i + 1);

        // If this slide already exists in files (first N entries where N <= template slides count),
        // update it in place. Otherwise, append a new entry.
        if i < slide_indices.len() {
            let si = slide_indices[i];
            files[si].1 = new_xml.into_bytes();
        } else {
            files.push((slide_name.clone(), new_xml.into_bytes()));

            // Compute unique rId
            max_rid += 1;
            let rid = format!("rId{}", max_rid);
            let sid = 256 + i as u32;

            // Create slide-level rels file (clone from slide1.rels)
            if let Some(ref rels_tpl) = slide_rels_template {
                let rels_name = format!("ppt/slides/_rels/slide{}.xml.rels", i + 1);
                if !files.iter().any(|(n, _)| n == &rels_name) {
                    files.push((rels_name.clone(), rels_tpl.clone()));
                    if let Some((_, ct_data)) =
                        files.iter_mut().find(|(n, _)| n == "[Content_Types].xml")
                    {
                        let ct_xml = String::from_utf8_lossy(ct_data).into_owned();
                        let rels_part = format!("/ppt/slides/_rels/slide{}.xml.rels", i + 1);
                        if !ct_xml.contains(&rels_part) {
                            let tag = format!(
                                r#"<Override PartName="{}" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
                                rels_part
                            );
                            let idx = ct_xml.rfind("</Types>").unwrap_or(ct_xml.len());
                            let mut new_ct = ct_xml[..idx].to_string();
                            new_ct.push_str(&tag);
                            new_ct.push_str(&ct_xml[idx..]);
                            *ct_data = new_ct.into_bytes();
                        }
                    }
                }
            }

            // Update [Content_Types].xml for the slide itself
            if let Some((_, ct_data)) = files.iter_mut().find(|(n, _)| n == "[Content_Types].xml") {
                let ct_xml = String::from_utf8_lossy(ct_data).into_owned();
                let slide_part = format!("/ppt/slides/slide{}.xml", i + 1);
                if !ct_xml.contains(&slide_part) {
                    let tag = format!(
                        r#"<Override PartName="{}" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#,
                        slide_part
                    );
                    let idx = ct_xml.rfind("</Types>").unwrap_or(ct_xml.len());
                    let mut new_ct = ct_xml[..idx].to_string();
                    new_ct.push_str(&tag);
                    new_ct.push_str(&ct_xml[idx..]);
                    *ct_data = new_ct.into_bytes();
                }
            }

            // Update ppt/presentation.xml
            if let Some((_, pres_data)) =
                files.iter_mut().find(|(n, _)| n == "ppt/presentation.xml")
            {
                let pres_xml = String::from_utf8_lossy(pres_data).into_owned();
                let sld_tag = format!(r#"<p:sldId id="{}" r:id="{}"/>"#, sid, rid);
                let mut new_pres = pres_xml;
                if !new_pres.contains(&sld_tag) {
                    let idx = new_pres.rfind("</p:sldIdLst>").unwrap_or(new_pres.len());
                    new_pres.insert_str(idx, &sld_tag);
                    *pres_data = new_pres.into_bytes();
                }
            }

            // Update ppt/_rels/presentation.xml.rels
            if let Some((_, rels_data)) = files
                .iter_mut()
                .find(|(n, _)| n == "ppt/_rels/presentation.xml.rels")
            {
                let rels_xml = String::from_utf8_lossy(rels_data).into_owned();
                let rel_tag = format!(
                    r#"<Relationship Id="{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>"#,
                    rid,
                    i + 1
                );
                if !rels_xml.contains(&format!("Target=\"slides/slide{}.xml\"", i + 1)) {
                    let idx = rels_xml.rfind("</Relationships>").unwrap_or(rels_xml.len());
                    let mut new_rels = rels_xml[..idx].to_string();
                    new_rels.push_str(&rel_tag);
                    new_rels.push_str(&rels_xml[idx..]);
                    *rels_data = new_rels.into_bytes();
                }
            }
        }

        if !progress(i + 1, total) {
            diag.warn(
                "generator",
                format!("生成已取消（{}/{}）", i + 1, total),
                Some(serde_json::json!({
                    "cancelled": true,
                    "processed": i + 1,
                    "total": total,
                })),
            );
            return Err(GenerateError::Cancelled);
        }
    }

    // Write output ZIP
    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = ZipWriter::new(&mut buf);
        let o = SimpleFileOptions::default();
        for (name, data) in &files {
            z.start_file(name, o)
                .map_err(|e| zip_error(diag, "ZIP 操作", e))?;
            z.write_all(data)
                .map_err(|e| zip_error(diag, "ZIP 操作", e))?;
        }
        z.finish().map_err(|e| zip_error(diag, "ZIP 操作", e))?;
    }
    ensure_parent_dir(output, diag)?;
    fs::write(output, buf.into_inner()).map_err(|error| {
        diag.error(
            "generator",
            format!("写入失败 {}: {error}", output.display()),
            Some(serde_json::json!({"output": output.display().to_string()})),
        );
        GenerateError::IoError(error)
    })?;
    diag.info(
        "generator",
        format!("done: {} slides → {}", total, output.display()),
        Some(serde_json::json!({
            "slides": total,
            "output": output.display().to_string(),
        })),
    );
    Ok(())
}

/// 记录并包装 ZIP 错误。
fn zip_error(diag: &Diag, context: &str, error: impl std::fmt::Display) -> GenerateError {
    diag.error(
        "generator",
        format!("{context}: {error}"),
        Some(serde_json::json!({"context": context})),
    );
    GenerateError::PptxError(format!("ZIP: {error}"))
}

/// 输出文件所在目录不存在时创建；生成器让自己写出的文件可落盘。
fn ensure_parent_dir(output: &Path, diag: &Diag) -> Result<(), GenerateError> {
    let Some(parent) = output.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(parent).map_err(|error| {
        diag.error(
            "generator",
            format!("创建输出目录失败 {}: {error}", parent.display()),
            Some(serde_json::json!({"output_dir": parent.display().to_string()})),
        );
        GenerateError::IoError(error)
    })
}
