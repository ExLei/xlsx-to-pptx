//! 诊断查询引擎：CLI `diag` 子命令与 GUI 摘要共用。
//!
//! 语义（`docs/design.md` §7.2 + 旧版 `diag` 子命令的兼容面）：
//! - [`Query::Summary`]：总数 / 等级计数 / 按 target 计数（降序）/ 最近 5 条 WARN|ERROR（新→旧）；
//! - [`Query::BlankSlides`]：`target = verify` 且 `fields.reason ∈ {blank, low_density}`；
//! - [`Query::FontTrace`]：`target = fonts` 的探针事件；
//! - [`Query::Errors`]：全部 WARN|ERROR；
//! - [`Query::Slide(n)`]：`fields.slide == n`（不看级别）；
//! - [`Query::All`]：全量。
//!
//! 解析契约（target 与 field 键名的唯一来源是 [`super::contract`]，此处不写字面量）：
//! - 合法事件 = JSON 对象且 `level ∈ {INFO, WARN, ERROR}`（大小写不敏感）；
//!   其余非空行计入坏行数，经 [`Report::render_human`] 提示、[`Report::to_json`] 上报，
//!   **不静默丢弃**；空行既不是事件也不是坏行。
//! - [`Query::Summary`] 命中全部事件，计数与"最近问题"覆盖整份日志。
//! - `--summary` 与 `--json` 互不排斥：汇总文本与结构化数据由调用方分别取
//!   [`Report::render_human`] / [`Report::to_json`] 组合。
//!
//! 修复（旧版缺陷）：坏行计数并报告；`--summary --json` 可共存。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::{field, Level, Target};

/// 摘要里"最近问题"的条数（对等旧版）。
const RECENT_ISSUES: usize = 5;

/// 查询类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Summary,
    BlankSlides,
    FontTrace,
    Errors,
    Slide(usize),
    All,
}

impl Query {
    /// JSON `query` 字段用的短名。
    fn name(&self) -> &'static str {
        match self {
            Query::Summary => "summary",
            Query::BlankSlides => "blank_slides",
            Query::FontTrace => "font_trace",
            Query::Errors => "errors",
            Query::Slide(_) => "slide",
            Query::All => "all",
        }
    }

    /// 该查询是否命中事件。
    fn matches(&self, event: &Event) -> bool {
        match self {
            Query::Summary | Query::All => true,
            Query::Errors => event.is_issue(),
            Query::BlankSlides => {
                event.target() == Target::Verify.as_str()
                    && matches!(
                        event.field(field::REASON).and_then(Value::as_str),
                        Some(field::REASON_BLANK) | Some(field::REASON_LOW_DENSITY)
                    )
            }
            Query::FontTrace => event.target() == Target::Fonts.as_str(),
            Query::Slide(n) => event.field(field::SLIDE).and_then(Value::as_u64) == Some(*n as u64),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("未找到日志: {0}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 查询结果。
#[derive(Debug)]
pub struct Report {
    log: PathBuf,
    query: Query,
    /// 全部合法事件；原始 JSON 原样保留（`--json` 输出不失真）。
    events: Vec<Event>,
    /// 命中事件在 `events` 中的下标（升序）。
    matched: Vec<usize>,
    counts: Counts,
    /// 每个 target 的事件数，降序（同数按名字升序，输出确定）。
    by_target: Vec<(String, usize)>,
    /// 最近 [`RECENT_ISSUES`] 条 WARN|ERROR 的下标（新→旧）。
    recent: Vec<usize>,
    /// 无法解析为事件的行数。
    bad_lines: usize,
}

impl Report {
    /// 是否没有匹配事件。
    pub fn is_empty(&self) -> bool {
        self.matched.is_empty()
    }

    /// 人类可读文本（以换行结尾，`print!` 直接可用）。
    ///
    /// [`Query::Summary`] 是特例：版式对等旧版 `diag --summary`（诊断文件 / 汇总 /
    /// 按模块 / 最近问题）；其余查询逐条打印 `[级别] 时间 target: message` 与缩进的 fields，
    /// 无命中时打印 `(无匹配事件)`。
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        if self.query == Query::Summary {
            out.push_str(&format!("诊断文件: {}\n", self.log.display()));
            self.push_bad_lines(&mut out);
            out.push_str("── 汇总 ──\n");
            out.push_str(&format!("  总事件: {}\n", self.counts.total()));
            out.push_str(&format!("  INFO:   {}\n", self.counts.info));
            out.push_str(&format!("  WARN:   {}\n", self.counts.warn));
            out.push_str(&format!("  ERROR:  {}\n", self.counts.error));
            if !self.by_target.is_empty() {
                out.push_str("── 按模块 ──\n");
                for (target, count) in &self.by_target {
                    out.push_str(&format!("  {target}: {count}\n"));
                }
            }
            if !self.recent.is_empty() {
                out.push_str(&format!("── 最近 {} 条问题 ──\n", self.recent.len()));
                for &index in &self.recent {
                    let event = &self.events[index];
                    out.push_str(&format!(
                        "  [{}] {}: {}\n",
                        level_name(event.level),
                        event.target(),
                        event.message()
                    ));
                }
            }
        } else {
            self.push_bad_lines(&mut out);
            if self.matched.is_empty() {
                out.push_str("(无匹配事件)\n");
            }
            for &index in &self.matched {
                let event = &self.events[index];
                out.push_str(&format!(
                    "[{}] {} {}: {}\n",
                    level_name(event.level),
                    event.timestamp(),
                    event.target(),
                    event.message()
                ));
                if let Some(fields) = event.raw.get("fields").and_then(Value::as_object) {
                    for (key, value) in fields {
                        out.push_str(&format!("  {key}: {value}\n"));
                    }
                }
            }
        }
        out
    }

    /// 脚本消费（`--json`）与 GUI 摘要共用的结构化形态。
    ///
    /// 键：`log` / `query`（+ `slide`）/ `counts{total,info,warn,error}` /
    /// `targets[{target,count}]`（降序）/ `recent`（最近 WARN|ERROR，新→旧）/
    /// `bad_lines` / `events`（命中事件，原样）。
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("log".into(), json!(self.log.display().to_string()));
        object.insert("query".into(), json!(self.query.name()));
        if let Query::Slide(n) = self.query {
            object.insert("slide".into(), json!(n));
        }
        object.insert(
            "counts".into(),
            json!({
                "total": self.counts.total(),
                "info": self.counts.info,
                "warn": self.counts.warn,
                "error": self.counts.error,
            }),
        );
        let targets: Vec<Value> = self
            .by_target
            .iter()
            .map(|(target, count)| json!({ "target": target, "count": count }))
            .collect();
        object.insert("targets".into(), Value::Array(targets));
        object.insert("recent".into(), self.raw_events(&self.recent));
        object.insert("bad_lines".into(), json!(self.bad_lines));
        object.insert("events".into(), self.raw_events(&self.matched));
        Value::Object(object)
    }

    fn raw_events(&self, indexes: &[usize]) -> Value {
        let events: Vec<Value> = indexes
            .iter()
            .map(|&index| self.events[index].raw.clone())
            .collect();
        Value::Array(events)
    }

    /// 坏行不静默：有坏行就提示数量。
    fn push_bad_lines(&self, out: &mut String) {
        if self.bad_lines > 0 {
            out.push_str(&format!(
                "跳过坏行 {} 行（无法解析为事件）\n",
                self.bad_lines
            ));
        }
    }
}

/// 读取并解析日志，运行查询。
pub fn run(log: &Path, query: &Query) -> Result<Report, QueryError> {
    if !log.exists() {
        return Err(QueryError::NotFound(log.display().to_string()));
    }
    let content = std::fs::read_to_string(log)?;
    Ok(analyze(log, query, &content))
}

/// 一条合法事件：保留原始 JSON（输出保真），另缓存级别（过滤与计数）。
#[derive(Debug)]
struct Event {
    raw: Value,
    level: Level,
}

impl Event {
    /// 合法事件 = JSON 对象且带可识别级别；其余返回 `None`（调用方计坏行）。
    fn parse(value: Value) -> Option<Event> {
        if !value.is_object() {
            return None;
        }
        let level = parse_level(value.get("level").and_then(Value::as_str)?)?;
        Some(Event { raw: value, level })
    }

    fn target(&self) -> &str {
        self.raw
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("?")
    }

    fn message(&self) -> &str {
        self.raw
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("?")
    }

    fn timestamp(&self) -> &str {
        self.raw
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("?")
    }

    fn field(&self, key: &str) -> Option<&Value> {
        self.raw.get("fields").and_then(|fields| fields.get(key))
    }

    fn is_issue(&self) -> bool {
        matches!(self.level, Level::Warn | Level::Error)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    info: usize,
    warn: usize,
    error: usize,
}

impl Counts {
    fn total(&self) -> usize {
        self.info + self.warn + self.error
    }
}

fn analyze(log: &Path, query: &Query, content: &str) -> Report {
    let mut events: Vec<Event> = Vec::new();
    let mut bad_lines = 0;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line).ok().and_then(Event::parse) {
            Some(event) => events.push(event),
            None => bad_lines += 1,
        }
    }

    let mut counts = Counts::default();
    let mut by_target: BTreeMap<&str, usize> = BTreeMap::new();
    let mut recent: Vec<usize> = Vec::new();
    for (index, event) in events.iter().enumerate() {
        match event.level {
            Level::Info => counts.info += 1,
            Level::Warn => counts.warn += 1,
            Level::Error => counts.error += 1,
        }
        *by_target.entry(event.target()).or_insert(0) += 1;
        if event.is_issue() {
            recent.push(index);
        }
    }
    recent.reverse();
    recent.truncate(RECENT_ISSUES);

    let mut by_target: Vec<(String, usize)> = by_target
        .into_iter()
        .map(|(target, count)| (target.to_string(), count))
        .collect();
    by_target.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let matched: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| query.matches(event))
        .map(|(index, _)| index)
        .collect();

    Report {
        log: log.to_path_buf(),
        query: query.clone(),
        events,
        matched,
        counts,
        by_target,
        recent,
        bad_lines,
    }
}

fn parse_level(raw: &str) -> Option<Level> {
    if raw.eq_ignore_ascii_case("INFO") {
        Some(Level::Info)
    } else if raw.eq_ignore_ascii_case("WARN") {
        Some(Level::Warn)
    } else if raw.eq_ignore_ascii_case("ERROR") {
        Some(Level::Error)
    } else {
        None
    }
}

/// NDJSON 与人类可读输出共用的级别名（大写，对等旧版）。
fn level_name(level: Level) -> &'static str {
    match level {
        Level::Info => "INFO",
        Level::Warn => "WARN",
        Level::Error => "ERROR",
    }
}
