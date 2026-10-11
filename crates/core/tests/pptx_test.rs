//! `pptx::Package` 的契约测试（design §6.1 / §12.2）：
//! 打开校验、标记扫描（含跨 run）、填充与转义、复制末页的结构修补、原子落盘与取消。
//!
//! fixture 全部在测试进程内用 `zip` 构造（最小合法 pptx），不依赖外部文件。

use std::fs;
use std::io::{Cursor, Read, Write};
use std::ops::ControlFlow;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;
use x2p_core::diag::{Diag, DiagSink, Level, MemorySink};
use x2p_core::domain::plan::{OutputError, OutputPlan, OverwritePolicy};
use x2p_core::domain::schema::{Field, WordEntry};
use x2p_core::platform::pptx::{Package, PptxError, TokenScheme, EMBEDDED_TEMPLATE};
use x2p_core::ports::{OpObserver, Progress};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const P_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_SLIDE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";

const SLIDE_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/></Relationships>"#;

// ── 测试替身与 fixture ───────────────────────────────────────────────────

fn diagnostics() -> (Diag, MemorySink) {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone()) as Box<dyn DiagSink>]);
    (diag, sink)
}

fn entry(word: &str) -> WordEntry {
    WordEntry {
        word: word.to_string(),
        phonetic: format!("/{word}/"),
        morphology: format!("{word}-root"),
        example: format!("I like {word}."),
        example_definition: format!("我喜欢{word}。"),
        definition: format!("n. {word}"),
    }
}

/// 记录进度、可编程取消的观察者。
struct Recording {
    progress: Vec<Progress>,
    break_after: Option<usize>,
}

impl Recording {
    fn passive() -> Recording {
        Recording {
            progress: Vec::new(),
            break_after: None,
        }
    }

    fn breaking_after(limit: usize) -> Recording {
        Recording {
            progress: Vec::new(),
            break_after: Some(limit),
        }
    }
}

impl OpObserver for Recording {
    fn on(&mut self, progress: &Progress) -> ControlFlow<()> {
        self.progress.push(progress.clone());
        match self.break_after {
            Some(limit) if self.progress.len() >= limit => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        }
    }
}

/// 幻灯片 XML 外壳：段落由调用方给出。
fn slide_xml_raw(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:a="{A_NS}" xmlns:r="{R_NS}" xmlns:p="{P_NS}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="文本框 1"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
    )
}

/// 一页幻灯片：每段一个 `<a:t>`。
fn slide_xml(paragraphs: &[&str]) -> String {
    let body: String = paragraphs
        .iter()
        .map(|text| format!("<a:p><a:r><a:t>{text}</a:t></a:r></a:p>"))
        .collect();
    slide_xml_raw(&body)
}

/// 最小合法 pptx：包结构 + 调用方给的幻灯片/rels。
struct Fixture {
    slides: Vec<(u32, String)>,
    rels: Vec<(u32, String)>,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            slides: Vec::new(),
            rels: Vec::new(),
        }
    }

    fn slide(mut self, number: u32, xml: &str) -> Fixture {
        self.slides.push((number, xml.to_string()));
        self
    }

    fn slide_rels(mut self, number: u32, xml: &str) -> Fixture {
        self.rels.push((number, xml.to_string()));
        self
    }

    fn build(self) -> Vec<u8> {
        let mut types = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>"#,
        );
        let mut sld_ids = String::new();
        let mut relationships = String::new();
        for (index, (number, _)) in self.slides.iter().enumerate() {
            let rid = index + 1;
            types.push_str(&format!(
                r#"<Override PartName="/ppt/slides/slide{number}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
            ));
            sld_ids.push_str(&format!(
                r#"<p:sldId id="{}" r:id="rId{rid}"/>"#,
                256 + index
            ));
            relationships.push_str(&format!(
                r#"<Relationship Id="rId{rid}" Type="{REL_SLIDE}" Target="slides/slide{number}.xml"/>"#
            ));
        }
        for (number, _) in &self.rels {
            types.push_str(&format!(
                r#"<Override PartName="/ppt/slides/_rels/slide{number}.xml.rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#
            ));
        }
        types.push_str("</Types>");
        // 非幻灯片关系刻意占一个较大的 rId：新增关系不得与它撞号。
        let used = self.slides.len() as u32 + 3;
        relationships.push_str(&format!(
            r#"<Relationship Id="rId{used}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/presProps" Target="presProps.xml"/>"#
        ));

        let presentation = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:a="{A_NS}" xmlns:r="{R_NS}" xmlns:p="{P_NS}"><p:sldIdLst>{sld_ids}</p:sldIdLst><p:sldSz cx="14630400" cy="8229600"/></p:presentation>"#
        );
        let presentation_rels = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{relationships}</Relationships>"#
        );

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        add_entry(
            &mut writer,
            "[Content_Types].xml",
            &types,
        );
        add_entry(
            &mut writer,
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#,
        );
        add_entry(&mut writer, "ppt/presentation.xml", &presentation);
        add_entry(
            &mut writer,
            "ppt/_rels/presentation.xml.rels",
            &presentation_rels,
        );
        for (number, xml) in &self.slides {
            add_entry(&mut writer, &format!("ppt/slides/slide{number}.xml"), xml);
        }
        for (number, xml) in &self.rels {
            add_entry(
                &mut writer,
                &format!("ppt/slides/_rels/slide{number}.xml.rels"),
                xml,
            );
        }
        writer.finish().unwrap().into_inner()
    }
}

fn add_entry(writer: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, data: &str) {
    writer.start_file(name, SimpleFileOptions::default()).unwrap();
    writer.write_all(data.as_bytes()).unwrap();
}

// ── 读回工具（只用于断言）───────────────────────────────────────────────

fn part_text(bytes: &[u8], name: &str) -> Option<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut file = archive.by_name(name).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    Some(text)
}

fn local(name: &[u8]) -> &[u8] {
    match name.iter().position(|&byte| byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    }
}

/// 按元素本地名收集属性表（顺带证明 XML 可解析）。
fn attributes_of(xml: &str, element: &str) -> Vec<Vec<(String, String)>> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    let mut buf = Vec::new();
    let mut found = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) | Ok(Event::Empty(start)) => {
                if local(start.name().as_ref()) == element.as_bytes() {
                    let mut attributes = Vec::new();
                    for attribute in start.attributes() {
                        let attribute = attribute.unwrap();
                        attributes.push((
                            String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
                            String::from_utf8_lossy(&attribute.value).into_owned(),
                        ));
                    }
                    found.push(attributes);
                }
            }
            Ok(_) => {}
            Err(error) => panic!("XML 解析失败: {error}"),
        }
    }
    found
}

fn attribute<'a>(attributes: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// 一页 XML 里所有 `<a:t>` 文本的拼接（独立于实现的读法）。
fn text_of(xml: &str) -> String {
    let mut reader = Reader::from_reader(xml.as_bytes());
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut collecting = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) if local(start.name().as_ref()) == b"t" => collecting = true,
            Ok(Event::End(end)) if local(end.name().as_ref()) == b"t" => collecting = false,
            Ok(Event::Text(chunk)) if collecting => {
                text.push_str(&chunk.unescape().unwrap());
            }
            Ok(_) => {}
            Err(error) => panic!("XML 解析失败: {error}"),
        }
    }
    text
}

fn part_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".part-"))
        .collect()
}

// ── 打开 ─────────────────────────────────────────────────────────────────

#[test]
fn open_reports_not_zip_for_garbage_bytes() {
    let (diag, sink) = diagnostics();
    let error = Package::open_bytes("这不是一个 zip 文件".as_bytes(), &diag).unwrap_err();
    assert!(matches!(error, PptxError::NotZip(_)), "得到 {error:?}");
    assert!(
        sink.events()
            .iter()
            .any(|event| event.level == Level::Error && event.target == "pptx"),
        "打开失败必须留 diag error 事件"
    );
}

#[test]
fn open_reports_missing_required_parts() {
    let (diag, _) = diagnostics();

    // 只有演示文稿主体：缺 [Content_Types].xml
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    add_entry(&mut writer, "ppt/presentation.xml", "<p:presentation/>");
    let bytes = writer.finish().unwrap().into_inner();
    match Package::open_bytes(&bytes, &diag).unwrap_err() {
        PptxError::MissingStructure(part) => assert_eq!(part, "[Content_Types].xml"),
        other => panic!("期望 MissingStructure，得到 {other:?}"),
    }

    // 有 [Content_Types].xml、缺 presentation.xml
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    add_entry(&mut writer, "[Content_Types].xml", "<Types/>");
    add_entry(&mut writer, "ppt/slides/slide1.xml", "<p:sld/>");
    let bytes = writer.finish().unwrap().into_inner();
    match Package::open_bytes(&bytes, &diag).unwrap_err() {
        PptxError::MissingStructure(part) => assert_eq!(part, "ppt/presentation.xml"),
        other => panic!("期望 MissingStructure，得到 {other:?}"),
    }
}

#[test]
fn open_reports_deck_without_slides() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new().build();
    let error = Package::open_bytes(&fixture, &diag).unwrap_err();
    assert!(matches!(error, PptxError::NoSlides), "得到 {error:?}");
}

#[test]
fn open_reports_unparsable_slide_xml() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, "<p:sld><a:p><a:t>缺结束标签</p:sld>")
        .build();
    let error = Package::open_bytes(&fixture, &diag).unwrap_err();
    assert!(matches!(error, PptxError::Xml(_)), "得到 {error:?}");
}

#[test]
fn slides_are_enumerated_in_ascending_number_order() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(10, &slide_xml(&["{{单词}}"]))
        .slide(2, &slide_xml(&["{{单词}}"]))
        .build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();
    assert_eq!(package.slide_count(), 2);

    let mut observer = Recording::passive();
    let entries = [entry("apple"), entry("banana")];
    assert_eq!(
        package
            .fill(&entries, TokenScheme::Placeholders, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "deck.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    let bytes = fs::read(&artifact.path).unwrap();
    assert!(
        text_of(&part_text(&bytes, "ppt/slides/slide2.xml").unwrap()).contains("apple"),
        "第 1 个词条应落在 slide2.xml（序号最小的页）"
    );
    assert!(
        text_of(&part_text(&bytes, "ppt/slides/slide10.xml").unwrap()).contains("banana"),
        "第 2 个词条应落在 slide10.xml"
    );
}

// ── 扫描 ─────────────────────────────────────────────────────────────────

#[test]
fn scan_tokens_dedupes_and_returns_canonical_order() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, &slide_xml(&["__DEF__", "__WORD__"]))
        .slide(2, &slide_xml(&["__PHONETIC__", "__WORD__"]))
        .build();
    let package = Package::open_bytes(&fixture, &diag).unwrap();
    assert_eq!(package.slide_count(), 2);
    assert_eq!(
        package.scan_tokens(TokenScheme::Markers),
        vec![Field::Word, Field::Phonetic, Field::Definition]
    );
    assert!(package.scan_tokens(TokenScheme::Placeholders).is_empty());
}

#[test]
fn scan_tokens_finds_placeholders_split_across_runs() {
    let (diag, _) = diagnostics();
    let split_word = r#"<a:p><a:r><a:t>{{</a:t></a:r><a:r><a:t>单词</a:t></a:r><a:r><a:t>}}</a:t></a:r></a:p>"#;
    let split_example_def = r#"<a:p><a:r><a:t>例句释义：{{例句</a:t></a:r><a:r><a:t>释义}}</a:t></a:r></a:p>"#;
    let fixture = Fixture::new()
        .slide(1, &slide_xml_raw(&format!("{split_word}{split_example_def}")))
        .build();
    let package = Package::open_bytes(&fixture, &diag).unwrap();
    assert_eq!(
        package.scan_tokens(TokenScheme::Placeholders),
        vec![Field::Word, Field::ExampleDef]
    );
}

// ── 填充 ─────────────────────────────────────────────────────────────────

#[test]
fn fill_replaces_tokens_and_escapes_values() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, &slide_xml(&["{{单词}}", "{{例句}}"]))
        .build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();

    let mut special = entry("Tom & Jerry <3>");
    special.example = r#"引号 " 与 ' 都进去"#.to_string();
    let mut observer = Recording::passive();
    assert_eq!(
        package
            .fill(&[special.clone()], TokenScheme::Placeholders, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "escaped.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    let bytes = fs::read(&artifact.path).unwrap();
    let xml = part_text(&bytes, "ppt/slides/slide1.xml").unwrap();

    assert!(!xml.contains("{{"), "占位符应当全部替换: {xml}");
    assert!(xml.contains("Tom &amp; Jerry &lt;3&gt;"));
    assert!(xml.contains("&quot;") && xml.contains("&apos;"));
    // 转义后的 XML 反解回来必须与词条原值逐字相同
    let text = text_of(&xml);
    assert!(text.contains(&special.word), "得到 {text}");
    assert!(text.contains(&special.example), "得到 {text}");
}

#[test]
fn fill_merges_marker_split_across_runs_and_keeps_run_style() {
    let (diag, _) = diagnostics();
    let body = r#"<a:p><a:r><a:rPr sz="7200"/><a:t>__WO</a:t></a:r><a:r><a:rPr sz="7200"/><a:t>RD__</a:t></a:r></a:p>"#;
    let fixture = Fixture::new().slide(1, &slide_xml_raw(body)).build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();

    let mut observer = Recording::passive();
    assert_eq!(
        package
            .fill(&[entry("apple")], TokenScheme::Markers, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "split.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    let bytes = fs::read(&artifact.path).unwrap();
    let xml = part_text(&bytes, "ppt/slides/slide1.xml").unwrap();

    assert_eq!(xml.matches(r#"<a:rPr sz="7200"/>"#).count(), 2, "run 样式必须保留");
    assert_eq!(xml.matches("<a:t>apple</a:t>").count(), 1, "合并后的文本落在首个 run");
    assert_eq!(xml.matches("<a:t></a:t>").count(), 1, "其余 run 清空");
    assert!(!xml.contains("__WO") && !xml.contains("RD__"));
    assert_eq!(text_of(&xml), "apple");
}

#[test]
fn fill_leaves_automatic_field_text_alone() {
    let (diag, _) = diagnostics();
    let body = r#"<a:p><a:fld id="{11111111-1111-1111-1111-111111111111}" type="slidenum"><a:t>7</a:t></a:fld><a:r><a:t>{{单词}}</a:t></a:r></a:p>"#;
    let fixture = Fixture::new().slide(1, &slide_xml_raw(body)).build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();

    let mut observer = Recording::passive();
    assert_eq!(
        package
            .fill(&[entry("apple")], TokenScheme::Placeholders, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "field.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    let bytes = fs::read(&artifact.path).unwrap();
    let xml = part_text(&bytes, "ppt/slides/slide1.xml").unwrap();

    assert!(xml.contains("<a:t>7</a:t>"), "自动字段文本不得被合并/清空: {xml}");
    assert_eq!(xml.matches("<a:t>apple</a:t>").count(), 1);
}

#[test]
fn fill_keeps_surplus_template_pages_untouched() {
    // 对等旧版：词条数 < 版式页数时，多余的版式页原样留下（不填充、不删除）。
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, &slide_xml(&["{{单词}}"]))
        .slide(2, &slide_xml(&["{{单词}}"]))
        .slide(3, &slide_xml(&["{{单词}}"]))
        .build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();

    let mut observer = Recording::passive();
    assert_eq!(
        package
            .fill(&[entry("apple")], TokenScheme::Placeholders, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );
    assert_eq!(package.slide_count(), 3);

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "fewer.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    assert_eq!(artifact.slides, 3);
    let bytes = fs::read(&artifact.path).unwrap();
    assert!(text_of(&part_text(&bytes, "ppt/slides/slide1.xml").unwrap()).contains("apple"));
    assert!(part_text(&bytes, "ppt/slides/slide2.xml")
        .unwrap()
        .contains("{{单词}}"));
    assert!(part_text(&bytes, "ppt/slides/slide3.xml")
        .unwrap()
        .contains("{{单词}}"));
}

#[test]
fn fill_returns_break_and_writes_nothing() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, &slide_xml(&["{{单词}}"]))
        .build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "cancelled.pptx", OverwritePolicy::Fail);

    let mut observer = Recording::breaking_after(1);
    let entries = [entry("apple"), entry("banana"), entry("cherry")];
    let outcome = package
        .fill(&entries, TokenScheme::Placeholders, &mut observer, &diag)
        .unwrap();

    // 取消是合法终态：调用方拿到 Break 后不落盘（对等 ops::generate_deck 的路径）。
    assert_eq!(outcome, ControlFlow::Break(()));
    assert!(!plan.dir.join(&plan.name).exists(), "取消不得留下产物");
    assert_eq!(observer.progress.len(), 1);
    assert_eq!(
        observer.progress[0],
        Progress::Slides { done: 1, total: 3 }
    );
    assert!(part_files(dir.path()).is_empty());
}

// ── 复制末页与结构自检 ───────────────────────────────────────────────────

#[test]
fn fill_copies_last_slide_for_extra_entries() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, &slide_xml(&["{{单词}}", "{{音标}}"]))
        .slide_rels(1, SLIDE_RELS)
        .build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();

    let mut observer = Recording::passive();
    let entries = [entry("apple"), entry("banana"), entry("cherry")];
    assert_eq!(
        package
            .fill(&entries, TokenScheme::Placeholders, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );
    assert_eq!(package.slide_count(), 3);

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "three.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    assert_eq!(artifact.slides, 3);
    assert!(artifact.path.exists());
    assert!(part_files(dir.path()).is_empty(), "不留 .part 临时文件");

    let bytes = fs::read(&artifact.path).unwrap();
    // 重开自检：页数不变
    let reopened = Package::open(&artifact.path, &diag).unwrap();
    assert_eq!(reopened.slide_count(), 3);
    assert_eq!(
        text_of(&part_text(&bytes, "ppt/slides/slide1.xml").unwrap()),
        "apple/apple/"
    );
    assert!(text_of(&part_text(&bytes, "ppt/slides/slide2.xml").unwrap()).starts_with("banana"));
    assert!(text_of(&part_text(&bytes, "ppt/slides/slide3.xml").unwrap()).starts_with("cherry"));

    // 复制页连 rels 一起拷贝
    assert_eq!(
        part_text(&bytes, "ppt/slides/_rels/slide2.xml.rels").unwrap(),
        SLIDE_RELS
    );
    assert_eq!(
        part_text(&bytes, "ppt/slides/_rels/slide3.xml.rels").unwrap(),
        SLIDE_RELS
    );

    // [Content_Types].xml：三页 + 两个新 rels 的 Override 齐备
    let types = part_text(&bytes, "[Content_Types].xml").unwrap();
    let overrides = attributes_of(&types, "Override");
    for number in 1..=3 {
        let expected = format!("/ppt/slides/slide{number}.xml");
        assert!(
            overrides
                .iter()
                .any(|attributes| attribute(attributes, "PartName") == Some(expected.as_str())),
            "缺 {expected} 的 Override: {overrides:?}"
        );
    }
    for number in 2..=3 {
        let expected = format!("/ppt/slides/_rels/slide{number}.xml.rels");
        assert!(
            overrides
                .iter()
                .any(|attributes| attribute(attributes, "PartName") == Some(expected.as_str())),
            "缺 {expected} 的 Override: {overrides:?}"
        );
    }

    // presentation.xml：sldId 三处一致、无重复
    let presentation = part_text(&bytes, "ppt/presentation.xml").unwrap();
    let sld_ids = attributes_of(&presentation, "sldId");
    assert_eq!(sld_ids.len(), 3);
    let mut ids: Vec<String> = sld_ids
        .iter()
        .map(|attributes| attribute(attributes, "id").unwrap().to_string())
        .collect();
    ids.sort();
    let mut unique = ids.clone();
    unique.dedup();
    assert_eq!(unique.len(), 3, "sldId id 有重复: {ids:?}");
    assert_eq!(unique, vec!["256", "257", "258"]);

    // rels：新增关系的 Id 与 sldId 的 r:id 一一对应，且避开已有的非幻灯片 rId
    let rels = part_text(&bytes, "ppt/_rels/presentation.xml.rels").unwrap();
    let relationships = attributes_of(&rels, "Relationship");
    let mut rel_ids: Vec<String> = relationships
        .iter()
        .map(|attributes| attribute(attributes, "Id").unwrap().to_string())
        .collect();
    rel_ids.sort();
    let mut unique_rels = rel_ids.clone();
    unique_rels.dedup();
    assert_eq!(unique_rels.len(), 4, "关系 Id 有重复: {rel_ids:?}");
    assert!(
        unique_rels.contains(&"rId5".to_string()),
        "新增关系应排在已有最大 rId（rId4，presProps）之后: {rel_ids:?}"
    );
    for number in 1..=3 {
        let expected = format!("slides/slide{number}.xml");
        assert!(
            relationships.iter().any(|attributes| {
                attribute(attributes, "Type") == Some(REL_SLIDE)
                    && attribute(attributes, "Target") == Some(expected.as_str())
            }),
            "缺 {expected} 的幻灯片关系: {relationships:?}"
        );
    }
    let mut referenced: Vec<String> = sld_ids
        .iter()
        .map(|attributes| attribute(attributes, "r:id").unwrap().to_string())
        .collect();
    referenced.sort();
    let mut unique_referenced = referenced.clone();
    unique_referenced.dedup();
    assert_eq!(unique_referenced.len(), 3, "sldId 的 r:id 有重复: {referenced:?}");
    let slide_rel_ids: Vec<String> = {
        let mut ids: Vec<String> = relationships
            .iter()
            .filter(|attributes| attribute(attributes, "Type") == Some(REL_SLIDE))
            .map(|attributes| attribute(attributes, "Id").unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        unique_referenced, slide_rel_ids,
        "sldIdLst 的 r:id 必须与关系表的幻灯片关系完全一致"
    );
}

#[test]
fn save_atomic_self_checks_and_opens_again() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, &slide_xml(&["{{单词}}"]))
        .build();
    let mut package = Package::open_bytes(&fixture, &diag).unwrap();
    let mut observer = Recording::passive();
    assert_eq!(
        package
            .fill(&[entry("apple")], TokenScheme::Placeholders, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "deck.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    assert_eq!(artifact.path, dir.path().join("deck.pptx"));
    assert_eq!(artifact.slides, 1);
    assert!(part_files(dir.path()).is_empty());
    let reopened = Package::open(&artifact.path, &diag).unwrap();
    assert_eq!(reopened.slide_count(), 1);
}

#[test]
fn save_atomic_propagates_exists_under_fail_policy() {
    let (diag, _) = diagnostics();
    let fixture = Fixture::new()
        .slide(1, &slide_xml(&["{{单词}}"]))
        .build();
    let package = Package::open_bytes(&fixture, &diag).unwrap();

    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("taken.pptx"), "原有内容".as_bytes()).unwrap();
    let plan = OutputPlan::new(dir.path(), "taken.pptx", OverwritePolicy::Fail);
    let error = package.save_atomic(&plan, &diag).unwrap_err();
    match error {
        PptxError::Output(OutputError::Exists(path)) => {
            assert_eq!(path, dir.path().join("taken.pptx"));
        }
        other => panic!("期望 PptxError::Output(Exists)，得到 {other:?}"),
    }
    // 撞名不得动到已有文件，也不得留下临时文件
    assert_eq!(
        fs::read(dir.path().join("taken.pptx")).unwrap(),
        "原有内容".as_bytes()
    );
    assert!(part_files(dir.path()).is_empty());
}

// ── 内嵌资产 ─────────────────────────────────────────────────────────────

#[test]
fn embedded_template_opens_scans_and_fills_markers() {
    let (diag, _) = diagnostics();
    let mut package = Package::open_bytes(EMBEDDED_TEMPLATE, &diag).unwrap();
    assert!(package.slide_count() >= 1);
    let fields = package.scan_tokens(TokenScheme::Markers);
    assert!(fields.contains(&Field::Word), "内嵌版式缺少 __WORD__: {fields:?}");
    assert!(fields.contains(&Field::Phonetic), "内嵌版式缺少 __PHONETIC__: {fields:?}");

    let mut observer = Recording::passive();
    assert_eq!(
        package
            .fill(&[entry("apple")], TokenScheme::Markers, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "embedded.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    let bytes = fs::read(&artifact.path).unwrap();
    let xml = part_text(&bytes, "ppt/slides/slide1.xml").unwrap();
    assert!(text_of(&xml).contains("apple"), "首屏应填入词条");
    assert!(!xml.contains("__WORD__"), "标记应被替换: {xml}");
}

/// 内嵌版式（真实包结构）走复制末页分支：三处结构必须自洽。
#[test]
fn embedded_template_copy_path_patches_structure() {
    let (diag, _) = diagnostics();
    let mut package = Package::open_bytes(EMBEDDED_TEMPLATE, &diag).unwrap();
    let template_pages = package.slide_count();
    let wanted = template_pages + 2;
    let entries: Vec<WordEntry> = (0..wanted).map(|i| entry(&format!("word{i}"))).collect();

    let mut observer = Recording::passive();
    assert_eq!(
        package
            .fill(&entries, TokenScheme::Markers, &mut observer, &diag)
            .unwrap(),
        ControlFlow::Continue(())
    );
    assert_eq!(package.slide_count(), wanted);

    let dir = tempfile::tempdir().unwrap();
    let plan = OutputPlan::new(dir.path(), "embedded-copy.pptx", OverwritePolicy::Fail);
    let artifact = package.save_atomic(&plan, &diag).unwrap();
    assert_eq!(artifact.slides, wanted);
    assert!(part_files(dir.path()).is_empty());
    let bytes = fs::read(&artifact.path).unwrap();
    assert_eq!(Package::open(&artifact.path, &diag).unwrap().slide_count(), wanted);

    // presentation.xml：sldId 数量与页数一致，id 与 r:id 都不重复
    let presentation = part_text(&bytes, "ppt/presentation.xml").unwrap();
    let sld_ids = attributes_of(&presentation, "sldId");
    assert_eq!(sld_ids.len(), wanted);
    let mut ids: Vec<String> = sld_ids
        .iter()
        .map(|attributes| attribute(attributes, "id").unwrap().to_string())
        .collect();
    ids.sort();
    let mut unique_ids = ids.clone();
    unique_ids.dedup();
    assert_eq!(unique_ids.len(), wanted, "sldId id 有重复: {ids:?}");
    let mut referenced: Vec<String> = sld_ids
        .iter()
        .map(|attributes| attribute(attributes, "r:id").unwrap().to_string())
        .collect();
    referenced.sort();
    let mut unique_referenced = referenced.clone();
    unique_referenced.dedup();
    assert_eq!(unique_referenced.len(), wanted, "r:id 有重复: {referenced:?}");

    // rels：幻灯片关系与 sldIdLst 一一对应，全部指向真实存在的页
    let rels = part_text(&bytes, "ppt/_rels/presentation.xml.rels").unwrap();
    let relationships = attributes_of(&rels, "Relationship");
    let mut slide_targets: Vec<String> = relationships
        .iter()
        .filter(|attributes| attribute(attributes, "Type") == Some(REL_SLIDE))
        .map(|attributes| attribute(attributes, "Target").unwrap().to_string())
        .collect();
    let mut slide_rel_ids: Vec<String> = relationships
        .iter()
        .filter(|attributes| attribute(attributes, "Type") == Some(REL_SLIDE))
        .map(|attributes| attribute(attributes, "Id").unwrap().to_string())
        .collect();
    slide_targets.sort();
    slide_rel_ids.sort();
    let expected_targets: Vec<String> = (1..=wanted)
        .map(|number| format!("slides/slide{number}.xml"))
        .collect();
    assert_eq!(slide_targets, expected_targets);
    assert_eq!(unique_referenced, slide_rel_ids);

    // [Content_Types].xml：每一页与新 rels 都有 Override；新页的 rels 从末页拷贝
    let types = part_text(&bytes, "[Content_Types].xml").unwrap();
    let overrides = attributes_of(&types, "Override");
    for number in 1..=wanted {
        let expected = format!("/ppt/slides/slide{number}.xml");
        assert!(
            overrides
                .iter()
                .any(|attributes| attribute(attributes, "PartName") == Some(expected.as_str())),
            "缺 {expected} 的 Override"
        );
    }
    let last_rels = part_text(
        &bytes,
        &format!("ppt/slides/_rels/slide{template_pages}.xml.rels"),
    )
    .unwrap();
    for number in template_pages + 1..=wanted {
        assert_eq!(
            part_text(&bytes, &format!("ppt/slides/_rels/slide{number}.xml.rels")).unwrap(),
            last_rels,
            "复制页应带末页的 rels"
        );
    }

    // 每页都填到了自己的词条
    for number in 1..=wanted {
        let text = text_of(
            &part_text(&bytes, &format!("ppt/slides/slide{number}.xml")).unwrap(),
        );
        assert!(
            text.contains(&format!("word{}", number - 1)),
            "第 {number} 页未填入对应词条: {text}"
        );
    }
}
