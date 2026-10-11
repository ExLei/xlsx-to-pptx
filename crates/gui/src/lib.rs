//! 图形界面：gpui-kit 视图、面板与后台任务管线。
//!
//! 二进制入口在 main.rs：无参数打开窗口，有参数走 CLI。

/// 窗口标题；窗口与 CLI 共用同一产品名。
pub const APP_TITLE: &str = "英语助记卡片生成";

pub mod app;
pub mod panels;
