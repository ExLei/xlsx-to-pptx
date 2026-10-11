use vocab_core::diag::Diag;
use vocab_core::template_reader::scan_placeholders;

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
    // Realistic slide XML fragment with {{单词}} in <a:t>
    let xml = r#"<?xml version="1.0"?><p:sld><p:sp><p:txBody><a:p><a:r><a:t>{{单词}}</a:t></a:r></a:p></p:txBody></p:sp></p:sld>"#;
    let result = scan_placeholders(xml, &Diag::new());
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "单词");
}

// ── validate_placeholders ──

#[test]
fn validate_accepts_all_six_placeholders() {
    use vocab_core::template_reader::validate_placeholders;

    let placeholders = vec![
        template_reader_ph("单词"),
        template_reader_ph("音标"),
        template_reader_ph("词根词缀"),
        template_reader_ph("例句"),
        template_reader_ph("例句释义"),
        template_reader_ph("单词释义"),
    ];
    assert!(validate_placeholders(&placeholders, &Diag::new()).is_ok());
}

#[test]
fn validate_rejects_missing_word() {
    use vocab_core::template_reader::validate_placeholders;

    let placeholders = vec![template_reader_ph("音标"), template_reader_ph("例句")];
    assert!(validate_placeholders(&placeholders, &Diag::new()).is_err());
}

#[test]
fn validate_accepts_subset_with_word() {
    use vocab_core::template_reader::validate_placeholders;

    // Just 单词 + 音标; should be fine
    let placeholders = vec![template_reader_ph("单词"), template_reader_ph("音标")];
    assert!(validate_placeholders(&placeholders, &Diag::new()).is_ok());
}

#[test]
fn validate_accepts_empty() {
    use vocab_core::template_reader::validate_placeholders;

    assert!(validate_placeholders(&[], &Diag::new()).is_err());
}

fn template_reader_ph(name: &str) -> vocab_core::template_reader::PlaceholderInfo {
    vocab_core::template_reader::PlaceholderInfo {
        name: name.to_string(),
    }
}
