//! 组合根：唯一二进制（`英语助记卡片生成`）。
//!
//! 形态：双击（无参数）→ GUI；带参 → CLI。前端平级，组合只在此处。

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        x2p_cli::run_from(args)
    } else {
        x2p_gui::run()
    }
}
