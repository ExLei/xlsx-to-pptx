#![windows_subsystem = "windows"]

#[cfg(target_os = "windows")]
mod win_console {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;

    extern "system" {
        fn AttachConsole(dwProcessId: u32) -> i32;
    }

    /// CLI 模式下附加到父进程控制台，确保 println/eprintln 有输出。
    pub fn ensure_console() {
        unsafe {
            // AttachConsole 失败也无妨（比如从非控制台启动），静默忽略
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

use gpui_kit::component::Theme;
use gpui_kit::*;
use gui::app;

const APP_TITLE: &str = "英语助记卡片生成";

fn main() {
    // 有命令行参数 → CLI 模式；无参数 → GUI 模式
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        // CLI 模式 — 附加到父进程控制台以输出信息
        #[cfg(target_os = "windows")]
        win_console::ensure_console();

        if let Err(e) = cli::run() {
            eprintln!("{e}");
            std::process::exit(1);
        }
    } else {
        run_gui();
    }
}

fn run_gui() {
    application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            // 必须先于任何组件创建，注册主题、遮罩层与输入设施。
            gpui_kit::init(cx);
            // 框架默认停在浅色；按系统的明暗设置开局，之后的切换由视图里的开关负责。
            Theme::sync_system_appearance(None, cx);

            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(APP_TITLE.into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1040.), px(760.)),
                    cx,
                ))),
                window_min_size: Some(size(px(760.), px(560.))),
                ..Default::default()
            };

            // open_window 自动把视图包进 Base Root，通知、对话框与菜单层才能工作。
            open_window(options, cx, |window, cx| {
                cx.new(|cx| app::VocabApp::new(window, cx))
            })
            .expect("无法打开窗口");
        });
}
