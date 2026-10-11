//! `platform::sheets` 契约测试：两个格式共用流水线（表头 / 映射 / 跳过 / 空表），
//! 加上各自差异（xlsx 选表与工作簿顺序、csv 编码）。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use x2p_core::diag::{field, Diag, DiagEvent, Level, MemorySink};
use x2p_core::domain::schema::{Field, SheetInfo, Source, SourceKind, FIELDS};
use x2p_core::platform::sheets::{self, LoadError};

// ------------------------------------------------------------------ 工具

fn memory() -> (Diag, MemorySink) {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
    (diag, sink)
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("建临时目录")
}

/// 六列表头行（表头来自字段表，不写字面量）。
fn header_line() -> String {
    FIELDS
        .iter()
        .map(|meta| meta.header)
        .collect::<Vec<_>>()
        .join(",")
}

fn csv_source(path: &Path, encoding: &str) -> Source {
    Source {
        path: path.to_path_buf(),
        kind: SourceKind::Csv {
            encoding: encoding.to_string(),
        },
    }
}

fn excel_source(path: &Path, sheet: Option<&str>) -> Source {
    Source {
        path: path.to_path_buf(),
        kind: SourceKind::Excel {
            sheet: sheet.map(str::to_string),
        },
    }
}

fn write_bytes(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).expect("写 fixture");
    path
}

fn write_csv(dir: &Path, name: &str, text: &str) -> PathBuf {
    write_bytes(dir, name, text.as_bytes())
}

fn events_on(sink: &MemorySink, level: Level) -> Vec<DiagEvent> {
    sink.events()
        .into_iter()
        .filter(|event| event.level == level)
        .collect()
}

/// 事件 `fields` 里的某个键（无 `fields` 或键缺失 → `None`）。
fn field_of(event: &DiagEvent, key: &str) -> Option<serde_json::Value> {
    event.fields.as_ref()?.get(key).cloned()
}

// ------------------------------------------------------------ csv：成功路径

#[test]
fn csv_reads_six_columns_in_field_order() {
    let dir = tempdir();
    let path = write_csv(
        dir.path(),
        "ok.csv",
        &format!(
            "{}\n\
             apple,/ˈæpl/,a-pple,I eat an apple every day.,我每天吃一个苹果。,苹果\n\
             \x20\x20banana\x20\x20,\x20/bəˈnɑːnə/\x20,\x20ban-ana\x20,Bananas are yellow.,\x20香蕉是黄色的。\x20,\x20香蕉\x20\n",
            header_line()
        ),
    );
    let (diag, sink) = memory();
    let read = sheets::read(&csv_source(&path, "UTF-8"), &diag).expect("六列表头应读取成功");

    assert_eq!(read.columns, Field::ALL.to_vec());
    assert_eq!(read.skipped, 0);
    assert_eq!(read.entries.len(), 2);

    let first = &read.entries[0];
    assert_eq!(first.word, "apple");
    assert_eq!(first.phonetic, "/ˈæpl/");
    assert_eq!(first.morphology, "a-pple");
    assert_eq!(first.example, "I eat an apple every day.");
    assert_eq!(first.example_definition, "我每天吃一个苹果。");
    assert_eq!(first.definition, "苹果");

    // 单元格 trim（前后空白不进词条）
    assert_eq!(read.entries[1].word, "banana");
    assert_eq!(read.entries[1].phonetic, "/bəˈnɑːnə/");
    assert_eq!(read.entries[1].definition, "香蕉");

    // 完成事件带上契约字段
    let done = events_on(&sink, Level::Info)
        .into_iter()
        .find(|event| field_of(event, field::ENTRIES).is_some())
        .expect("应有读取完成事件");
    assert_eq!(field_of(&done, field::ENTRIES), Some(serde_json::json!(2)));
    assert_eq!(field_of(&done, field::SKIPPED), Some(serde_json::json!(0)));
    assert_eq!(
        field_of(&done, field::COLUMNS),
        Some(serde_json::json!([
            "英文单词", "英文音标", "词根词缀", "例句", "例句释义", "单词释义"
        ]))
    );
    assert_eq!(
        field_of(&done, field::ENCODING),
        Some(serde_json::json!("UTF-8"))
    );
}

#[test]
fn csv_ignores_unknown_columns() {
    let dir = tempdir();
    let path = write_csv(
        dir.path(),
        "extra.csv",
        &format!(
            "备注,{},来源\n注,apple,/ˈæpl/,a-pple,I eat an apple.,我吃苹果。,苹果,自编\n",
            header_line()
        ),
    );
    let (diag, _sink) = memory();
    let read = sheets::read(&csv_source(&path, "UTF-8"), &diag).expect("未知列应被忽略");

    assert_eq!(read.columns, Field::ALL.to_vec());
    assert_eq!(read.entries.len(), 1);
    assert_eq!(read.entries[0].word, "apple");
    assert_eq!(read.entries[0].definition, "苹果");
}

// ------------------------------------------------------------ csv：缺列聚合

#[test]
fn missing_columns_are_aggregated_in_field_order() {
    let dir = tempdir();
    let path = write_csv(
        dir.path(),
        "missing.csv",
        "英文单词,例句\napple,I eat an apple.\n",
    );
    let (diag, sink) = memory();
    let error = sheets::read(&csv_source(&path, "UTF-8"), &diag).expect_err("缺列应报错");

    match &error {
        LoadError::MissingColumns(missing) => assert_eq!(
            missing,
            &vec!["英文音标", "词根词缀", "例句释义", "单词释义"],
            "缺失列按 FIELDS 顺序一次性报出"
        ),
        other => panic!("应为 MissingColumns，实际: {other:?}"),
    }
    // 文案含缺失列名（CLI / GUI 直接展示）
    let text = error.to_string();
    for header in ["英文音标", "词根词缀", "例句释义", "单词释义"] {
        assert!(text.contains(header), "{text} 应含 {header}");
    }
    // 失败留痕（I2），缺失列另可查询
    let failure = events_on(&sink, Level::Error)
        .into_iter()
        .next()
        .expect("失败应记 error 事件");
    assert_eq!(
        field_of(&failure, field::MISSING),
        Some(serde_json::json!(["英文音标", "词根词缀", "例句释义", "单词释义"]))
    );
}

// -------------------------------------------------------------- csv：编码

#[test]
fn gbk_gb2312_gb18030_csv_decode_chinese() {
    let dir = tempdir();
    let text = format!(
        "{}\n苹果,/pingguo/,苹-果,我每天吃一个苹果。,我每天吃一个苹果的意思。,苹果囧\n",
        header_line()
    );
    for label in ["GBK", "GB2312", "GB18030"] {
        let encoding =
            encoding_rs::Encoding::for_label(label.as_bytes()).expect("候选编码应被 encoding_rs 认识");
        let (bytes, _, had_errors) = encoding.encode(&text);
        assert!(!had_errors, "{label} 应能表示全部字符");
        let path = write_bytes(dir.path(), &format!("{label}.csv"), &bytes);

        let (diag, _sink) = memory();
        let read = sheets::read(&csv_source(&path, label), &diag)
            .unwrap_or_else(|error| panic!("{label} 应正确解码: {error}"));
        assert_eq!(read.entries.len(), 1, "{label}");
        assert_eq!(read.entries[0].word, "苹果", "{label}");
        assert_eq!(read.entries[0].phonetic, "/pingguo/", "{label}");
        assert_eq!(
            read.entries[0].example_definition,
            "我每天吃一个苹果的意思。",
            "{label}"
        );
        assert_eq!(read.entries[0].definition, "苹果囧", "{label}");
    }
}

#[test]
fn invalid_encoding_label_is_reported() {
    let dir = tempdir();
    let path = write_csv(dir.path(), "label.csv", &format!("{}\n", header_line()));
    let (diag, _sink) = memory();
    let error = sheets::read(&csv_source(&path, "utf-99"), &diag).expect_err("非法标签应报错");

    match &error {
        LoadError::Encoding { label } => assert_eq!(label, "utf-99"),
        other => panic!("应为 Encoding，实际: {other:?}"),
    }
}

#[test]
fn invalid_utf8_bytes_are_reported() {
    let dir = tempdir();
    let mut bytes = format!("{}\n", header_line()).into_bytes();
    bytes.extend_from_slice(b"apple,/x/,\xff\xfe,ex,exdef,def\n");
    let path = write_bytes(dir.path(), "bad-utf8.csv", &bytes);
    let (diag, _sink) = memory();
    let error = sheets::read(&csv_source(&path, "UTF-8"), &diag).expect_err("无效字节应报错");

    match &error {
        LoadError::Encoding { label } => assert_eq!(label, "UTF-8"),
        other => panic!("应为 Encoding，实际: {other:?}"),
    }
}

// ------------------------------------------------------- csv：跳过与空表

#[test]
fn blank_word_rows_are_skipped_counted_and_logged() {
    let dir = tempdir();
    let path = write_csv(
        dir.path(),
        "skip.csv",
        &format!(
            "{}\n\
             ,/biː/,be,To be or not to be.,生存还是毁灭。,是\n\
             apple,/ˈæpl/,a-pple,I eat an apple.,我吃苹果。,苹果\n\
             \x20\x20\x20,/x/,y,中文全空的行,\x20,\n",
            header_line()
        ),
    );
    let (diag, sink) = memory();
    let read = sheets::read(&csv_source(&path, "UTF-8"), &diag).expect("空 word 行跳过而非报错");

    assert_eq!(read.entries.len(), 1);
    assert_eq!(read.entries[0].word, "apple");
    assert_eq!(read.skipped, 2, "两行「英文单词」为空");

    let warns = events_on(&sink, Level::Warn);
    assert_eq!(warns.len(), 2);
    let rows: Vec<u64> = warns
        .iter()
        .filter_map(|event| field_of(event, field::ROW)?.as_u64())
        .collect();
    assert_eq!(rows, vec![2, 4], "行号含表头行，从 2 起算（对等旧版）");
}

#[test]
fn empty_csv_is_empty_sheet() {
    let dir = tempdir();
    let path = write_csv(dir.path(), "empty.csv", "");
    let (diag, sink) = memory();
    let error = sheets::read(&csv_source(&path, "UTF-8"), &diag).expect_err("空文件应报 EmptySheet");

    assert!(matches!(error, LoadError::EmptySheet), "实际: {error:?}");
    assert_eq!(events_on(&sink, Level::Error).len(), 1);
}

#[test]
fn header_only_csv_has_no_entries() {
    let dir = tempdir();
    let path = write_csv(dir.path(), "header.csv", &format!("{}\n", header_line()));
    let (diag, _sink) = memory();
    let read = sheets::read(&csv_source(&path, "UTF-8"), &diag).expect("只有表头不是错误");

    assert!(read.entries.is_empty());
    assert_eq!(read.skipped, 0);
    assert_eq!(read.columns, Field::ALL.to_vec());
}

#[test]
fn missing_file_is_open_error() {
    let dir = tempdir();
    let missing = dir.path().join("nope.csv");
    let (diag, _sink) = memory();

    let error = sheets::read(&csv_source(&missing, "UTF-8"), &diag).expect_err("文件不存在");
    assert!(matches!(error, LoadError::Open(_)), "实际: {error:?}");
    let error = sheets::list_sheets(&missing, &diag).expect_err("文件不存在");
    assert!(matches!(error, LoadError::Open(_)), "实际: {error:?}");
}

// ------------------------------------------------------------------ xlsx

/// xlsx fixture 源：优先旧仓库 `assets/template.xlsx`（验收指定），
/// CI 无旧仓库时退回本仓库自带资产（两者字节一致）。
fn fixture_source() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let legacy = manifest.join("../../../xlsx-to-pptx-legacy/assets/template.xlsx");
    if legacy.is_file() {
        return legacy;
    }
    manifest.join("assets").join("词表模板.xlsx")
}

/// 复制一份 fixture 到临时目录（测试不碰仓库资产，也不在仓库里写文件）。
fn copy_fixture(dir: &Path) -> PathBuf {
    let source = fixture_source();
    assert!(source.is_file(), "缺少 xlsx fixture: {}", source.display());
    let target = dir.join("template.xlsx");
    fs::copy(&source, &target).expect("复制 fixture");
    target
}

#[test]
fn xlsx_fixture_reads_five_entries() {
    let dir = tempdir();
    let path = copy_fixture(dir.path());
    let (diag, sink) = memory();
    let read = sheets::read(&excel_source(&path, Some("单词表")), &diag).expect("模板应读取成功");

    assert_eq!(read.columns, Field::ALL.to_vec());
    assert_eq!(read.skipped, 0);
    assert_eq!(read.entries.len(), 5, "模板为 5 条数据");

    let first = &read.entries[0];
    assert_eq!(first.word, "apple");
    assert_eq!(first.phonetic, "/ˈæpl/");
    assert_eq!(first.morphology, "a-pple");
    assert_eq!(first.example, "I eat an apple every day.");
    assert_eq!(first.example_definition, "我每天吃一个苹果。");
    assert_eq!(first.definition, "苹果");
    assert_eq!(read.entries[4].word, "elephant");

    let done = events_on(&sink, Level::Info)
        .into_iter()
        .find(|event| field_of(event, field::ENTRIES).is_some())
        .expect("应有读取完成事件");
    assert_eq!(field_of(&done, field::ENTRIES), Some(serde_json::json!(5)));
    assert_eq!(
        field_of(&done, field::SHEET),
        Some(serde_json::json!("单词表"))
    );
}

#[test]
fn xlsx_no_sheet_takes_first_worksheet() {
    let dir = tempdir();
    let path = copy_fixture(dir.path());
    let (diag, _sink) = memory();
    let read = sheets::read(&excel_source(&path, None), &diag).expect("sheet = None 取第一个工作表");

    assert_eq!(read.entries.len(), 5);
    assert_eq!(read.entries[0].word, "apple");
}

#[test]
fn xlsx_unknown_sheet_is_open_error() {
    let dir = tempdir();
    let path = copy_fixture(dir.path());
    let (diag, _sink) = memory();
    let error =
        sheets::read(&excel_source(&path, Some("不存在")), &diag).expect_err("工作表不存在应报错");

    match error {
        LoadError::Open(message) => {
            assert!(message.contains("工作表不存在"), "{message}");
            assert!(message.contains("单词表"), "应列出可用工作表: {message}");
        }
        other => panic!("应为 Open，实际: {other:?}"),
    }
}

#[test]
fn non_workbook_file_is_open_error() {
    let dir = tempdir();
    let path = write_csv(dir.path(), "fake.xlsx", "这不是工作簿\n");
    let (diag, _sink) = memory();
    let error = sheets::read(&excel_source(&path, None), &diag).expect_err("非工作簿应报错");

    assert!(matches!(error, LoadError::Open(_)), "实际: {error:?}");
}

// -------------------------------------------------- xlsx：工作簿内顺序

/// `<sheets>` 里的新表声明（`rid` 由 [`free_rid`] 分配）。
fn sheet_element(name: &str, rid: &str) -> String {
    format!(r#"<sheet name="{name}" sheetId="2" state="visible" r:id="{rid}" />"#)
}

/// `workbook.xml.rels` 里的新表关系。
fn sheet_relationship(rid: &str) -> String {
    format!(
        concat!(
            r#"<Relationship Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" "#,
            r#"Target="worksheets/sheet2.xml" Id="{}" />"#
        ),
        rid
    )
}

const SECOND_SHEET_TYPE: &str = concat!(
    r#"<Override PartName="/xl/worksheets/sheet2.xml" "#,
    r#"ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml" />"#
);

/// fixture 里未占用的 rId（撞号会让 calamine 把新表指到 styles.xml 之类的部件上）。
fn free_rid(rels: &str) -> String {
    let mut max = 0u32;
    for part in rels.split(r#"Id="rId"#).skip(1) {
        if let Some(digits) = part.split('"').next() {
            if let Ok(number) = digits.parse::<u32>() {
                max = max.max(number);
            }
        }
    }
    format!("rId{}", max + 1)
}

/// 在 `anchor` 之后插入 `addition`（fixture 的锚点缺失即测试失败，不静默失效）。
fn inject_after(text: &str, anchor: &str, addition: &str) -> String {
    let at = text
        .find(anchor)
        .unwrap_or_else(|| panic!("fixture 缺少锚点 {anchor}"))
        + anchor.len();
    format!("{}{}{}", &text[..at], addition, &text[at..])
}

/// 在 `anchor` 之前插入 `addition`（闭合标签锚点用）。
fn inject_before(text: &str, anchor: &str, addition: &str) -> String {
    let at = text
        .find(anchor)
        .unwrap_or_else(|| panic!("fixture 缺少锚点 {anchor}"));
    format!("{}{}{}", &text[..at], addition, &text[at..])
}

/// 一行内联字符串单元格（与 fixture 同风格；值里不含 XML 特殊字符）。
fn row_xml(row: usize, values: &[&str]) -> String {
    let mut xml = format!("<row r=\"{row}\">");
    for (index, value) in values.iter().enumerate() {
        let column = (b'A' + index as u8) as char;
        xml.push_str(&format!(
            "<c r=\"{column}{row}\" t=\"inlineStr\"><is><t>{value}</t></is></c>"
        ));
    }
    xml.push_str("</row>");
    xml
}

/// 「第二表」部件：表头 + 一行 `zebra`（单词列前后带空白，顺带断言 trim）。
fn second_sheet_xml() -> String {
    let headers = FIELDS.map(|meta| meta.header);
    format!(
        concat!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
            r#"<dimension ref="A1:F2" /><sheetData>{}{}</sheetData></worksheet>"#
        ),
        row_xml(1, &headers),
        row_xml(
            2,
            &[
                "  zebra  ",
                "/ˈziːbrə/",
                "ze-bra",
                "Zebras have stripes.",
                "斑马有条纹。",
                "斑马"
            ]
        ),
    )
}

/// 「错误表」部件：表头 + 一行，`英文单词` 是公式错误值（`#REF!`）→ 行级问题。
fn error_sheet_xml() -> String {
    let headers = FIELDS.map(|meta| meta.header);
    format!(
        concat!(
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
            r#"<dimension ref="A1:F2" /><sheetData>{}{}</sheetData></worksheet>"#
        ),
        row_xml(1, &headers),
        r#"<row r="2"><c r="A2" t="e"><v>#REF!</v></c><c r="B2" t="inlineStr"><is><t>/x/</t></is></c></row>"#,
    )
}

/// fixture 副本 + 追加一个工作表部件（`sheet2.xml`）：`first` 决定新表声明插在
/// `<sheets>` 首位还是末尾——首位时它的 `sheetId` 与相邻表相反，于是「顺序」只能来自
/// 工作簿内的声明顺序，而不是 sheetId 或表名排序。
fn fixture_with_sheet(
    dir: &Path,
    file: &str,
    name: &str,
    sheet_xml: &str,
    first: bool,
) -> PathBuf {
    let source = copy_fixture(dir);
    let mut archive =
        zip::ZipArchive::new(fs::File::open(&source).expect("打开 fixture")).expect("读 zip");
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("zip 条目");
        let component = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("读条目");
        parts.push((component, bytes));
    }
    drop(archive);

    let rels = parts
        .iter()
        .find(|(component, _)| component == "xl/_rels/workbook.xml.rels")
        .map(|(_, bytes)| String::from_utf8(bytes.clone()).expect("rels 为 UTF-8"))
        .expect("fixture 应有 workbook.xml.rels");
    let rid = free_rid(&rels);
    let element = sheet_element(name, &rid);

    let target = dir.join(file);
    let mut writer = zip::ZipWriter::new(fs::File::create(&target).expect("建 xlsx"));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (component, bytes) in &parts {
        let text = std::str::from_utf8(bytes).expect("fixture 部件为 UTF-8");
        let patched = match component.as_str() {
            "xl/workbook.xml" if first => inject_after(text, "<sheets>", &element),
            "xl/workbook.xml" => inject_before(text, "</sheets>", &element),
            "xl/_rels/workbook.xml.rels" => {
                inject_after(text, "relationships\">", &sheet_relationship(&rid))
            }
            "[Content_Types].xml" => inject_after(
                text,
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
                SECOND_SHEET_TYPE,
            ),
            _ => text.to_string(),
        };
        writer
            .start_file(component.as_str(), options)
            .expect("写 zip 条目");
        writer.write_all(patched.as_bytes()).expect("写部件");
    }
    writer
        .start_file("xl/worksheets/sheet2.xml", options)
        .expect("写 sheet2");
    writer.write_all(sheet_xml.as_bytes()).expect("写 sheet2 内容");
    writer.finish().expect("收尾");
    target
}

#[test]
fn list_sheets_follows_workbook_order() {
    let dir = tempdir();
    let path = fixture_with_sheet(
        dir.path(),
        "two_sheets.xlsx",
        "第二表",
        &second_sheet_xml(),
        true,
    );
    let (diag, _sink) = memory();
    let sheets = sheets::list_sheets(&path, &diag).expect("列出工作表");

    assert_eq!(
        sheets,
        vec![
            SheetInfo {
                name: "第二表".into()
            },
            SheetInfo {
                name: "单词表".into()
            }
        ],
        "顺序 = 工作簿内声明顺序"
    );

    // sheet = None → 第一个工作表（第二表），而不是表名排序或 sheetId 排序的结果
    let read = sheets::read(&excel_source(&path, None), &diag).expect("读第一个工作表");
    assert_eq!(read.entries.len(), 1);
    assert_eq!(read.entries[0].word, "zebra", "xlsx 单元格同样 trim");

    // 按名字选表仍取到原表
    let read = sheets::read(&excel_source(&path, Some("单词表")), &diag).expect("按名选表");
    assert_eq!(read.entries.len(), 5);
    assert_eq!(read.entries[0].word, "apple");
}

#[test]
fn xlsx_error_cell_is_row_parse_error() {
    let dir = tempdir();
    let path = fixture_with_sheet(
        dir.path(),
        "error_cell.xlsx",
        "错误表",
        &error_sheet_xml(),
        false,
    );
    let (diag, _sink) = memory();
    let error =
        sheets::read(&excel_source(&path, Some("错误表")), &diag).expect_err("错误单元格应报错");

    match error {
        LoadError::RowParse { row, detail } => {
            assert_eq!(row, 2, "行号含表头行");
            assert!(detail.contains("英文单词"), "{detail}");
            assert!(detail.contains("#REF!"), "{detail}");
        }
        other => panic!("应为 RowParse，实际: {other:?}"),
    }
}
