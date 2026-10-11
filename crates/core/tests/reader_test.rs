use std::path::{Path, PathBuf};
use vocab_core::diag::Diag;
use vocab_core::reader;
use vocab_core::types::InputSource;

fn template_path() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    Path::new(&manifest)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("assets")
        .join("template.xlsx")
}

#[test]
fn list_sheets_returns_template_sheets() {
    let path = template_path();
    let sheets = reader::list_sheets(&path, &Diag::new()).expect("should list sheets");
    assert!(
        sheets.iter().any(|s| s.contains("单词表")),
        "sheets 应包含 '单词表'，实际: {sheets:?}"
    );
}

#[test]
fn load_template_returns_5_entries() {
    let path = template_path();
    let source = InputSource::Excel {
        path: path.clone(),
        sheet: "单词表".into(),
    };
    let entries = reader::load(&source, &Diag::new()).expect("should load entries");
    assert_eq!(entries.len(), 5, "模板应为 5 条数据，实际: {entries:?}");

    let first = &entries[0];
    assert_eq!(first.word, "apple");
    assert_eq!(first.phonetic, "/ˈæpl/");
    assert_eq!(first.morphology, "a-pple");
    assert_eq!(first.example, "I eat an apple every day.");
    assert_eq!(first.example_definition, "我每天吃一个苹果。");
    assert_eq!(first.definition, "苹果");
}

#[test]
fn load_csv_returns_invalid_format() {
    let path = template_path();
    let source = InputSource::Csv {
        path,
        encoding: "utf-8".into(),
    };
    let result = reader::load(&source, &Diag::new());
    assert!(result.is_err(), "非 CSV 文件应返回错误");
}

#[test]
fn load_csv_utf8_parses_one_row() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let csv_path = dir.path().join("test.csv");
    std::fs::write(
        &csv_path,
        "英文单词,英文音标,词根词缀,例句,例句释义,单词释义\n\
         apple,/ˈæpl/,a-pple,I eat an apple every day.,我每天吃一个苹果。,苹果\n",
    )
    .expect("write test csv");

    let source = InputSource::Csv {
        path: csv_path,
        encoding: "utf-8".into(),
    };
    let entries = reader::load(&source, &Diag::new()).expect("should load CSV");
    assert_eq!(entries.len(), 1, "应为 1 条数据，实际: {entries:?}");
    assert_eq!(entries[0].word, "apple");
    assert_eq!(entries[0].phonetic, "/ˈæpl/");
    assert_eq!(entries[0].morphology, "a-pple");
    assert_eq!(entries[0].example, "I eat an apple every day.");
    assert_eq!(entries[0].example_definition, "我每天吃一个苹果。");
    assert_eq!(entries[0].definition, "苹果");
}

#[test]
fn load_csv_missing_columns_reports_all() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let csv_path = dir.path().join("missing.csv");
    std::fs::write(&csv_path, "英文单词,例句\napple,I eat an apple.\n").expect("write test csv");

    let source = InputSource::Csv {
        path: csv_path,
        encoding: "utf-8".into(),
    };
    let result = reader::load(&source, &Diag::new());
    assert!(result.is_err(), "缺失列应返回错误");
    let err = result.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("缺少列"),
        "错误应为 MissingColumns，实际: {err_msg}"
    );
    // 应报告所有 4 个缺失列
    assert!(err_msg.contains("英文音标"), "应报告缺少 英文音标");
    assert!(err_msg.contains("词根词缀"), "应报告缺少 词根词缀");
    assert!(err_msg.contains("例句释义"), "应报告缺少 例句释义");
    assert!(err_msg.contains("单词释义"), "应报告缺少 单词释义");
}

#[test]
fn load_csv_skips_empty_word_row() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let csv_path = dir.path().join("skip.csv");
    std::fs::write(
        &csv_path,
        "英文单词,英文音标,词根词缀,例句,例句释义,单词释义\n\
         ,/biː/,be,To be or not to be.,生存还是毁灭。,是\n\
         apple,/ˈæpl/,a-pple,I eat an apple.,我吃苹果。,苹果\n",
    )
    .expect("write test csv");

    let source = InputSource::Csv {
        path: csv_path,
        encoding: "utf-8".into(),
    };
    let entries = reader::load(&source, &Diag::new()).expect("should load CSV");
    assert_eq!(entries.len(), 1, "空 word 行应被跳过，实际: {entries:?}");
    assert_eq!(entries[0].word, "apple");
}
