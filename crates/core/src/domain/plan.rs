//! 输出计划：产物落点的**唯一决策处**（目录 + 文件名 + 覆盖策略）。
//!
//! 旧版的三处策略（GUI `find_available_path`、CLI `--force`、generator `FileExists`）
//! 全部收进这里；调用点不得自行处理覆盖。

use std::path::{Path, PathBuf};

/// 覆盖策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePolicy {
    /// 已存在即报错。
    Fail,
    /// 直接覆盖（原子写会替换旧文件）。
    Overwrite,
    /// 自动找下一个空闲名：`word.pptx` → `word (1).pptx`（不叠号）。
    Suffix,
}

/// 一次文件产物的计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPlan {
    pub dir: PathBuf,
    /// 文件名（含扩展名），如 `words.pptx`。
    pub name: String,
    pub policy: OverwritePolicy,
}

/// 产物：路径 + 页数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub path: PathBuf,
    pub slides: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("输出已存在: {0}")]
    Exists(PathBuf),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

impl OutputPlan {
    pub fn new(dir: impl Into<PathBuf>, name: impl Into<String>, policy: OverwritePolicy) -> Self {
        Self {
            dir: dir.into(),
            name: name.into(),
            policy,
        }
    }

    /// 解析最终落点（只读文件系统，不创建目录、不写盘）。
    pub fn resolve(&self) -> Result<PathBuf, OutputError> {
        let path = self.dir.join(&self.name);
        match self.policy {
            OverwritePolicy::Overwrite => Ok(path),
            OverwritePolicy::Fail => {
                if path.exists() {
                    Err(OutputError::Exists(path))
                } else {
                    Ok(path)
                }
            }
            OverwritePolicy::Suffix => Ok(next_free(&self.dir, &self.name)),
        }
    }
}

/// `word.pptx` → `word (1).pptx` → `word (2).pptx`…；已有序号则不叠号。
fn next_free(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = split_ext(name);
    let base = strip_index_suffix(stem);
    let mut n: u32 = 1;
    loop {
        let candidate = dir.join(format!("{base} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn strip_index_suffix(stem: &str) -> &str {
    if let Some(rest) = stem.strip_suffix(')') {
        if let Some(open) = rest.rfind(" (") {
            let inner = &rest[open + 2..];
            if !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_digit()) {
                return &rest[..open];
            }
        }
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_policy_errors_when_target_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("words.pptx"), b"x").unwrap();
        let plan = OutputPlan::new(dir.path(), "words.pptx", OverwritePolicy::Fail);
        assert!(matches!(plan.resolve(), Err(OutputError::Exists(_))));
    }

    #[test]
    fn fail_policy_passes_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let plan = OutputPlan::new(dir.path(), "words.pptx", OverwritePolicy::Fail);
        assert_eq!(plan.resolve().unwrap(), dir.path().join("words.pptx"));
    }

    #[test]
    fn overwrite_policy_returns_target_as_is() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("words.pptx"), b"x").unwrap();
        let plan = OutputPlan::new(dir.path(), "words.pptx", OverwritePolicy::Overwrite);
        assert_eq!(plan.resolve().unwrap(), dir.path().join("words.pptx"));
    }

    #[test]
    fn suffix_policy_keeps_requested_path_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let plan = OutputPlan::new(dir.path(), "words.pptx", OverwritePolicy::Suffix);
        assert_eq!(plan.resolve().unwrap(), dir.path().join("words.pptx"));
    }

    #[test]
    fn suffix_policy_finds_next_free_number() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("words.pptx"), b"x").unwrap();
        let plan = OutputPlan::new(dir.path(), "words.pptx", OverwritePolicy::Suffix);
        assert_eq!(plan.resolve().unwrap(), dir.path().join("words (1).pptx"));
    }

    #[test]
    fn suffix_policy_does_not_stack_numbers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("words.pptx"), b"x").unwrap();
        std::fs::write(dir.path().join("words (1).pptx"), b"x").unwrap();
        // 请求名本身就是 `words (1)`，下一个空闲应回到基名继续: `words (2).pptx`
        let plan = OutputPlan::new(dir.path(), "words (1).pptx", OverwritePolicy::Suffix);
        assert_eq!(plan.resolve().unwrap(), dir.path().join("words (2).pptx"));
    }

    #[test]
    fn suffix_policy_handles_names_without_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("words"), b"x").unwrap();
        let plan = OutputPlan::new(dir.path(), "words", OverwritePolicy::Suffix);
        assert_eq!(plan.resolve().unwrap(), dir.path().join("words (1)"));
    }

    #[test]
    fn strip_index_suffix_edge_cases() {
        assert_eq!(strip_index_suffix("words"), "words");
        assert_eq!(strip_index_suffix("words (1)"), "words");
        assert_eq!(strip_index_suffix("words (12)"), "words");
        assert_eq!(strip_index_suffix("words (a)"), "words (a)");
        assert_eq!(strip_index_suffix("words ()"), "words ()");
        assert_eq!(strip_index_suffix("(1)"), "(1)");
        assert_eq!(strip_index_suffix("a (1) (2)"), "a (1)");
    }
}
