//! 原子落盘与目录工具（不变式 I1：先写临时文件，再改名；失败/取消不留半成品）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::diag::{Diag, Target};

/// 原子写：同目录临时文件 + 改名。失败时清理临时文件并记 diag。
pub fn write_atomic(path: &Path, bytes: &[u8], diag: &Diag) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = temp_path(path);
    if let Err(error) = write_then_rename(path, dir, &tmp, bytes) {
        diag.error(
            Target::Output,
            format!("写入失败: {error}"),
            Some(json!({"path": display(path)})),
        );
        if tmp.exists() {
            if let Err(cleanup) = fs::remove_file(&tmp) {
                diag.warn(
                    Target::Output,
                    format!("清理临时文件失败: {cleanup}"),
                    Some(json!({"path": display(&tmp)})),
                );
            }
        }
        return Err(error);
    }
    Ok(())
}

fn write_then_rename(path: &Path, dir: &Path, tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    {
        let mut file = fs::File::create(tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
    }
    match fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        // Windows 上 rename 不覆盖已有文件：先删目标再试一次。
        Err(_) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(tmp, path)
        }
        Err(error) => Err(error),
    }
}

/// 同目录唯一临时路径。
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.part-{}-{nanos}", std::process::id()))
}

/// 确保目录存在（失败记 error）。
pub fn ensure_dir(path: &Path, diag: &Diag) -> std::io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    match fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) => {
            diag.error(
                Target::Output,
                format!("创建目录失败: {error}"),
                Some(json!({"dir": display(path)})),
            );
            Err(error)
        }
    }
}

/// 删除文件；不存在视为成功，失败记 warn（不允许静默）。
pub fn remove_file(path: &Path, diag: &Diag) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => diag.warn(
            Target::Output,
            format!("删除失败: {error}"),
            Some(json!({"path": display(path)})),
        ),
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::{DiagSink, MemorySink};

    fn diag_with_sink() -> (Diag, MemorySink) {
        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone()) as Box<dyn DiagSink>]);
        (diag, sink)
    }

    #[test]
    fn write_atomic_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let (diag, _) = diag_with_sink();
        write_atomic(&path, b"hello", &diag).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn write_atomic_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, b"old").unwrap();
        let (diag, _) = diag_with_sink();
        write_atomic(&path, b"new", &diag).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn write_atomic_leaves_no_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        let (diag, _) = diag_with_sink();
        write_atomic(&path, b"data", &diag).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".part-"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
    }

    #[test]
    fn ensure_dir_creates_nested() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        let (diag, _) = diag_with_sink();
        ensure_dir(&nested, &diag).unwrap();
        assert!(nested.is_dir());
    }

    #[test]
    fn remove_file_missing_is_silent_success() {
        let dir = tempfile::tempdir().unwrap();
        let (diag, sink) = diag_with_sink();
        remove_file(&dir.path().join("missing"), &diag);
        assert!(sink.events().is_empty());
    }
}
