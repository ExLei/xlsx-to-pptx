//! 数据目录与默认落点。
//!
//! 程序自己产生的文件只有一个默认根：程序同目录的 `英语助记卡片生成数据/`，
//! 不可写时回退到用户数据目录。显式传入的路径永远优先，本模块只给缺省值。

use std::path::{Path, PathBuf};

/// 数据目录名。
pub const DATA_DIR_NAME: &str = "英语助记卡片生成数据";

/// 数据目录布局：`日志/`、`PPT/`、`PNG/` 与两个模板文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// 解析默认数据目录：程序同目录优先，不可写时回退用户数据目录。
    pub fn resolve() -> Layout {
        Layout::resolve_with(&exe_dir(), &user_data_root())
    }

    /// 用指定的程序目录与回退根解析（测试与嵌入场景）。
    pub fn resolve_with(exe_dir: &Path, fallback_root: &Path) -> Layout {
        let preferred = exe_dir.join(DATA_DIR_NAME);
        let root = if writable(&preferred) {
            preferred
        } else {
            fallback_root.join(DATA_DIR_NAME)
        };
        Layout { root }
    }

    /// 用指定的数据目录根构造。
    pub fn with_root(root: impl Into<PathBuf>) -> Layout {
        Layout { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 运行日志目录。
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("日志")
    }

    /// PPTX 输出目录。
    pub fn ppt_dir(&self) -> PathBuf {
        self.root.join("PPT")
    }

    /// PNG 输出目录。
    pub fn png_dir(&self) -> PathBuf {
        self.root.join("PNG")
    }

    /// Excel 词汇表模板的默认落点。
    pub fn template_xlsx(&self) -> PathBuf {
        self.root.join("单词表模板.xlsx")
    }

    /// 示例 PPTX 模板的默认落点。
    pub fn template_pptx(&self) -> PathBuf {
        self.root.join("示例模板.pptx")
    }

    /// PPTX 输出的默认文件：`PPT/<stem>.pptx`。
    pub fn ppt_file(&self, stem: &str) -> PathBuf {
        self.ppt_dir().join(format!("{stem}.pptx"))
    }
}

/// 目录可写性探测：确保目录存在，并用探针文件验证真的能写。
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 程序所在目录（解析符号链接，取不到时退回当前目录）。
fn exe_dir() -> PathBuf {
    let exe = std::env::current_exe()
        .and_then(|path| std::fs::canonicalize(&path).or(Ok(path)))
        .unwrap_or_default();
    exe.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 回退根：平台用户数据目录，环境变量缺席时用临时目录兜底。
fn user_data_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support");
        }
        return std::env::temp_dir();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg);
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share");
        }
        return std::env::temp_dir();
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        std::env::temp_dir()
    }
}
