#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! 组合根：唯一二进制（`英语助记卡片生成`）。
//!
//! 形态：双击（无参数）→ GUI；带参 → CLI。前端平级，组合只在此处。

#[cfg(target_os = "windows")]
mod win_console {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;

    extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
    }

    /// CLI 模式下附加到父进程控制台，确保 println/eprintln 有输出。
    pub fn ensure_console() {
        // 失败无妨：从非控制台启动（双击）时本就不需要控制台。
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        #[cfg(target_os = "windows")]
        win_console::ensure_console();
        x2p_cli::run_from(args)
    } else {
        x2p_gui::run()
    }
}
