//! GUI 前端：层次状态机 + 视图 + 命令执行。
//!
//! 结构（见 `docs/design.md` §8）：
//! - `state`：纯逻辑状态机（`reduce(state, event) -> Vec<Command>`），**无窗口可测**；
//! - `views`：只读状态渲染，把用户动作翻译成事件；
//! - `exec`：执行命令（spawn 作业/探针、通知、打开路径）。
//!
//! M2 起填充；入口形态：双击（无参数）→ GUI。

pub mod state;

use std::process::ExitCode;

/// GUI 入口（由组合根在无参数时调用）。
pub fn run() -> ExitCode {
    unimplemented!("M2：见 docs/design.md §8")
}
