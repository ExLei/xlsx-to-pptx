//! 数据目录：程序产物的家（解析与回退规则的唯一来源）。
//!
//! 布局：`<root>/{日志, PPT, PNG, 单词表模板.xlsx, 示例模板.pptx}`；
//! 解析：程序同目录优先，不可写时回退用户数据目录（结果记录进 session 事件）。
//!
//! 分工：[`Layout::resolve`] 只提供环境路径（可执行文件目录 / 平台用户数据根），
//! 判定与回退逻辑全在 [`Layout::resolve_with`]——两者是同一实现的两个入口。

use std::path::{Path, PathBuf};

/// 数据目录名（兼容面，不可改）。
pub const DATA_DIR_NAME: &str = "英语助记卡片生成数据";

/// 可写性探针文件名（对等旧版；判定后即删除）。
const PROBE_FILE: &str = ".write-probe";

/// 数据目录布局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
    fallback: bool,
}

impl Layout {
    /// 解析当前进程的数据目录（程序目录优先，不可写回退用户数据目录）。
    pub fn resolve() -> Layout {
        Layout::resolve_with(&exe_dir(), &user_data_root())
    }

    /// 显式给定程序目录与用户数据根目录（测试与 `resolve` 共用同一实现）。
    ///
    /// 程序目录下的数据目录能建且能写 → 用它；否则回退用户数据根下的同名目录。
    pub fn resolve_with(exe_dir: &Path, user_data_root: &Path) -> Layout {
        let preferred = exe_dir.join(DATA_DIR_NAME);
        if writable(&preferred) {
            Layout {
                root: preferred,
                fallback: false,
            }
        } else {
            Layout {
                root: user_data_root.join(DATA_DIR_NAME),
                fallback: true,
            }
        }
    }

    /// 直接在指定根目录上构造（测试与显式落点）。
    pub fn with_root(root: impl Into<PathBuf>) -> Layout {
        Layout {
            root: root.into(),
            fallback: false,
        }
    }

    /// 是否回退到了用户数据目录（进 session 事件 `fields.fallback`）。
    pub fn is_fallback(&self) -> bool {
        self.fallback
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("日志")
    }

    pub fn ppt_dir(&self) -> PathBuf {
        self.root.join("PPT")
    }

    pub fn png_dir(&self) -> PathBuf {
        self.root.join("PNG")
    }

    pub fn template_xlsx(&self) -> PathBuf {
        self.root.join("单词表模板.xlsx")
    }

    pub fn template_pptx(&self) -> PathBuf {
        self.root.join("示例模板.pptx")
    }

    /// generate / batch 的默认 PPTX 落点。
    pub fn ppt_file(&self, stem: &str) -> PathBuf {
        self.ppt_dir().join(format!("{stem}.pptx"))
    }
}

/// 目录可写性探测：确保目录存在，并用探针文件验证真的能写。
///
/// 只靠 `create_dir_all` 会漏掉"目录已存在但只读"这一最常见的情形。
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(PROBE_FILE);
    if std::fs::write(&probe, b"").is_err() {
        return false;
    }
    // 清理探针。删除失败不改变"可写"结论（目录已被证明可写），残留的只是一个空文件；
    // 本层没有 Diag 句柄（签名冻结），判定结果由调用方进 session 事件（design §7.3）。
    drop(std::fs::remove_file(&probe));
    true
}

/// 程序所在目录（解析符号链接，取不到时退回当前目录）。
fn exe_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    // 解析符号链接；解析不到就用原路径（对等旧版）。
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 回退根：平台用户数据目录，环境变量缺席时用临时目录兜底。
#[cfg(target_os = "windows")]
fn user_data_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(target_os = "macos")]
fn user_data_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn user_data_root() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        // 空串视作未设置（对等旧版）。
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share"))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn user_data_root() -> PathBuf {
    std::env::temp_dir()
}
