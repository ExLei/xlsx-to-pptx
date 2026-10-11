//! 事件模型、汇适配器、日志命名与滚动的行为测试。

use std::path::Path;

use vocab_core::diag::{self, Diag, DiagLevel, FileSink, MemorySink};

#[test]
fn memory_sink_captures_levels_message_and_fields() {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);

    diag.info("reader", "loaded", Some(serde_json::json!({"rows": 2})));
    diag.warn(
        "verify",
        "slide may be blank",
        Some(serde_json::json!({"slide": 3, "reason": "blank"})),
    );
    diag.error("render", "boom", None);

    assert_eq!(diag.event_count(), 3);
    assert_eq!(diag.warnings(), 1);
    assert_eq!(diag.errors(), 1);

    let events = sink.events();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].level, DiagLevel::Info);
    assert_eq!(events[0].target, "reader");
    assert_eq!(events[1].slide_index(), Some(3));
    assert_eq!(events[1].reason(), Some("blank"));
    assert!(events[2].fields.is_none());
}

#[test]
fn file_sink_writes_one_ndjson_line_per_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.log");
    let sink = FileSink::create(&path).unwrap();
    let diag = Diag::with_sinks(vec![Box::new(sink)]);

    diag.info("export", "started", Some(serde_json::json!({"slides": 2})));
    diag.warn("verify", "low density", None);

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "每事件一行");

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["level"], "INFO");
    assert_eq!(first["target"], "export");
    assert_eq!(first["message"], "started");
    assert_eq!(first["fields"]["slides"], 2);
    let timestamp = first["timestamp"].as_str().unwrap();
    assert!(
        timestamp.contains('-') && timestamp.contains(':') && timestamp.contains('.'),
        "时间戳应为本地日期时间（含毫秒）: {timestamp}"
    );

    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert!(second.get("fields").is_none(), "无字段时不应输出 fields 键");
}

#[test]
fn run_log_sink_prunes_to_newest_logs() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("日志");
    std::fs::create_dir_all(&logs).unwrap();
    // 手工播种 25 个旧日志，名字按时间递增
    for i in 1..=25u32 {
        std::fs::write(logs.join(format!("old_20260101_0000{i:02}.log")), b"").unwrap();
    }

    let sink = diag::run_log_sink(&logs, "new").unwrap();
    let newest = sink.path().to_path_buf();

    let count = std::fs::read_dir(&logs)
        .unwrap()
        .filter(|entry| entry.as_ref().map(|e| e.path().is_file()).unwrap_or(false))
        .count();
    assert_eq!(count, diag::MAX_LOGS, "超量旧日志应被清理");
    assert!(
        !logs.join("old_20260101_000001.log").exists(),
        "最早的手工日志应被清理"
    );
    assert!(
        logs.join("old_20260101_000025.log").exists(),
        "较新的手工日志应保留"
    );
    assert!(newest.exists(), "最新日志必须保留");
    assert_eq!(diag::newest_log(&logs).as_ref(), Some(&newest));

    let name = newest.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("new_") && name.ends_with(".log"), "{name}");
}

#[test]
fn newest_log_of_missing_dir_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(diag::newest_log(&dir.path().join("不存在")).is_none());
}

#[test]
fn session_event_records_command_and_data_dir() {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);

    diag::record_session(&diag, "generate", Path::new("/data/英语助记卡片生成数据"));

    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].target, "session");
    let fields = events[0].fields.as_ref().unwrap();
    assert_eq!(fields["command"], "generate");
    assert_eq!(fields["data_dir"], "/data/英语助记卡片生成数据");
    assert!(fields["version"]
        .as_str()
        .map(|version| !version.is_empty())
        .unwrap_or(false));
}
