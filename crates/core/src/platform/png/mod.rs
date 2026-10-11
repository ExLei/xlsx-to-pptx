//! PNG 渲染：端口 + 两个适配器。
//!
//! - **端口** [`PngRenderer`]：把 pptx 变成 `slide_N.png` 的能力；
//! - **适配器** [`external`]（PowerPoint / LibreOffice / WPS，数据表驱动，3 个后端不是 3 份代码）；
//! - **内置管线** [`builtin`]：无模板模式，直接由词条渲染（不走 pptx）。

pub mod builtin;
pub mod external;
pub mod fonts;

use std::path::{Path, PathBuf};

use crate::diag::Diag;
use crate::ports::OpObserver;

/// 外呼渲染器选择；`Auto` = 探测顺序 PowerPoint → LibreOffice → WPS。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RendererChoice {
    #[default]
    Auto,
    PowerPoint,
    LibreOffice,
    Wps,
}

impl RendererChoice {
    pub fn name(self) -> &'static str {
        match self {
            RendererChoice::Auto => "auto",
            RendererChoice::PowerPoint => "PowerPoint",
            RendererChoice::LibreOffice => "LibreOffice",
            RendererChoice::Wps => "WPS",
        }
    }

    /// CLI `--renderer` 解析（大小写不敏感，接受常见别名）。
    pub fn parse(value: &str) -> Option<RendererChoice> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Some(RendererChoice::Auto),
            "powerpoint" | "ppt" | "office" => Some(RendererChoice::PowerPoint),
            "libreoffice" | "soffice" | "lo" => Some(RendererChoice::LibreOffice),
            "wps" => Some(RendererChoice::Wps),
            _ => None,
        }
    }
}

/// 渲染器端口。
pub trait PngRenderer {
    fn name(&self) -> &'static str;
    /// 渲染到 `dir`，返回 `slide_N.png`（1 起，排序）。
    /// 取消时必须杀掉子进程并返回 [`RenderError::Cancelled`]。
    fn render(
        &self,
        pptx: &Path,
        dir: &Path,
        observer: &mut dyn OpObserver,
        diag: &Diag,
    ) -> Result<Vec<PathBuf>, RenderError>;
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("未检测到可用的外部渲染器")]
    NoRenderer,
    #[error("未找到渲染器: {name}")]
    Missing { name: String },
    #[error("渲染失败（{renderer}）: {detail}")]
    Failed { renderer: String, detail: String },
    #[error("渲染已取消")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum PngError {
    #[error("字体探测失败: {0}")]
    Fonts(String),
    #[error("第 {slide} 页 SVG 生成失败: {detail}")]
    Svg { slide: usize, detail: String },
    #[error("第 {slide} 页渲染失败: {detail}")]
    Render { slide: usize, detail: String },
    #[error("第 {slide} 页编码失败: {detail}")]
    Encode { slide: usize, detail: String },
    #[error("导出已取消")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 选择外部渲染器（唯一选择点；探测顺序在 [`external`]）。
pub fn select(choice: RendererChoice, diag: &Diag) -> Result<Box<dyn PngRenderer>, RenderError> {
    external::select(choice, diag)
}

/// 渲染器安装指引（CLI 与 GUI 共用一份文案）。
pub fn renderer_help() -> &'static str {
    RENDERER_HELP
}

pub const RENDERER_HELP: &str = "未检测到可用的 PPTX→PNG 渲染器。安装以下任意一个即可：\n  \
     · Microsoft PowerPoint（Windows 桌面版）\n  \
     · LibreOffice（https://www.libreoffice.org/）\n  \
     · WPS Office（https://www.wps.cn/）";
