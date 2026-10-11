//! 诊断查询引擎：过滤语义、计数、坏行与两种渲染（fixture 为真实 NDJSON 形态）。

use std::path::PathBuf;

use serde_json::{json, Value};
use x2p_core::diag::query::{self, Query, QueryError};
use x2p_core::diag::{self, field, Target};

/// fixture 里的合法事件数。
const TOTAL_EVENTS: usize = 11;
/// fixture 里的坏行数（另有两行空行，不算坏行）。
const BAD_LINES: usize = 2;

/// 覆盖六类查询所需的全部事实：
/// target 分布（verify 3 / render 2 / sheets 2 / fonts 1 / output 1 / renderer 1 / session 1）、
/// 级别分布（INFO 5 / WARN 5 / ERROR 1）、同一 slide 两种级别、以及两类"看着像但不该命中"的事件
/// （reason=timeout 的 verify；target≠verify 的 reason=blank）。
const FIXTURE_LINES: &[&str] = &[
    r#"{"timestamp":"2026-10-12T10:00:00","level":"INFO","target":"session","message":"session","fields":{"command":"generate","fallback":false},"extra":"keep"}"#,
    r#"{"timestamp":"2026-10-12T10:00:01","level":"INFO","target":"sheets","message":"加载完成","fields":{"entries":3,"skipped":1}}"#,
    r#"{"timestamp":"2026-10-12T10:00:02","level":"WARN","target":"sheets","message":"跳过空单词行","fields":{"row":5}}"#,
    r#"{"timestamp":"2026-10-12T10:00:03","level":"WARN","target":"verify","message":"slide 2 可能空白","fields":{"slide":2,"reason":"blank"}}"#,
    r#"{"timestamp":"2026-10-12T10:00:04","level":"ERROR","target":"render","message":"渲染失败","fields":{"slide":3,"renderer":"libreoffice"}}"#,
    r#"{"timestamp":"2026-10-12T10:00:05","level":"WARN","target":"verify","message":"密度偏低","fields":{"slide":4,"reason":"low_density","density_pct":1.2}}"#,
    r#"{"timestamp":"2026-10-12T10:00:06","level":"INFO","target":"fonts","message":"字体链探测","fields":{"chain":"cjk","font":"Noto Sans CJK SC"}}"#,
    r#"{"timestamp":"2026-10-12T10:00:07","level":"INFO","target":"output","message":"落盘","fields":{"path":"/data/PPT/words.pptx","slides":4}}"#,
    r#"{"timestamp":"2026-10-12T10:00:08","level":"WARN","target":"verify","message":"渲染超时","fields":{"slide":9,"reason":"timeout"}}"#,
    r#"{"timestamp":"2026-10-12T10:00:09","level":"WARN","target":"renderer","message":"可疑空白","fields":{"slide":1,"reason":"blank"}}"#,
    r#"{"timestamp":"2026-10-12T10:00:10","level":"INFO","target":"render","message":"渲染完成","fields":{"slide":3,"renderer":"libreoffice"}}"#,
    "",
    "   ",
    "not-json-line",
    r#"{"truncated": "json""#,
];

fn write_log(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generate_20261012_100000.log");
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    (dir, path)
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    write_log(FIXTURE_LINES)
}

#[test]
fn summary_renders_counts_targets_and_recent_issues() {
    let (_dir, path) = fixture();
    let report = query::run(&path, &Query::Summary).unwrap();
    assert!(!report.is_empty());

    let text = report.render_human();
    assert!(text.contains(&format!("诊断文件: {}", path.display())));
    assert!(text.contains("── 汇总 ──"));
    assert!(text.contains(&format!("  总事件: {TOTAL_EVENTS}\n")));
    assert!(text.contains("  INFO:   5\n"));
    assert!(text.contains("  WARN:   5\n"));
    assert!(text.contains("  ERROR:  1\n"));
    assert!(
        text.contains(&format!("跳过坏行 {BAD_LINES} 行")),
        "坏行不静默：{text}"
    );

    let targets: Vec<&str> = text
        .split("── 按模块 ──\n")
        .nth(1)
        .expect("按模块小节的正文")
        .lines()
        .take_while(|line| !line.starts_with("──"))
        .collect();
    assert_eq!(
        targets,
        vec![
            "  verify: 3",
            "  render: 2",
            "  sheets: 2",
            "  fonts: 1",
            "  output: 1",
            "  renderer: 1",
            "  session: 1",
        ],
        "按模块：计数降序、同数按名字升序"
    );

    assert!(text.contains("── 最近 5 条问题 ──"));
    let recent: Vec<&str> = text
        .split("── 最近 5 条问题 ──\n")
        .nth(1)
        .expect("最近问题小节的正文")
        .lines()
        .map(|line| line.rsplit(": ").next().unwrap())
        .collect();
    assert_eq!(
        recent,
        vec!["可疑空白", "渲染超时", "密度偏低", "渲染失败", "slide 2 可能空白"],
        "最近问题：新→旧，最多 5 条"
    );
}

#[test]
fn summary_json_exposes_counts_targets_recent_and_bad_lines() {
    let (_dir, path) = fixture();
    let json = query::run(&path, &Query::Summary).unwrap().to_json();

    assert_eq!(json["query"], "summary");
    assert_eq!(json["counts"]["total"], TOTAL_EVENTS);
    assert_eq!(json["counts"]["info"], 5);
    assert_eq!(json["counts"]["warn"], 5);
    assert_eq!(json["counts"]["error"], 1);
    assert_eq!(json["bad_lines"], BAD_LINES);
    assert_eq!(json["log"], path.display().to_string());

    assert_eq!(json["targets"][0]["target"], "verify");
    assert_eq!(json["targets"][0]["count"], 3);
    assert_eq!(json["targets"].as_array().unwrap().len(), 7);

    assert_eq!(json["recent"].as_array().unwrap().len(), 5);
    assert_eq!(json["recent"][0]["message"], "可疑空白");

    assert_eq!(
        json["events"].as_array().unwrap().len(),
        TOTAL_EVENTS,
        "Summary 计数覆盖整份日志"
    );
    // 原样保真：不认识的键也不丢（--json 是原始事件的搬运）。
    assert_eq!(json["events"][0]["extra"], "keep");
    assert_eq!(json["events"][0]["fields"]["command"], "generate");
}

#[test]
fn blank_slides_keeps_only_verify_events_with_blank_or_low_density() {
    let (_dir, path) = fixture();
    let report = query::run(&path, &Query::BlankSlides).unwrap();
    let json = report.to_json();
    assert_eq!(json["query"], "blank_slides");

    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["target"], "verify");
    assert_eq!(events[0]["fields"]["slide"], 2);
    assert_eq!(events[0]["fields"]["reason"], "blank");
    assert_eq!(events[1]["fields"]["reason"], "low_density");

    // reason=timeout 的 verify 事件、target≠verify 的 reason=blank 事件都不进空白清单。
    let text = report.render_human();
    assert!(!text.contains("渲染超时"));
    assert!(!text.contains("可疑空白"));
    assert!(text.contains("[WARN] 2026-10-12T10:00:03 verify: slide 2 可能空白"));
}

#[test]
fn font_trace_selects_fonts_target_events() {
    let (_dir, path) = fixture();
    let report = query::run(&path, &Query::FontTrace).unwrap();
    let json = report.to_json();
    assert_eq!(json["query"], "font_trace");

    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["target"], "fonts");
    assert_eq!(events[0]["fields"]["chain"], "cjk");

    let text = report.render_human();
    assert!(text.contains("[INFO] 2026-10-12T10:00:06 fonts: 字体链探测"));
    assert!(text.contains("  chain: \"cjk\""));
    assert!(text.contains("  font: \"Noto Sans CJK SC\""));
}

#[test]
fn errors_selects_warn_and_error_events() {
    let (_dir, path) = fixture();
    let report = query::run(&path, &Query::Errors).unwrap();
    let json = report.to_json();
    assert_eq!(json["query"], "errors");

    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 6, "5 条 WARN + 1 条 ERROR");
    assert!(events
        .iter()
        .all(|event| matches!(event["level"].as_str(), Some("WARN") | Some("ERROR"))));

    let text = report.render_human();
    assert!(text.contains("[ERROR] 2026-10-12T10:00:04 render: 渲染失败"));
    assert!(text.contains("  renderer: \"libreoffice\""));
    assert!(!text.contains("加载完成"), "INFO 不进错误清单");
}

#[test]
fn slide_query_matches_field_regardless_of_level() {
    let (_dir, path) = fixture();

    let json = query::run(&path, &Query::Slide(3)).unwrap().to_json();
    assert_eq!(json["query"], "slide");
    assert_eq!(json["slide"], 3);
    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "同一页的 INFO 与 ERROR 都算命中");
    assert!(events.iter().all(|event| event["fields"]["slide"] == 3));

    let slide_two = query::run(&path, &Query::Slide(2)).unwrap().to_json();
    let events = slide_two["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["message"], "slide 2 可能空白");
}

#[test]
fn all_returns_every_valid_event() {
    let (_dir, path) = fixture();
    let report = query::run(&path, &Query::All).unwrap();
    let json = report.to_json();
    assert_eq!(json["query"], "all");
    assert_eq!(json["events"].as_array().unwrap().len(), TOTAL_EVENTS);
    assert_eq!(json["counts"]["total"], TOTAL_EVENTS);

    let text = report.render_human();
    assert!(text.starts_with(&format!("跳过坏行 {BAD_LINES} 行")));
    assert!(text.contains("[INFO] 2026-10-12T10:00:00 session: session"));
    assert!(text.contains("  command: \"generate\""));
    assert!(text.contains("  fallback: false"));
}

#[test]
fn empty_result_is_empty_and_renders_placeholder() {
    let (_dir, path) = fixture();

    let report = query::run(&path, &Query::Slide(99)).unwrap();
    assert!(report.is_empty());
    assert!(report.render_human().contains("(无匹配事件)"));
    assert!(report.to_json()["events"].as_array().unwrap().is_empty());

    let (_clean_dir, clean) = write_log(&[
        r#"{"timestamp":"2026-10-12T10:00:00","level":"INFO","target":"session","message":"session"}"#,
    ]);
    assert!(query::run(&clean, &Query::Errors).unwrap().is_empty());
    let summary = query::run(&clean, &Query::Summary).unwrap().render_human();
    assert!(summary.contains("  WARN:   0\n"));
    assert!(!summary.contains("最近"), "没有问题就没有最近问题小节");
    assert!(!summary.contains("跳过坏行"));
}

#[test]
fn invalid_lines_are_counted_as_bad_lines_not_events() {
    let (_dir, path) = write_log(&[
        r#"{"timestamp":"2026-10-12T10:00:00","target":"session","message":"缺级别"}"#,
        r#"{"timestamp":"2026-10-12T10:00:01","level":"DEBUG","target":"session","message":"未知级别"}"#,
        "[1, 2, 3]",
        r#"{"timestamp":"2026-10-12T10:00:02","level":"info","target":"session","message":"小写级别"}"#,
        "   ",
    ]);
    let report = query::run(&path, &Query::All).unwrap();
    let json = report.to_json();
    assert_eq!(json["bad_lines"], 3);
    assert_eq!(
        json["events"].as_array().unwrap().len(),
        1,
        "只有小写 info 那行是合法事件（级别大小写不敏感）"
    );
    assert_eq!(json["counts"]["info"], 1);
    assert!(report.render_human().contains("跳过坏行 3 行"));
}

#[test]
fn missing_log_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.log");

    let error = query::run(&path, &Query::All).unwrap_err();
    assert!(matches!(error, QueryError::NotFound(_)));
    let message = error.to_string();
    assert!(message.contains("未找到日志"));
    assert!(message.contains("missing.log"));
}

#[test]
fn engine_reads_what_the_diag_producer_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (diag, path) = diag::start_session(dir.path(), "generate", false);
    let path = path.expect("会话日志");

    diag.record_session(fields(&[
        (field::VERSION, json!("0.2.0")),
        (field::COMMAND, json!("generate")),
        (field::FALLBACK, json!(false)),
    ]));
    diag.warn(
        Target::Verify,
        "slide 1 可能空白",
        Some(fields(&[
            (field::SLIDE, json!(1)),
            (field::REASON, json!(field::REASON_BLANK)),
        ])),
    );
    diag.info(
        Target::Fonts,
        "字体链探测",
        Some(fields(&[(field::CHAIN, json!("cjk"))])),
    );

    let blank = query::run(&path, &Query::BlankSlides).unwrap().to_json();
    assert_eq!(blank["events"].as_array().unwrap().len(), 1);
    assert_eq!(blank["events"][0]["target"], Target::Verify.as_str());

    let trace = query::run(&path, &Query::FontTrace).unwrap().to_json();
    assert_eq!(trace["events"][0]["fields"][field::CHAIN], "cjk");

    let summary = query::run(&path, &Query::Summary).unwrap().to_json();
    assert_eq!(summary["counts"]["total"], 3);
    assert_eq!(summary["counts"]["warn"], 1);
    assert_eq!(summary["bad_lines"], 0, "生产端写出的每一行都应是合法事件");
}

/// 用 `field` 常量拼 fields：生产端与查询端共用同一份键名。
fn fields(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect(),
    )
}
