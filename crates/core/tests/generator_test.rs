use std::cell::Cell;
use std::fs;
use std::path::Path;

use tempfile::tempdir;
use vocab_core::diag::Diag;
use vocab_core::generator;
use vocab_core::types::{GenerateError, WordEntry};

fn make_entry(word: &str) -> WordEntry {
    WordEntry {
        word: word.into(),
        phonetic: "/test/".into(),
        morphology: "n.".into(),
        example: "This is a test.".into(),
        example_definition: "这是一个测试。".into(),
        definition: "测试".into(),
    }
}

#[test]
fn generate_empty_entries_errors() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("empty.pptx");
    let call_count = Cell::new(0);
    let result = generator::generate(
        &[],
        &output,
        |_, _| {
            call_count.set(call_count.get() + 1);
            true
        },
        &Diag::new(),
    );
    assert!(matches!(result, Err(GenerateError::NoEntries)));
    assert_eq!(
        call_count.get(),
        0,
        "progress should not be called for empty input"
    );
}

#[test]
fn generate_one_slide_creates_pptx() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("one.pptx");
    let entries = vec![make_entry("apple")];
    let call_count = Cell::new(0);
    let result = generator::generate(
        &entries,
        &output,
        |_, _| {
            call_count.set(call_count.get() + 1);
            true
        },
        &Diag::new(),
    );
    assert!(result.is_ok(), "generate should succeed: {result:?}");
    assert!(output.exists(), "output file should exist");
    assert_eq!(
        call_count.get(),
        1,
        "progress should be called once for 1 entry"
    );

    // Verify it's a valid ZIP (PPTX is a ZIP archive)
    let data = fs::read(&output).unwrap();
    assert!(!data.is_empty(), "output should not be empty");
    // ZIP magic bytes
    assert_eq!(&data[0..2], b"PK", "output should be a valid ZIP archive");
}

#[test]
fn generate_calls_progress_correctly() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("three.pptx");
    let entries = vec![make_entry("one"), make_entry("two"), make_entry("three")];

    let calls: Cell<Vec<(usize, usize)>> = Cell::new(Vec::new());
    let result = generator::generate(
        &entries,
        &output,
        |current, total| {
            let mut v = calls.take();
            v.push((current, total));
            calls.set(v);
            true
        },
        &Diag::new(),
    );
    assert!(result.is_ok(), "generate should succeed: {result:?}");

    let recorded = calls.into_inner();
    assert_eq!(recorded.len(), 3, "progress should be called 3 times");
    for (i, (current, total)) in recorded.iter().enumerate() {
        assert_eq!(*current, i + 1, "current should be 1-based index");
        assert_eq!(*total, 3, "total should always be 3");
    }
}

#[test]
fn generate_cancel_stops_early() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("cancelled.pptx");
    let entries = vec![make_entry("one"), make_entry("two"), make_entry("three")];

    let call_count = Cell::new(0);
    let result = generator::generate(
        &entries,
        &output,
        |_, _| {
            call_count.set(call_count.get() + 1);
            false // cancel immediately
        },
        &Diag::new(),
    );
    assert!(matches!(result, Err(GenerateError::Cancelled)));
    assert_eq!(
        call_count.get(),
        1,
        "progress should be called exactly once before cancel"
    );
}

#[test]
fn generate_file_exists_error() {
    let dir = tempdir().unwrap();
    let output = dir.path().join("exists.pptx");
    // Create the file first
    fs::write(&output, b"existing content").unwrap();
    assert!(output.exists());

    let entries = vec![make_entry("test")];
    let result = generator::generate(&entries, &output, |_, _| true, &Diag::new());
    match result {
        Err(GenerateError::FileExists(path)) => {
            assert!(
                Path::new(&path).ends_with("exists.pptx"),
                "path should be output path"
            );
        }
        other => panic!("expected FileExists, got {other:?}"),
    }
}
