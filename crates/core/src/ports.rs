//! 端口：位于层与层之间的 seam 类型（platform 与 ops 共用，前端实现）。
//!
//! 进度/取消是**接口自带能力**，不是每个调用点各自适配。

use std::ops::ControlFlow;

/// 进度事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// 阶段开始（"读取表格" / "填充模板" / "渲染" …）。
    Stage(&'static str),
    /// 逐页进度。
    Slides { done: usize, total: usize },
    /// 批量逐文件进度。
    Files { done: usize, total: usize },
}

/// 观察者端口，三个适配器：GUI / CLI / 测试假体。
///
/// 返回 [`ControlFlow::Break`] 表示用户请求取消；调用方在逐页/逐文件循环里检查。
pub trait OpObserver {
    fn on(&mut self, progress: &Progress) -> ControlFlow<()>;
}

/// 不关心进度的场景。
#[derive(Debug, Default)]
pub struct NoopObserver;

impl OpObserver for NoopObserver {
    fn on(&mut self, _progress: &Progress) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}
