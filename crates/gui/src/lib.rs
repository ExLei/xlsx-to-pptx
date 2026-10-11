//! GUI 前端：层次状态机 + 视图 + 命令执行。
//!
//! 结构（见 `docs/design.md` §8）：
//! - `state`：纯逻辑状态机（`reduce(state, event) -> Vec<Command>`），**无窗口可测**；
//! - `views`：只读状态渲染，把用户动作翻译成事件；
//! - `exec`：执行命令（spawn 作业/探针、通知、打开路径）。
//!
//! 入口形态：双击（无参数）→ GUI（组合根 `x2p-app` 在无参数时调用 [`run`]）。

pub mod exec;
pub mod state;
pub mod views;

use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;

use gpui_kit::component::Theme;
use gpui_kit::*;

/// 应用名（窗口标题与界面标题的唯一来源）。
pub const APP_TITLE: &str = "英语助记卡片生成";

/// `X2P_TEXT_MODE=gray|subpixel` 覆盖文本渲染模式；未设置或无法识别时返回 `None`（平台默认）。
fn text_mode_override() -> Option<TextRenderingMode> {
    match std::env::var("X2P_TEXT_MODE")
        .ok()?
        .to_ascii_lowercase()
        .as_str()
    {
        "gray" | "grayscale" => Some(TextRenderingMode::Grayscale),
        "subpixel" | "cleartype" => Some(TextRenderingMode::Subpixel),
        _ => None,
    }
}

/// GUI 入口（由组合根在无参数时调用）。
///
/// 装配顺序：`application` → `init`（主题/遮罩/输入设施）→ 跟随系统明暗 → 开窗；
/// 窗口里的 `views::VocabApp` 持有状态机与执行层，并在构造时打开会话日志、发起渲染器探针。
pub fn run() -> ExitCode {
    // 启动级故障（窗口打不开）要反映到退出码：闭包里捕获一个标志位带出来。
    let failed = Rc::new(Cell::new(false));
    let flag = failed.clone();

    application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            // 诊断/兼容开关：文本渲染模式可由环境变量覆盖（未设置 = 平台默认，行为不变）。
            // Windows 平台默认可走 ClearType 子像素路径；若该路径在目标机异常（典型症状：
            // 界面几何与图标正常但全部文字不显示），用 X2P_TEXT_MODE=gray 切到灰度路径验证。
            if let Some(mode) = text_mode_override() {
                eprintln!("[x2p] 文本渲染模式覆盖：{mode:?}");
                cx.set_text_rendering_mode(mode);
            }
            // 开局跟随系统明暗；之后的切换由状态栏的开关负责（视图侧，不进状态机）。
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
            if let Err(error) = open_window(options, cx, |window, cx| {
                cx.new(|cx| views::VocabApp::new(window, cx))
            }) {
                // 零静默失败（I2）：打不开窗口是启动级故障，必须可见。
                eprintln!("无法打开窗口: {error}");
                flag.set(true);
            }
        });

    if failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
