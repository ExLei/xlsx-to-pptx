use crate::diag::Diag;
/// 整个 PPTX 模板的占位符扫描结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateScan {
    /// 模板包含的幻灯片数量。
    pub slide_count: usize,
    /// 所有幻灯片出现的占位符名（去重，按首次出现顺序）。
    pub placeholders: Vec<String>,
}

/// 扫描整个 PPTX 模板，收集每张幻灯片里的 `{{占位符}}` 名。
///
/// 供 GUI 在用户选定模板后即时校验：缺 `{{单词}}`、文件损坏等问题
/// 应在点击生成之前暴露，而不是生成时才失败。
pub fn scan_pptx_template(
    path: &std::path::Path,
    diag: &Diag,
) -> Result<TemplateScan, crate::types::TemplateError> {
    use std::io::Read as _;

    let template = path.display().to_string();
    let data = std::fs::read(path)?;
    let reader = std::io::Cursor::new(&data);
    let mut archive = zip::ZipArchive::new(reader).map_err(|error| {
        diag.error(
            "template",
            format!("模板不是有效的 PPTX: {error}"),
            Some(serde_json::json!({"template": template})),
        );
        crate::types::TemplateError::Corrupt(error.to_string())
    })?;

    let mut slide_count = 0usize;
    let mut placeholders: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|error| {
            diag.error(
                "template",
                format!("读取模板内容失败: {error}"),
                Some(serde_json::json!({"template": template})),
            );
            crate::types::TemplateError::Corrupt(error.to_string())
        })?;
        let name = entry.name().to_string();
        if !(name.starts_with("ppt/slides/slide") && name.ends_with(".xml")) {
            continue;
        }
        slide_count += 1;
        let mut xml = Vec::new();
        entry.read_to_end(&mut xml).map_err(|error| {
            diag.error(
                "template",
                format!("读取幻灯片 {name} 失败: {error}"),
                Some(serde_json::json!({"template": template, "slide": name})),
            );
            crate::types::TemplateError::Corrupt(error.to_string())
        })?;
        for info in scan_placeholders(&String::from_utf8_lossy(&xml), diag) {
            if !placeholders.contains(&info.name) {
                placeholders.push(info.name);
            }
        }
    }

    diag.info(
        "template",
        format!(
            "模板扫描完成：{slide_count} 张幻灯片，{} 个占位符",
            placeholders.len()
        ),
        Some(serde_json::json!({
            "template": template,
            "slides": slide_count,
            "placeholders": placeholders,
        })),
    );
    Ok(TemplateScan {
        slide_count,
        placeholders,
    })
}

/// 占位符扫描结果
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceholderInfo {
    /// 占位符名称（不含 {{ }}）
    pub name: String,
}

/// 扫描 PPTX slide XML 中的 {{占位符}} 标记
///
/// 先将所有 `<a:t>` 标签内的文本拼接，再扫描 `{{name}}` 模式。
/// 这处理了 PowerPoint 将 `{{` `单词` `}}` 拆分为多个 `<a:r><a:t>` 的情况。
pub fn scan_placeholders(xml: &str, diag: &Diag) -> Vec<PlaceholderInfo> {
    let combined = extract_at_texts(xml);
    let mut results = Vec::new();
    let bytes = combined.as_bytes();
    let mut i = 0;

    while i + 3 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let start = i + 2;
            if let Some(end) = bytes[start..]
                .windows(2)
                .position(|w| w[0] == b'}' && w[1] == b'}')
            {
                let name_bytes = &bytes[start..start + end];
                if let Ok(name) = std::str::from_utf8(name_bytes) {
                    if !name.is_empty() {
                        results.push(PlaceholderInfo {
                            name: name.to_string(),
                        });
                    }
                }
                i = start + end + 2;
                continue;
            }
        }
        i += 1;
    }

    if results.is_empty() {
        diag.warn(
            "template",
            "no {{placeholder}} markers found in slide XML",
            Some(serde_json::json!({"count": 0})),
        );
    } else {
        let names: Vec<&str> = results.iter().map(|p| p.name.as_str()).collect();
        diag.info(
            "template",
            format!("found {} placeholders: {:?}", results.len(), names),
            Some(serde_json::json!({"count": results.len(), "names": names})),
        );
    }
    results
}

/// 提取 XML 中所有 <a:t> 标签内的文本并拼接
fn extract_at_texts(xml: &str) -> String {
    let mut result = String::with_capacity(xml.len() / 2);
    let bytes = xml.as_bytes();
    let open_tag = b"<a:t";
    let close_tag = b"</a:t>";
    let close_marker = b'>';

    let mut i = 0;
    while i < bytes.len() {
        // Find <a:t ... >
        if let Some(start) = bytes[i..]
            .windows(open_tag.len())
            .position(|w| w == open_tag)
        {
            let tag_start = i + start;
            // Reject <a:tab/>, <a:tbl/>, <a:txBody/> — must be <a:t> or <a:t ...>
            if tag_start + 4 < bytes.len() {
                let next = bytes[tag_start + 4];
                if next != b'>' && next != b' ' {
                    i = tag_start + 1;
                    continue;
                }
            }
            // Skip to the > that closes the opening tag
            if let Some(end) = bytes[tag_start..].iter().position(|&b| b == close_marker) {
                let content_start = tag_start + end + 1;
                // Find </a:t>
                if let Some(close_pos) = bytes[content_start..]
                    .windows(close_tag.len())
                    .position(|w| w == close_tag)
                {
                    let content = &bytes[content_start..content_start + close_pos];
                    if let Ok(s) = std::str::from_utf8(content) {
                        result.push_str(s);
                    }
                    i = content_start + close_pos + close_tag.len();
                    continue;
                }
            }
        }
        i += 1;
    }

    result
}

pub fn validate_placeholders(
    placeholders: &[PlaceholderInfo],
    diag: &Diag,
) -> Result<(), crate::types::TemplateError> {
    let has_word = placeholders.iter().any(|p| p.name == "单词");
    if !has_word {
        diag.error(
            "template",
            "required placeholder {{单词}} missing",
            Some(serde_json::json!({"required": "单词"})),
        );
        return Err(crate::types::TemplateError::MissingPlaceholder);
    }
    diag.info(
        "template",
        "placeholder validation passed: {{单词}} present",
        Some(serde_json::json!({"required": "单词", "present": true})),
    );
    Ok(())
}
/// Replace `{{placeholder}}` markers in XML with values.
///
/// Handles PowerPoint's split `{{...}}` across `<a:t>` elements by:
/// 1. Collecting all `<a:t>` text positions
/// 2. Concatenating text and doing replacements
pub fn replace_placeholders(xml: &str, replacements: &[(&str, &str)], diag: &Diag) -> String {
    let bytes = xml.as_bytes();

    // Find <a:p>...</a:p> paragraphs, and within each:
    // collect <a:t> text regions, concatenate, replace, put back in first <a:t>
    let p_open = b"<a:p";
    let p_close = b"</a:p>";

    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        // Find next <a:p> or <a:p ...>
        if let Some(p_pos) = bytes[i..].windows(p_open.len()).position(|w| w == p_open) {
            let p_tag_start = i + p_pos;
            // Copy everything before this <a:p>
            result.extend_from_slice(&bytes[i..p_tag_start]);

            // Find closing </a:p>
            if let Some(p_end) = bytes[p_tag_start..]
                .windows(p_close.len())
                .position(|w| w == p_close)
            {
                let p_content_start = p_tag_start;
                let p_content_end = p_tag_start + p_end + p_close.len();
                let p_bytes = &bytes[p_content_start..p_content_end];

                // Process this paragraph: find <a:t> regions
                let processed = process_paragraph(p_bytes, replacements);
                result.extend_from_slice(&processed);

                i = p_content_end;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }

    let replaced: Vec<&str> = replacements.iter().map(|(from, _)| *from).collect();
    diag.info(
        "template",
        format!(
            "replace_placeholders: applied {} replacements ({:?})",
            replacements.len(),
            replaced
        ),
        Some(serde_json::json!({
            "replacements": replacements.len(),
            "markers": replaced,
        })),
    );
    String::from_utf8(result).unwrap_or_else(|_| xml.to_string())
}

/// Within a single <a:p>...</a:p>, find all <a:t> text,
/// concatenate, replace placeholders, put result in first <a:t>, empty rest.
fn process_paragraph(p_bytes: &[u8], replacements: &[(&str, &str)]) -> Vec<u8> {
    let at_open = b"<a:t";
    let at_close = b"</a:t>";

    struct AtRegion {
        start: usize,
        end: usize,
    }
    let mut regions: Vec<AtRegion> = Vec::new();
    let mut i = 0;
    while i < p_bytes.len() {
        if let Some(pos) = p_bytes[i..]
            .windows(at_open.len())
            .position(|w| w == at_open)
        {
            let tag_start = i + pos;
            if tag_start + 4 < p_bytes.len() {
                let next = p_bytes[tag_start + 4];
                if next != b'>' && next != b' ' {
                    i = tag_start + 1;
                    continue;
                }
            }
            if let Some(gt) = p_bytes[tag_start..].iter().position(|&b| b == b'>') {
                let cs = tag_start + gt + 1;
                if let Some(cp) = p_bytes[cs..]
                    .windows(at_close.len())
                    .position(|w| w == at_close)
                {
                    regions.push(AtRegion {
                        start: cs,
                        end: cs + cp,
                    });
                    i = cs + cp + at_close.len();
                    continue;
                }
            }
        }
        i += 1;
    }

    if regions.is_empty() {
        return p_bytes.to_vec();
    }

    // Concatenate text
    let mut combined = String::new();
    for r in &regions {
        if let Ok(s) = std::str::from_utf8(&p_bytes[r.start..r.end]) {
            combined.push_str(s);
        }
    }

    // Replace
    for &(from, to) in replacements {
        combined = combined.replace(from, to);
    }

    // Rebuild: put all combined text in first <a:t>, empty the rest
    let mut result = Vec::with_capacity(p_bytes.len());
    let mut bi = 0;
    let mut first = true;
    for r in &regions {
        result.extend_from_slice(&p_bytes[bi..r.start]);
        if first {
            result.extend_from_slice(combined.as_bytes());
            first = false;
        }
        bi = r.end;
    }
    result.extend_from_slice(&p_bytes[bi..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_all_six_standard_placeholders() {
        let xml = concat!(
            r#"<a:t>{{单词}}</a:t>"#,
            r#"<a:t>{{音标}}</a:t>"#,
            r#"<a:t>{{词根词缀}}</a:t>"#,
            r#"<a:t>{{例句}}</a:t>"#,
            r#"<a:t>{{例句释义}}</a:t>"#,
            r#"<a:t>{{单词释义}}</a:t>"#,
        );

        let result = scan_placeholders(xml, &Diag::new());
        assert_eq!(result.len(), 6, "should find all 6 standard placeholders");
        assert_eq!(result[0].name, "单词");
        assert_eq!(result[1].name, "音标");
        assert_eq!(result[2].name, "词根词缀");
        assert_eq!(result[3].name, "例句");
        assert_eq!(result[4].name, "例句释义");
        assert_eq!(result[5].name, "单词释义");
    }

    #[test]
    fn scan_handles_prefix_text() {
        let xml = r#"<a:t>词根词缀：{{词根词缀}}</a:t><a:t>例句：{{例句}}</a:t>"#;
        let result = scan_placeholders(xml, &Diag::new());
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "词根词缀");
        assert_eq!(result[1].name, "例句");
    }

    #[test]
    fn scan_ignores_non_placeholder_braces() {
        let xml = r#"<a:t>hello {world}</a:t><a:t>{{valid}}</a:t><a:t>plain text</a:t>"#;
        let result = scan_placeholders(xml, &Diag::new());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "valid");
    }

    #[test]
    fn scan_empty_xml_returns_empty() {
        let result = scan_placeholders("", &Diag::new());
        assert!(result.is_empty());

        let result = scan_placeholders("<a:t>no placeholders here</a:t>", &Diag::new());
        assert!(result.is_empty());
    }

    #[test]
    fn scan_placeholder_in_real_slide_xml() {
        let xml = r#"<?xml version="1.0"?><p:sld><p:sp><p:txBody><a:p><a:r><a:t>{{单词}}</a:t></a:r></a:p></p:txBody></p:sp></p:sld>"#;
        let result = scan_placeholders(xml, &Diag::new());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "单词");
    }

    #[test]
    fn scan_split_across_runs() {
        // PowerPoint often splits {{placeholder}} across <a:r> elements:
        // <a:r><a:t>{{</a:t></a:r><a:r><a:t>单词</a:t></a:r><a:r><a:t>}}</a:t></a:r>
        let xml = r#"<a:p><a:r><a:t>{{</a:t></a:r><a:r><a:t>单词</a:t></a:r><a:r><a:t>}}</a:t></a:r></a:p>"#;
        let result = scan_placeholders(xml, &Diag::new());
        assert_eq!(
            result.len(),
            1,
            "should find {{单词}} even when split across runs"
        );
        assert_eq!(result[0].name, "单词");
    }

    #[test]
    fn scan_split_multiple_placeholders() {
        let xml = concat!(
            r#"<a:p>"#,
            r#"<a:r><a:t>单词释义：</a:t></a:r>"#,
            r#"<a:r><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:t>单词释义</a:t></a:r>"#,
            r#"<a:r><a:t>}}</a:t></a:r>"#,
            r#"<a:r><a:t> / 词根词缀：</a:t></a:r>"#,
            r#"<a:r><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:t>词根词缀</a:t></a:r>"#,
            r#"<a:r><a:t>}}</a:t></a:r>"#,
            r#"</a:p>"#,
        );
        let result = scan_placeholders(xml, &Diag::new());
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "单词释义");
        assert_eq!(result[1].name, "词根词缀");
    }
    #[test]
    fn replace_simple_placeholder() {
        let xml = r#"<a:p><a:r><a:t>{{单词}}</a:t></a:r></a:p>"#;
        let result = replace_placeholders(xml, &[("{{单词}}", "apple")], &Diag::new());
        assert!(
            result.contains("apple"),
            "should contain apple, got: {result}"
        );
        assert!(!result.contains("{{"), "should not contain {{");
    }

    #[test]
    fn replace_split_across_tags() {
        // Simulates PowerPoint: {{单词}} split across <a:t> elements in one <a:p>
        let xml = r#"<a:p><a:r><a:t>{{</a:t></a:r><a:r><a:t>单词</a:t></a:r><a:r><a:t>}}</a:t></a:r></a:p>"#;
        let result = replace_placeholders(xml, &[("{{单词}}", "apple")], &Diag::new());
        assert!(
            result.contains("apple"),
            "should contain apple, got: {result}"
        );
        assert!(!result.contains("{{"), "should not contain {{");
    }

    #[test]
    fn replace_split_with_attributes() {
        // With <a:rPr> between <a:r> and <a:t>
        let xml = r#"<a:p><a:r><a:rPr sz="7200"/><a:t>{{</a:t></a:r><a:r><a:rPr/><a:t>单词</a:t></a:r><a:r><a:rPr/><a:t>}}</a:t></a:r></a:p>"#;
        let result = replace_placeholders(xml, &[("{{单词}}", "apple")], &Diag::new());
        assert!(
            result.contains("apple"),
            "should contain apple, got: {result}"
        );
        assert!(!result.contains("{{"), "should not contain {{");
    }

    #[test]
    fn replace_multiple_placeholders() {
        let xml = r#"<a:p><a:r><a:t>{{单词}}</a:t></a:r><a:r><a:t> / {{音标}}</a:t></a:r></a:p>"#;
        let result = replace_placeholders(
            xml,
            &[("{{单词}}", "hello"), ("{{音标}}", "/həˈloʊ/")],
            &Diag::new(),
        );
        assert!(result.contains("hello"), "should contain hello");
        assert!(result.contains("/həˈloʊ/"), "should contain phonetic");
        assert!(!result.contains("{{"), "should not contain {{");
    }

    #[test]
    fn replace_no_match_unchanged() {
        let xml = r#"<a:p><a:r><a:t>no placeholders here</a:t></a:r></a:p>"#;
        let result = replace_placeholders(xml, &[("{{单词}}", "apple")], &Diag::new());
        assert!(result.contains("no placeholders here"));
    }

    #[test]
    fn replace_full_slide_five_paragraphs() {
        // Full slide XML with 5 paragraphs, each having split {{...}} (PowerPoint style)
        let xml = concat!(
            r#"<p:sp><p:txBody>"#,
            // Paragraph 1: {{单词}} split
            r#"<a:p><a:pPr algn="ctr"/>"#,
            r#"<a:r><a:rPr sz="7200"/><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="7200"/><a:t>单词</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="7200"/><a:t>}}</a:t></a:r>"#,
            r#"</a:p>"#,
            // Paragraph 2: {{音标}} split
            r#"<a:p><a:pPr algn="ctr"/>"#,
            r#"<a:r><a:rPr sz="2800"/><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2800"/><a:t>音标</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2800"/><a:t>}}</a:t></a:r>"#,
            r#"</a:p>"#,
            // Paragraph 3: 词根词缀：{{词根词缀}} split
            r#"<a:p><a:pPr algn="l"/>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>词根词缀：</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>词根词缀</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>}}</a:t></a:r>"#,
            r#"</a:p>"#,
            // Paragraph 4: {{单词释义}} split
            r#"<a:p><a:pPr algn="l"/>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>单词释义：</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>单词释义</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>}}</a:t></a:r>"#,
            r#"</a:p>"#,
            // Paragraph 5: {{例句}} + {{例句释义}} split
            r#"<a:p><a:pPr algn="l"/>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>例句：</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>例句</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>}}</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>&#10;例句释义：</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>{{</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>例句释义</a:t></a:r>"#,
            r#"<a:r><a:rPr sz="2400"/><a:t>}}</a:t></a:r>"#,
            r#"</a:p>"#,
            r#"</p:txBody></p:sp>"#,
        );
        let result = replace_placeholders(
            xml,
            &[
                ("{{单词}}", "apple"),
                ("{{音标}}", "/ˈæpl/"),
                ("{{词根词缀}}", "a-pple"),
                ("{{单词释义}}", "苹果"),
                ("{{例句}}", "I eat an apple."),
                ("{{例句释义}}", "我吃苹果。"),
            ],
            &Diag::new(),
        );
        assert!(result.contains("apple"), "word missing");
        assert!(result.contains("/ˈæpl/"), "phonetic missing");
        assert!(result.contains("a-pple"), "morph missing");
        assert!(result.contains("苹果"), "definition missing");
        assert!(result.contains("I eat an apple."), "example missing");
        assert!(result.contains("我吃苹果。"), "example def missing");
        assert!(!result.contains("{{"), "placeholder remaining");
    }

    #[test]
    fn validate_accepts_all_six() {
        let ph = |n: &str| PlaceholderInfo {
            name: n.to_string(),
        };
        let placeholders = vec![
            ph("单词"),
            ph("音标"),
            ph("词根词缀"),
            ph("例句"),
            ph("例句释义"),
            ph("单词释义"),
        ];
        assert!(validate_placeholders(&placeholders, &Diag::new()).is_ok());
    }

    #[test]
    fn validate_rejects_missing_word() {
        let ph = |n: &str| PlaceholderInfo {
            name: n.to_string(),
        };
        assert!(validate_placeholders(&[ph("音标")], &Diag::new()).is_err());
    }

    #[test]
    fn validate_accepts_subset_with_word() {
        let ph = |n: &str| PlaceholderInfo {
            name: n.to_string(),
        };
        assert!(validate_placeholders(&[ph("单词"), ph("音标")], &Diag::new()).is_ok());
    }
    #[test]
    fn scan_pptx_template_reports_slides_and_placeholders() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("示例模板.pptx");
        crate::template_pptx::generate_example_pptx(&path).expect("生成示例模板");

        let scan = scan_pptx_template(&path, &Diag::new()).expect("扫描应成功");
        assert!(scan.slide_count >= 1, "至少要有一张幻灯片");
        for expected in ["单词", "音标", "词根词缀", "例句", "例句释义", "单词释义"]
        {
            assert!(
                scan.placeholders.iter().any(|name| name == expected),
                "缺少 {expected}：{:?}",
                scan.placeholders
            );
        }
    }

    #[test]
    fn scan_pptx_template_rejects_a_non_pptx_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("坏模板.pptx");
        std::fs::write(&path, b"not a zip archive").expect("写入坏文件");

        let error = scan_pptx_template(&path, &Diag::new()).expect_err("坏文件应报错");
        assert!(
            matches!(error, crate::types::TemplateError::Corrupt(_)),
            "得到 {error:?}"
        );
    }
}
