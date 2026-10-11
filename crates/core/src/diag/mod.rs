//! 诊断：句柄 + 汇 + NDJSON 事件 + 会话日志。
//!
//! 模型（保留旧版已验证的部分）：
//! - [`Diag`] 可克隆句柄，`&self` 记录，作为函数最后参数传递；
//! - [`DiagSink`] 是 seam：`FileSink`（NDJSON 追加 + flush）/ `StderrSink`（ERROR 镜像）/
//!   `MemorySink`（测试回读）；
//! - 装配在边界：CLI 每进程 File + Stderr；GUI 每会话 File；测试 Memory。
//!
//! 契约：target 与 field 键的唯一来源见 [`contract`]；查询引擎见 [`query`]。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use serde_json::{json, Value};

pub mod contract;
pub mod query;

pub use contract::{field, Target};

/// 日志目录保留的最新日志份数。
pub const MAX_LOGS: usize = 20;

/// 事件级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// 一条 NDJSON 事件（一行一个）。
#[derive(Debug, Clone, Serialize)]
pub struct DiagEvent {
    pub timestamp: String,
    pub level: Level,
    pub target: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
}

/// 事件的接收端（seam）。
pub trait DiagSink: Send + Sync {
    fn record(&self, event: &DiagEvent) -> std::io::Result<()>;
}

/// NDJSON 文件汇：每事件一行，实时 flush。
pub struct FileSink {
    file: Mutex<fs::File>,
}

impl FileSink {
    pub fn create(path: &Path) -> std::io::Result<FileSink> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(FileSink {
            file: Mutex::new(file),
        })
    }
}

impl DiagSink for FileSink {
    fn record(&self, event: &DiagEvent) -> std::io::Result<()> {
        let line = serde_json::to_string(event)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let mut file = lock(&self.file);
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()
    }
}

/// stderr 汇：只镜像 ERROR。
#[derive(Debug, Default)]
pub struct StderrSink;

impl DiagSink for StderrSink {
    fn record(&self, event: &DiagEvent) -> std::io::Result<()> {
        if event.level == Level::Error {
            eprintln!(
                "[错误] {} {}: {}",
                event.timestamp, event.target, event.message
            );
        }
        Ok(())
    }
}

/// 内存汇（测试回读）。
#[derive(Debug, Clone, Default)]
pub struct MemorySink {
    events: Arc<Mutex<Vec<DiagEvent>>>,
}

impl MemorySink {
    pub fn new() -> MemorySink {
        MemorySink::default()
    }

    pub fn events(&self) -> Vec<DiagEvent> {
        lock(&self.events).clone()
    }
}

impl DiagSink for MemorySink {
    fn record(&self, event: &DiagEvent) -> std::io::Result<()> {
        lock(&self.events).push(event.clone());
        Ok(())
    }
}

#[derive(Default)]
struct Inner {
    sinks: Vec<Box<dyn DiagSink>>,
    events: AtomicUsize,
    warnings: AtomicUsize,
    errors: AtomicUsize,
    write_failures: AtomicUsize,
    last_failure: Mutex<Option<String>>,
}

/// 诊断句柄。
#[derive(Clone)]
pub struct Diag {
    inner: Arc<Inner>,
}

impl Diag {
    /// 无汇句柄（计数仍工作；用于不关心输出的调用点与测试）。
    pub fn new() -> Diag {
        Self::with_sinks(Vec::new())
    }

    pub fn with_sinks(sinks: Vec<Box<dyn DiagSink>>) -> Diag {
        Diag {
            inner: Arc::new(Inner {
                sinks,
                ..Inner::default()
            }),
        }
    }

    pub fn info(&self, target: Target, message: impl Into<String>, fields: Option<Value>) {
        self.log(Level::Info, target, message.into(), fields);
    }

    pub fn warn(&self, target: Target, message: impl Into<String>, fields: Option<Value>) {
        self.log(Level::Warn, target, message.into(), fields);
    }

    pub fn error(&self, target: Target, message: impl Into<String>, fields: Option<Value>) {
        self.log(Level::Error, target, message.into(), fields);
    }

    fn log(&self, level: Level, target: Target, message: String, fields: Option<Value>) {
        self.inner.events.fetch_add(1, Ordering::Relaxed);
        match level {
            Level::Warn => {
                self.inner.warnings.fetch_add(1, Ordering::Relaxed);
            }
            Level::Error => {
                self.inner.errors.fetch_add(1, Ordering::Relaxed);
            }
            Level::Info => {}
        }
        let event = DiagEvent {
            timestamp: timestamp(),
            level,
            target: target.as_str(),
            message,
            fields,
        };
        for sink in &self.inner.sinks {
            if let Err(error) = sink.record(&event) {
                self.inner.write_failures.fetch_add(1, Ordering::Relaxed);
                *lock(&self.inner.last_failure) = Some(error.to_string());
            }
        }
    }

    /// 会话首行（版本 / 命令 / 参数 / 数据目录 / 可执行文件 / 系统）。
    pub fn record_session(&self, fields: Value) {
        self.info(Target::Session, "session", Some(fields));
    }

    pub fn event_count(&self) -> usize {
        self.inner.events.load(Ordering::Relaxed)
    }

    pub fn warnings(&self) -> usize {
        self.inner.warnings.load(Ordering::Relaxed)
    }

    pub fn errors(&self) -> usize {
        self.inner.errors.load(Ordering::Relaxed)
    }

    /// 汇写入失败次数（日志系统自身故障）。
    pub fn write_failures(&self) -> usize {
        self.inner.write_failures.load(Ordering::Relaxed)
    }

    pub fn last_failure(&self) -> Option<String> {
        lock(&self.inner.last_failure).clone()
    }
}

/// 打开一次会话：`<logs_dir>/<prefix>_YYYYMMDD_HHMMSS.log`（同名追加 `-n`），
/// 随后滚动保留最新 [`MAX_LOGS`] 份。
///
/// 失败**不阻塞运行**：返回只有 stderr 汇的句柄与 `None`。
pub fn start_session(
    logs_dir: &Path,
    prefix: &str,
    mirror_stderr: bool,
) -> (Diag, Option<PathBuf>) {
    let path = match create_session_file(logs_dir, prefix) {
        Ok(path) => path,
        Err(error) => {
            let diag = Diag::with_sinks(vec![Box::new(StderrSink)]);
            diag.error(
                Target::Session,
                format!("无法创建日志目录: {error}"),
                Some(json!({"dir": logs_dir.display().to_string()})),
            );
            return (diag, None);
        }
    };
    let sink = match FileSink::create(&path) {
        Ok(sink) => sink,
        Err(error) => {
            let diag = Diag::with_sinks(vec![Box::new(StderrSink)]);
            diag.error(
                Target::Session,
                format!("无法创建日志文件: {error}"),
                Some(json!({"path": path.display().to_string()})),
            );
            return (diag, None);
        }
    };
    let mut sinks: Vec<Box<dyn DiagSink>> = vec![Box::new(sink)];
    if mirror_stderr {
        sinks.push(Box::new(StderrSink));
    }
    let diag = Diag::with_sinks(sinks);
    let removed = prune_logs(logs_dir, MAX_LOGS);
    if removed > 0 {
        diag.info(
            Target::Session,
            "old logs pruned",
            Some(json!({"count": removed})),
        );
    }
    (diag, Some(path))
}

fn create_session_file(logs_dir: &Path, prefix: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(logs_dir)?;
    let base = format!("{prefix}_{}", chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let mut path = logs_dir.join(format!("{base}.log"));
    let mut n = 1;
    while path.exists() {
        path = logs_dir.join(format!("{base}-{n}.log"));
        n += 1;
    }
    Ok(path)
}

/// 最新一份日志（按文件名排序；时间戳可排序）。
pub fn newest_log(logs_dir: &Path) -> Option<PathBuf> {
    let mut files = log_files(logs_dir);
    files.pop()
}

/// 滚动：仅保留最新 `keep` 份；返回删除数量。
pub fn prune_logs(logs_dir: &Path, keep: usize) -> usize {
    let files = log_files(logs_dir);
    if files.len() <= keep {
        return 0;
    }
    let mut removed = 0;
    for path in &files[..files.len() - keep] {
        if fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn log_files(logs_dir: &Path) -> Vec<PathBuf> {
    let Ok(reader) = fs::read_dir(logs_dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = reader
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "log").unwrap_or(false))
        .collect();
    files.sort();
    files
}

fn timestamp() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
        .to_string()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_diag() -> (Diag, MemorySink) {
        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone()) as Box<dyn DiagSink>]);
        (diag, sink)
    }

    #[test]
    fn events_and_counters_track_levels() {
        let (diag, sink) = memory_diag();
        diag.info(Target::Sheets, "a", None);
        diag.warn(Target::Sheets, "b", Some(json!({"row": 3})));
        diag.error(Target::Sheets, "c", None);
        assert_eq!(diag.event_count(), 3);
        assert_eq!(diag.warnings(), 1);
        assert_eq!(diag.errors(), 1);
        let events = sink.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[1].level, Level::Warn);
        assert_eq!(events[1].target, "sheets");
        assert_eq!(events[1].fields.as_ref().unwrap()["row"], 3);
    }

    #[test]
    fn sink_failure_is_counted_not_silent() {
        struct FailingSink;
        impl DiagSink for FailingSink {
            fn record(&self, _: &DiagEvent) -> std::io::Result<()> {
                Err(std::io::Error::other("disk full"))
            }
        }
        let diag = Diag::with_sinks(vec![Box::new(FailingSink)]);
        diag.error(Target::Session, "boom", None);
        assert_eq!(diag.write_failures(), 1);
        assert_eq!(diag.last_failure().as_deref(), Some("disk full"));
    }

    #[test]
    fn session_file_is_ndjson_and_first_event_is_session() {
        let dir = tempfile::tempdir().unwrap();
        let (diag, path) = start_session(dir.path(), "generate", false);
        let path = path.expect("session file");
        diag.record_session(json!({"version": "0.2.0", "command": "generate"}));
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let value: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(value["target"], "session");
        assert_eq!(value["level"], "INFO");
        assert_eq!(value["fields"]["command"], "generate");
    }

    #[test]
    fn prune_keeps_newest_and_counts_removed() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..25 {
            std::fs::write(
                dir.path().join(format!("cmd_20260101_000000-{i:02}.log")),
                b"x",
            )
            .unwrap();
        }
        let removed = prune_logs(dir.path(), 20);
        assert_eq!(removed, 5);
        assert_eq!(log_files(dir.path()).len(), 20);
        // 删除的是最旧的：编号 00..=04 不存在，编号 05 存在。
        assert!(!dir.path().join("cmd_20260101_000000-00.log").exists());
        assert!(dir.path().join("cmd_20260101_000000-05.log").exists());
    }

    #[test]
    fn start_session_prunes_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..25 {
            std::fs::write(dir.path().join(format!("old_20260101_00000{i}.log")), b"x").unwrap();
        }
        let (_diag, path) = start_session(dir.path(), "export", false);
        assert!(path.is_some());
        assert_eq!(log_files(dir.path()).len(), MAX_LOGS);
    }

    #[test]
    fn newest_log_picks_latest_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a_20260101_010101.log"), b"x").unwrap();
        std::fs::write(dir.path().join("a_20260101_020202.log"), b"x").unwrap();
        std::fs::write(dir.path().join("not-a-log.txt"), b"x").unwrap();
        assert_eq!(
            newest_log(dir.path()).unwrap().file_name().unwrap(),
            "a_20260101_020202.log"
        );
    }
}
