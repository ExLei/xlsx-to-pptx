//! CLI 前端：参数映射 + 结果渲染 + 退出码。
//!
//! 只依赖 `x2p-core`（ops / domain / diag / paths）；不引用任何 platform 细节。
//! 业务编排一律在 ops 用例层；这里只有 clap 解析、stdout 渲染与退出码。

use std::process::ExitCode;

/// 完整 CLI 入口（`args` 含 argv[0]；供组合根与集成测试共用）。
pub fn run_from<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let _ = args;
    unimplemented!("M1：见 docs/design.md §5.2 / §9")
}
