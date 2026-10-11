use vocab_core::png_export;
use vocab_core::types::WordEntry;

// ── Red: SVG generation tests ──

#[test]
fn svg_skips_empty_fields() {
    let entry = WordEntry {
        word: "test".into(),
        phonetic: String::new(),
        morphology: "test-morph".into(),
        example: String::new(),
        example_definition: String::new(),
        definition: String::new(),
    };
    let svg = png_export::render_slide_to_svg(&entry);
    assert!(svg.contains("test"), "SVG should contain word");
    assert!(svg.contains("test-morph"), "SVG should contain morphology");
    assert_eq!(
        png_export::count_text_nodes(&svg),
        2,
        "only the two filled fields should render"
    );
}

#[test]
fn svg_generation_produces_valid_xml() {
    let entry = WordEntry {
        word: "apple".into(),
        phonetic: "/ˈæpl/".into(),
        morphology: String::new(),
        example: String::new(),
        example_definition: String::new(),
        definition: String::new(),
    };

    let svg = png_export::render_slide_to_svg(&entry);

    // Must be valid SVG XML
    assert!(
        svg.starts_with("<svg"),
        "SVG should start with <svg> tag, got: {svg:.100}"
    );
    assert!(svg.contains("</svg>"), "SVG should close with </svg>");
    assert!(svg.contains("apple"), "SVG should contain the word text");
    assert!(svg.contains("/ˈæpl/"), "SVG should contain phonetic text");
    // Must have xmlns and viewBox
    assert!(svg.contains("xmlns="), "SVG should have xmlns attribute");
    assert!(
        svg.contains("viewBox="),
        "SVG should have viewBox attribute"
    );
}

#[test]
fn svg_handles_chinese_characters() {
    let entry = WordEntry {
        word: "苹果".into(),
        phonetic: String::new(),
        morphology: String::new(),
        example: "我每天吃一个苹果。".into(),
        example_definition: String::new(),
        definition: "一种水果".into(),
    };

    let svg = png_export::render_slide_to_svg(&entry);

    // All Chinese characters should be present in the SVG
    assert!(svg.contains("苹果"), "SVG should contain Chinese word");
    assert!(
        svg.contains("我每天吃一个苹果"),
        "SVG should contain Chinese example"
    );
    assert!(
        svg.contains("一种水果"),
        "SVG should contain Chinese definition"
    );
}

#[test]
fn svg_handles_ipa_characters() {
    let entry = WordEntry {
        word: "pronunciation".into(),
        phonetic: "/prəˌnʌnsiˈeɪʃən/".into(),
        morphology: String::new(),
        example: String::new(),
        example_definition: String::new(),
        definition: String::new(),
    };

    let svg = png_export::render_slide_to_svg(&entry);

    // IPA characters (ə, ʌ, ʃ) should be in SVG
    assert!(svg.contains("ə"), "SVG should contain IPA schwa character");
    assert!(svg.contains("ʌ"), "SVG should contain IPA character");
    assert!(svg.contains("ʃ"), "SVG should contain IPA esh character");
}

// ── Red: PNG rendering tests ──

fn make_test_svg() -> String {
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width="1920" height="1080">
  <rect width="1920" height="1080" fill="#ffffff"/>
  <text x="80" y="180" font-family="sans-serif" font-size="72" fill="#1e1e1e">测试文字</text>
</svg>"##
        .to_string()
}

// ── Font probe tests ──

#[test]
fn probe_fonts_returns_non_empty() {
    let diag = vocab_core::diag::Diag::new();
    let cfg = vocab_core::png_export::pipeline::probe_fonts(&diag);
    assert!(
        !cfg.font_stack.is_empty(),
        "probe_fonts must return a font_stack"
    );
}

#[test]
fn probe_fonts_emits_diag_events() {
    let sink = vocab_core::diag::MemorySink::new();
    let diag = vocab_core::diag::Diag::with_sinks(vec![Box::new(sink.clone())]);
    let _cfg = vocab_core::png_export::pipeline::probe_fonts(&diag);
    assert!(diag.event_count() > 0, "probe_fonts must emit diag events");

    let events = sink.events();
    assert!(
        events.iter().all(|event| event.target == "font_probe"),
        "probe_fonts 的事件必须带 font_probe target"
    );
    assert!(
        events
            .iter()
            .any(|event| event.fields.as_ref().and_then(|f| f.get("chain")).is_some()),
        "font 链事件必须带结构化 fields"
    );
}

// probe_fonts now returns a single FontConfig directly (no first_available needed)

#[test]
fn probe_fonts_returns_valid_config() {
    let diag = vocab_core::diag::Diag::new();
    let cfg = vocab_core::png_export::pipeline::probe_fonts(&diag);
    assert!(!cfg.font_stack.is_empty());
    assert!(cfg.font_stack.contains("sans-serif"));
}

// ── count_text_nodes tests ──

#[test]
fn count_text_nodes_counts_text_elements() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
  <text x="10" y="20">hello</text>
  <text x="10" y="40">world</text>
  <rect width="100" height="100"/>
  <text x="30" y="60">third</text>
</svg>"##;
    let count = vocab_core::png_export::count_text_nodes(svg);
    assert_eq!(count, 3, "should count 3 text elements");
}

#[test]
fn count_text_nodes_returns_zero_for_rect_only() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
  <rect width="1920" height="1080" fill="#ffffff"/>
</svg>"##;
    let count = vocab_core::png_export::count_text_nodes(svg);
    assert_eq!(count, 0, "should return 0 when no text elements");
}

// ── count_non_white_pixels tests ──

fn make_all_white_png() -> Vec<u8> {
    use image::{ImageBuffer, Rgb};
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(100, 50, Rgb([255, 255, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

#[test]
fn count_non_white_pixels_all_white_returns_zero() {
    let png = make_all_white_png();
    let count = vocab_core::png_export::count_non_white_pixels(&png).expect("should decode PNG");
    assert_eq!(count, 0, "all-white PNG should have 0 non-white pixels");
}

#[test]
fn count_non_white_pixels_counts_colored_pixels() {
    use image::{ImageBuffer, Rgb};
    let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(10, 10, Rgb([255, 255, 255]));
    // Set top-left corner to black
    img.put_pixel(0, 0, Rgb([0, 0, 0]));
    img.put_pixel(1, 0, Rgb([128, 128, 128]));
    // Set rightmost pixel to near-white (not exactly white)
    img.put_pixel(9, 9, Rgb([254, 255, 255]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    let png = buf.into_inner();
    let count = vocab_core::png_export::count_non_white_pixels(&png).expect("should decode PNG");
    assert_eq!(
        count, 3,
        "should count 3 non-white pixels (including near-white)"
    );
}

// ── Updated render_svg_to_png tests with FontConfig ──

fn make_test_font_config() -> vocab_core::png_export::pipeline::FontConfig {
    let diag = vocab_core::diag::Diag::new();
    vocab_core::png_export::pipeline::probe_fonts(&diag)
}

#[test]
fn render_svg_to_png_produces_valid_png_bytes() {
    let svg = make_test_svg();
    let font_cfg = make_test_font_config();
    let diag = vocab_core::diag::Diag::new();
    let png_bytes = vocab_core::png_export::render_svg_to_png(&svg, &font_cfg, &diag)
        .expect("should render SVG to PNG");

    assert_eq!(
        &png_bytes[0..8],
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        "output should start with PNG magic bytes"
    );
    assert!(
        png_bytes.len() > 1000,
        "PNG should be non-trivial size, got {}",
        png_bytes.len()
    );
}

#[test]
fn render_svg_to_png_has_correct_dimensions() {
    let svg = make_test_svg();
    let font_cfg = make_test_font_config();
    let diag = vocab_core::diag::Diag::new();
    let png_bytes = vocab_core::png_export::render_svg_to_png(&svg, &font_cfg, &diag).unwrap();

    let img = image::load_from_memory(&png_bytes).expect("should decode PNG");
    assert_eq!(img.width(), 1920, "PNG width should be 1920");
    assert_eq!(img.height(), 1080, "PNG height should be 1080");
}

// ── Blank slide detection tests ──

#[test]
fn blank_slide_detection_density_below_001_pct_with_text_emits_error() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width="1920" height="1080">
  <rect width="1920" height="1080" fill="#ffffff"/>
  <text x="80" y="180" font-family="sans-serif" font-size="72" fill="#ffffff">invisible text</text>
</svg>"##;
    let font_cfg = make_test_font_config();
    let sink = vocab_core::diag::MemorySink::new();
    let diag = vocab_core::diag::Diag::with_sinks(vec![Box::new(sink.clone())]);
    let png =
        vocab_core::png_export::render_svg_to_png(&svg, &font_cfg, &diag).expect("should render");

    let text_nodes = vocab_core::png_export::count_text_nodes(&svg);
    let non_white = vocab_core::png_export::count_non_white_pixels(&png).unwrap();
    assert!(text_nodes > 0, "SVG should have text nodes");
    assert!(
        non_white < 100,
        "near-zero density: {non_white} non-white pixels"
    );

    vocab_core::png_export::verify_slide(&diag, 3, &svg, &png);

    let events = sink.events();
    let blank = events
        .iter()
        .find(|event| event.reason() == Some("blank"))
        .expect("应上报 reason=blank 的空白 slide");
    assert_eq!(blank.target, "verify");
    assert_eq!(blank.slide_index(), Some(3));
    assert_eq!(blank.level, vocab_core::diag::DiagLevel::Error);
}

// ── 默认版式：与 template-pptx 示例模板同尺度（1920×1080 画布） ──

/// 六个字段齐全的条目，用于比对整版坐标。
fn make_full_entry() -> WordEntry {
    WordEntry {
        word: "apple".into(),
        phonetic: "/ˈæpl/".into(),
        morphology: "a-pple".into(),
        example: "I eat an apple every day.".into(),
        example_definition: "我每天吃一个苹果。".into(),
        definition: "苹果".into(),
    }
}

#[test]
fn svg_default_layout_matches_example_template_geometry() {
    let entry = make_full_entry();
    let svg = png_export::render_slide_to_svg(&entry);

    // 单词：画布水平居中，72pt → 120px，基线 = 文本框顶 144 + 约 0.8em
    assert!(
        svg.contains(r#"x="960" y="240" text-anchor="middle""#),
        "单词 should be centered on the canvas, got: {svg}"
    );
    assert!(
        svg.contains(r#"font-size="120""#),
        "单词 should render at 72pt → 120px, got: {svg}"
    );
    // 音标：居中，28pt → 46.7px
    assert!(
        svg.contains(r#"font-size="46.7""#),
        "音标 should render at 28pt → 46.7px, got: {svg}"
    );

    // 标签列：左起 240px，正文 24pt → 40px，行距与模板文本框一致
    for (baseline, label) in [
        (452, "单词释义"),
        (548, "词根词缀"),
        (644, "例句"),
        (692, "例句释义"),
    ] {
        assert!(
            svg.contains(&format!(r#"x="240" y="{baseline}""#)),
            "{label} should sit at x=240 y={baseline}, got: {svg}"
        );
    }
    assert!(
        svg.contains(r#"font-size="40""#),
        "标签行应为 24pt → 40px, got: {svg}"
    );
}

// ── 导出事件流：export 生命周期 + 逐 slide 事件（fields 驱动查询） ──

#[test]
fn export_emits_lifecycle_and_per_slide_events() {
    use vocab_core::diag::{Diag, MemorySink};

    let dir = tempfile::tempdir().unwrap();
    let entry = WordEntry {
        word: "apple".into(),
        phonetic: "/ˈæpl/".into(),
        morphology: String::new(),
        example: String::new(),
        example_definition: String::new(),
        definition: "苹果".into(),
    };

    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
    let pngs = png_export::export_entries_to_png(&[entry], dir.path(), None, &diag)
        .expect("export should succeed");
    assert_eq!(pngs.len(), 1);

    let events = sink.events();
    assert!(
        events
            .iter()
            .any(|event| event.target == "export" && event.message.contains("导出完成")),
        "应有一条 export 完成事件: {:?}",
        events
            .iter()
            .map(|event| (&event.target, &event.message))
            .collect::<Vec<_>>()
    );

    let slide = events
        .iter()
        .find(|event| event.target == "slide" && event.slide_index() == Some(1))
        .expect("应有一条 fields.slide == 1 的逐 slide 事件");
    assert!(
        slide.fields.as_ref().unwrap().get("path").is_some(),
        "逐 slide 事件应带 path 字段"
    );
}
