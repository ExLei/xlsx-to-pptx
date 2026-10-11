//! 诊断：结构化事件与可装配的汇（适配器）。
//!
//! 生产者只持有一个可克隆的 [`Diag`] 句柄（`&self` 记录），
//! 文件 / stderr / 内存都是汇，由边界（CLI / GUI）在启动时装配。
//! 汇写入失败不阻塞主流程，但会记账，调用方可用 [`Diag::write_failures`] 查。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// 日志目录保留的最大日志数；建立新日志时清理超量旧日志。
pub const MAX_LOGS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DiagLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct DiagEvent {
    pub timestamp: String,
    pub level: DiagLevel,
    pub target: String,
    pub message: String,
    pub fields: Option<serde_json::Value>,
}

impl DiagEvent {
    /// 序列化为一行 NDJSON（不含换行）。
    pub fn to_ndjson(&self) -> String {
        #[derive(Serialize)]
        struct NdjsonLine<'a> {
            timestamp: &'a str,
            level: DiagLevel,
            target: &'a str,
            message: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            fields: Option<&'a serde_json::Value>,
        }

        let line = NdjsonLine {
            timestamp: &self.timestamp,
            level: self.level,
            target: &self.target,
            message: &self.message,
            fields: self.fields.as_ref(),
        };
        serde_json::to_string(&line).expect("NDJSON line serialization cannot fail")
    }

    /// `fields.slide`（1 起算的 slide 序号），用于 `diag --slide` 过滤。
    pub fn slide_index(&self) -> Option<u64> {
        self.fields
            .as_ref()
            .and_then(|fields| fields.get("slide"))
            .and_then(|slide| slide.as_u64())
    }

    /// `fields.reason`，用于空白 slide 等分类事件。
    pub fn reason(&self) -> Option<&str> {
        self.fields
            .as_ref()
            .and_then(|fields| fields.get("reason"))
            .and_then(|reason| reason.as_str())
    }
}

/// 本地时间戳，毫秒精度。
fn timestamp() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

/// 汇：事件的目的地。实现必须可跨线程共享。
pub trait DiagSink: Send + Sync {
    fn record(&self, event: &DiagEvent) -> Result<(), String>;
}

/// 文件汇：每事件一行 NDJSON，追加写并立即落盘（崩溃也留下已发生的事件）。
pub struct FileSink {
    file: Mutex<File>,
    path: PathBuf,
}

impl FileSink {
    /// 打开（或创建）日志文件，追加写入。
    pub fn create(path: &Path) -> std::io::Result<FileSink> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(FileSink {
            file: Mutex::new(file),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl DiagSink for FileSink {
    fn record(&self, event: &DiagEvent) -> Result<(), String> {
        let mut line = event.to_ndjson();
        line.push('\n');
        let mut file = self
            .file
            .lock()
            .map_err(|_| "log file lock poisoned".to_string())?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|error| error.to_string())
    }
}

/// stderr 镜像汇：只输出 ERROR 事件。
pub struct StderrSink;

impl DiagSink for StderrSink {
    fn record(&self, event: &DiagEvent) -> Result<(), String> {
        if event.level == DiagLevel::Error {
            eprintln!("[ERROR] {}: {}", event.target, event.message);
        }
        Ok(())
    }
}

/// 内存汇：测试与需要回读事件的场合。
#[derive(Clone, Default)]
pub struct MemorySink {
    events: Arc<Mutex<Vec<DiagEvent>>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<DiagEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

impl DiagSink for MemorySink {
    fn record(&self, event: &DiagEvent) -> Result<(), String> {
        self.events
            .lock()
            .map_err(|_| "memory sink lock poisoned".to_string())?
            .push(event.clone());
        Ok(())
    }
}

/// 事件句柄：克隆它传给各个函数，记录走 `&self`。
#[derive(Clone)]
pub struct Diag {
    inner: Arc<Inner>,
}

struct Inner {
    sinks: Vec<Box<dyn DiagSink>>,
    events: AtomicUsize,
    warnings: AtomicUsize,
    errors: AtomicUsize,
    write_failures: AtomicUsize,
    last_failure: Mutex<Option<String>>,
}

impl Diag {
    /// 不落任何汇的句柄（只计数的空实现，测试与省略诊断的场合）。
    pub fn new() -> Self {
        Self::with_sinks(Vec::new())
    }

    pub fn with_sinks(sinks: Vec<Box<dyn DiagSink>>) -> Self {
        Diag {
            inner: Arc::new(Inner {
                sinks,
                events: AtomicUsize::new(0),
                warnings: AtomicUsize::new(0),
                errors: AtomicUsize::new(0),
                write_failures: AtomicUsize::new(0),
                last_failure: Mutex::new(None),
            }),
        }
    }

    /// 正常流程里程碑。
    pub fn info(
        &self,
        target: &str,
        message: impl Into<String>,
        fields: Option<serde_json::Value>,
    ) {
        self.record(DiagLevel::Info, target, message.into(), fields);
    }

    /// 可恢复异常。
    pub fn warn(
        &self,
        target: &str,
        message: impl Into<String>,
        fields: Option<serde_json::Value>,
    ) {
        self.record(DiagLevel::Warn, target, message.into(), fields);
    }

    /// 错误（不阻止执行）。
    pub fn error(
        &self,
        target: &str,
        message: impl Into<String>,
        fields: Option<serde_json::Value>,
    ) {
        self.record(DiagLevel::Error, target, message.into(), fields);
    }

    fn record(
        &self,
        level: DiagLevel,
        target: &str,
        message: String,
        fields: Option<serde_json::Value>,
    ) {
        let event = DiagEvent {
            timestamp: timestamp(),
            level,
            target: target.to_string(),
            message,
            fields,
        };
        self.inner.events.fetch_add(1, Ordering::Relaxed);
        match level {
            DiagLevel::Warn => {
                self.inner.warnings.fetch_add(1, Ordering::Relaxed);
            }
            DiagLevel::Error => {
                self.inner.errors.fetch_add(1, Ordering::Relaxed);
            }
            DiagLevel::Info => {}
        }
        for sink in &self.inner.sinks {
            if let Err(error) = sink.record(&event) {
                self.inner.write_failures.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut last) = self.inner.last_failure.lock() {
                    *last = Some(error);
                }
            }
        }
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

    /// 汇写入失败的次数（日志系统自身的故障）。
    pub fn write_failures(&self) -> usize {
        self.inner.write_failures.load(Ordering::Relaxed)
    }

    /// 最近一次汇写入失败的原因。
    pub fn last_failure(&self) -> Option<String> {
        self.inner
            .last_failure
            .lock()
            .ok()
            .and_then(|last| last.clone())
    }
}

impl Default for Diag {
    fn default() -> Self {
        Self::new()
    }
}

/// 运行开始的第一行事件：版本、命令、参数、数据目录、可执行文件、系统。
pub fn record_session(diag: &Diag, command: &str, data_dir: &Path) {
    diag.info(
        "session",
        "run started",
        Some(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "command": command,
            "args": std::env::args().skip(1).collect::<Vec<String>>(),
            "data_dir": data_dir.display().to_string(),
            "exe": std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            "os": std::env::consts::OS,
        })),
    );
}

/// 建立一次运行的日志文件汇：建目录、清理超量旧日志、按 `<前缀>_YYYYMMDD_HHMMSS.log` 命名。
pub fn run_log_sink(dir: &Path, prefix: &str) -> std::io::Result<FileSink> {
    std::fs::create_dir_all(dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    for n in 1..1000u32 {
        let name = if n == 1 {
            format!("{prefix}_{stamp}.log")
        } else {
            format!("{prefix}_{stamp}-{n}.log")
        };
        let path = dir.join(name);
        match OpenOptions::new().create_new(true).append(true).open(&path) {
            Ok(file) => {
                prune_logs(dir, MAX_LOGS);
                return Ok(FileSink {
                    file: Mutex::new(file),
                    path,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("无法生成唯一日志文件名"))
}

/// 日志目录里最新的一个日志文件；目录不存在或为空时返回 None。
pub fn newest_log(dir: &Path) -> Option<PathBuf> {
    sorted_logs(dir).pop()
}

/// 只保留最新的 `keep` 个日志文件，其余删除；清理失败不影响运行。
fn prune_logs(dir: &Path, keep: usize) {
    let logs = sorted_logs(dir);
    let excess = logs.len().saturating_sub(keep);
    for path in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// 按（修改时间，文件名键）升序排列：最老在前。
///
/// 文件系统的修改时间粒度可能粗到毫秒级，同一秒内创建的多个日志会并列；
/// 文件名键（日期 → 时间 → 同秒序号）在并列时给出确定的先后。
fn sorted_logs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut logs: Vec<(std::time::SystemTime, LogNameKey, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().map(|ext| ext == "log").unwrap_or(false))
        .map(|path| {
            let modified = path
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let key = LogNameKey::parse(&path);
            (modified, key, path)
        })
        .collect();
    logs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    logs.into_iter().map(|(_, _, path)| path).collect()
}

/// 从 `<前缀>_<日期>_<时间>[-<序号>].log` 解析出的同秒排序键。
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct LogNameKey {
    date: String,
    time: String,
    seq: u32,
    prefix: String,
}

impl LogNameKey {
    fn parse(path: &Path) -> LogNameKey {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = name.strip_suffix(".log").unwrap_or(&name).to_string();
        let mut parts = stem.rsplitn(3, '_');
        let time_part = parts.next().unwrap_or_default().to_string();
        let date = parts.next().unwrap_or_default().to_string();
        let prefix = parts.next().unwrap_or_default().to_string();
        let (time, seq) = match time_part.split_once('-') {
            Some((time, seq)) => (time.to_string(), seq.parse::<u32>().unwrap_or(0)),
            None => (time_part, 0),
        };
        LogNameKey {
            date,
            time,
            seq,
            prefix,
        }
    }
}
