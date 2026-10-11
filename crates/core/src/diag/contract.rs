//! 诊断契约：target 与 `fields` 键名的**唯一来源**。
//!
//! 生产端（各模块）与消费端（[`super::query`]）共用同一常量；
//! `docs/diagnostics.md` 的表格由本模块同步维护（一致性测试见 query 模块）。

use std::fmt;

/// 事件 target。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Session,
    Paths,
    Sheets,
    Template,
    Pptx,
    Renderer,
    Render,
    Fonts,
    Verify,
    Output,
    Batch,
    Gui,
    Watcher,
}

impl Target {
    pub const ALL: [Target; 13] = [
        Target::Session,
        Target::Paths,
        Target::Sheets,
        Target::Template,
        Target::Pptx,
        Target::Renderer,
        Target::Render,
        Target::Fonts,
        Target::Verify,
        Target::Output,
        Target::Batch,
        Target::Gui,
        Target::Watcher,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Target::Session => "session",
            Target::Paths => "paths",
            Target::Sheets => "sheets",
            Target::Template => "template",
            Target::Pptx => "pptx",
            Target::Renderer => "renderer",
            Target::Render => "render",
            Target::Fonts => "fonts",
            Target::Verify => "verify",
            Target::Output => "output",
            Target::Batch => "batch",
            Target::Gui => "gui",
            Target::Watcher => "watcher",
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `fields` 键名契约：可被查询/断言的事实必须落进这些键。
pub mod field {
    pub const SLIDE: &str = "slide";
    pub const SLIDES: &str = "slides";
    pub const COUNT: &str = "count";
    pub const TOTAL: &str = "total";
    pub const PATH: &str = "path";
    pub const DIR: &str = "dir";
    pub const NAME: &str = "name";
    /// 空白判定原因：`blank` 或 `low_density`。
    pub const REASON: &str = "reason";
    pub const REASON_BLANK: &str = "blank";
    pub const REASON_LOW_DENSITY: &str = "low_density";
    pub const RENDERER: &str = "renderer";
    pub const EXE: &str = "exe";
    pub const FONT: &str = "font";
    /// 字体链：`latin` / `cjk` / `emoji`。
    pub const CHAIN: &str = "chain";
    pub const MISSING: &str = "missing";
    pub const DENSITY_PCT: &str = "density_pct";
    pub const ENTRIES: &str = "entries";
    pub const SKIPPED: &str = "skipped";
    pub const ROW: &str = "row";
    pub const COLUMNS: &str = "columns";
    pub const SHEET: &str = "sheet";
    pub const ENCODING: &str = "encoding";
    pub const VERSION: &str = "version";
    pub const COMMAND: &str = "command";
    pub const ARGS: &str = "args";
    pub const DATA_DIR: &str = "data_dir";
    /// 数据目录是否回退到用户目录。
    pub const FALLBACK: &str = "fallback";
    pub const EXE_PATH: &str = "exe_path";
    pub const OS: &str = "os";
    pub const OP: &str = "op";
}
