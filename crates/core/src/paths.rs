//! 数据目录：程序产物的家（解析与回退规则的唯一来源）。
//!
//! 布局：`<root>/{日志, PPT, PNG, 单词表模板.xlsx, 示例模板.pptx}`；
//! 解析：程序同目录优先，不可写时回退用户数据目录（结果记录进 session 事件）。

use std::path::{Path, PathBuf};

/// 数据目录名（兼容面，不可改）。
pub const DATA_DIR_NAME: &str = "英语助记卡片生成数据";

/// 数据目录布局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
    fallback: bool,
}

impl Layout {
    /// 解析当前进程的数据目录（程序目录优先，不可写回退用户数据目录）。
    pub fn resolve() -> Layout {
        unimplemented!("M1：见 docs/design.md §7.3")
    }

    /// 显式给定程序目录与用户数据根目录（测试与 `resolve` 共用同一实现）。
    pub fn resolve_with(exe_dir: &Path, user_data_root: &Path) -> Layout {
        let _ = (exe_dir, user_data_root);
        unimplemented!("M1")
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
