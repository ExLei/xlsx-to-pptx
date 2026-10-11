//! 外呼渲染器：PowerPoint（COM / PowerShell）/ LibreOffice / WPS。
//!
//! 三个后端是**数据表里的三行**（探测命令 + 渲染命令 + 输出匹配），不是三份代码；
//! 取消贯通：等待循环轮询观察者，取消即 `kill` 子进程。

use std::path::{Path, PathBuf};

use crate::diag::Diag;
use crate::ports::OpObserver;

use super::{PngRenderer, RenderError, RendererChoice};

/// 探测并选择后端；`Auto` 顺序：PowerPoint → LibreOffice(`libreoffice`/`soffice`) → WPS(`wps`)。
pub fn select(choice: RendererChoice, diag: &Diag) -> Result<Box<dyn PngRenderer>, RenderError> {
    let _ = (choice, diag);
    unimplemented!("M1：见 docs/design.md §6.3")
}

/// 数据表驱动的一个后端。
pub struct ExternalCli {
    name: &'static str,
    executable: PathBuf,
}

impl PngRenderer for ExternalCli {
    fn name(&self) -> &'static str {
        self.name
    }

    fn render(
        &self,
        pptx: &Path,
        dir: &Path,
        observer: &mut dyn OpObserver,
        diag: &Diag,
    ) -> Result<Vec<PathBuf>, RenderError> {
        let _ = (pptx, dir, observer, diag, &self.executable);
        unimplemented!("M1")
    }
}
