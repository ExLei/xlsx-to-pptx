//! PNG 导出用例：模板模式外呼渲染器；无模板模式内置 SVG 管线。

use std::path::{Path, PathBuf};

use super::{Finished, OpContext, OpError, OpResult, PngArtifact, RendererChoice};
use crate::diag::{field, Target};
use crate::domain::plan::{OutputPlan, OverwritePolicy};
use crate::domain::schema::WordEntry;
use crate::platform::png;
use crate::ports::Progress;

/// 导出 PNG 到 `dir`，逐页命名 `slide_N.png`（1 起）。
///
/// - 模板模式：先写出唯一临时 pptx（生成 → 渲染 → 清理；取消杀掉渲染子进程）；
/// - 无模板模式：内置 SVG 管线（字体链 → SVG → 渲染 → verify → 原子写）。
pub fn export_png(
    entries: &[WordEntry],
    template: Option<&Path>,
    renderer: RendererChoice,
    dir: &Path,
    cx: &mut OpContext,
) -> OpResult<PngArtifact> {
    cx.diag.info(
        Target::Render,
        "export png start",
        Some(serde_json::json!({
            "entries": entries.len(),
            "dir": dir.display().to_string(),
            "renderer": renderer.name(),
        })),
    );
    if entries.is_empty() {
        return Err(OpError::NoEntries);
    }

    let slides: Vec<PathBuf> = match template {
        Some(template_path) => {
            match export_via_renderer(entries, template_path, renderer, dir, cx)? {
                Finished::Completed(slides) => slides,
                Finished::Cancelled => return Ok(Finished::Cancelled),
            }
        }
        None => {
            if renderer != RendererChoice::Auto {
                cx.diag.error(
                    Target::Render,
                    "无模板模式只支持内置管线（--renderer 冲突）",
                    Some(serde_json::json!({"renderer": renderer.name()})),
                );
                return Err(png::RenderError::Failed {
                    renderer: renderer.name().into(),
                    detail: "无模板模式恒使用内置管线，不能指定外部渲染器".into(),
                }
                .into());
            }
            match png::builtin::render(entries, dir, cx.observer, cx.diag) {
                Ok(slides) => slides,
                Err(png::PngError::Cancelled) => return Ok(Finished::Cancelled),
                Err(error) => return Err(error.into()),
            }
        }
    };
    cx.diag.info(
        Target::Render,
        "export png done",
        Some(serde_json::json!({"dir": dir.display().to_string(), field::SLIDES: slides.len()})),
    );
    Ok(Finished::Completed(PngArtifact {
        dir: dir.to_path_buf(),
        slides,
    }))
}

/// 模板模式：唯一临时 pptx → 外呼渲染 → 清理（无论成败）。
fn export_via_renderer(
    entries: &[WordEntry],
    template: &Path,
    renderer: RendererChoice,
    dir: &Path,
    cx: &mut OpContext,
) -> OpResult<Vec<PathBuf>> {
    crate::platform::io::ensure_dir(dir, cx.diag)?;
    let renderer_impl = png::select(renderer, cx.diag)?;

    let temp_name = unique_temp_name();
    let generated = super::generate_deck(
        entries,
        Some(template),
        OutputPlan::new(dir.to_path_buf(), temp_name, OverwritePolicy::Overwrite),
        cx,
    );
    let generated = match generated {
        Ok(Finished::Completed(artifact)) => artifact,
        Ok(Finished::Cancelled) => return Ok(Finished::Cancelled),
        Err(error) => return Err(error),
    };

    if cx.cancelled(Progress::Stage("渲染")) {
        crate::platform::io::remove_file(&generated.path, cx.diag);
        return Ok(Finished::Cancelled);
    }
    let rendered = renderer_impl.render(&generated.path, dir, cx.observer, cx.diag);
    crate::platform::io::remove_file(&generated.path, cx.diag);
    match rendered {
        Ok(slides) => Ok(Finished::Completed(slides)),
        Err(png::RenderError::Cancelled) => Ok(Finished::Cancelled),
        Err(error) => Err(error.into()),
    }
}

/// 唯一临时文件名（绝不复用固定中间名，避免并发与残留互踩）。
fn unique_temp_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(".x2p-render-{}-{nanos}.pptx", std::process::id())
}
