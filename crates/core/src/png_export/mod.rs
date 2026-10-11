pub mod pipeline;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use image::Rgba;

use crate::diag::DiagStore;
use crate::types::WordEntry;

const PNG_W: u32 = 1920;
const PNG_H: u32 = 1080;

#[derive(Debug, Clone)]
pub struct PlaceholderLayout {
    pub name: String,
    pub x: i32,
    pub h: u32,
    pub y: i32,
    pub w: u32,
    pub font_size_pt: f32,
    pub bold: bool,
    pub color: Rgba<u8>,
    pub align_center: bool,
    pub font_family: Option<String>,
    /// Text vertical anchor: "t" (top), "ctr" (center), or "b" (bottom)
    pub text_anchor: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PngExportError {
    #[error("unavailable font")]
    NoFontFound,
    #[error("font database load failed: {0}")]
    FontLoadError(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
    #[error("template parse: {0}")]
    TemplateParse(String),
}
/// 1920×1080 画布对应 16:9 幻灯片的 16in×9in = 1152pt，即 1pt = 1.667px。
///
/// 模板解析出的字号是磅值，写进 SVG 前必须换算成画布像素，否则整版文字会小一半。
pub const PT_TO_PX: f32 = PNG_W as f32 / (16.0 * 72.0);

/// 无模板时的内置版式槽位。
struct DefaultSlot {
    name: &'static str,
    /// 字段名前缀，与示例模板中的 `单词释义：{{单词释义}}` 一致。
    label: &'static str,
    x: f32,
    /// 基线坐标。
    baseline: f32,
    font_px: f32,
    color: &'static str,
    centered: bool,
}

/// 内置版式：与 `template-pptx` 生成的示例模板同坐标（1920×1080 画布）。
///
/// 坐标取自示例模板的文本框：单词/音标居中，标签列左起 240px，
/// 正文 24pt、行距 96px。
const DEFAULT_LAYOUT: [DefaultSlot; 6] = [
    DefaultSlot {
        name: "单词",
        label: "",
        x: 960.,
        baseline: 240.,
        font_px: 120.,
        color: "#1e1e1e",
        centered: true,
    },
    DefaultSlot {
        name: "音标",
        label: "",
        x: 960.,
        baseline: 325.,
        font_px: 46.7,
        color: "#646464",
        centered: true,
    },
    DefaultSlot {
        name: "单词释义",
        label: "单词释义：",
        x: 240.,
        baseline: 452.,
        font_px: 40.,
        color: "#1e1e1e",
        centered: false,
    },
    DefaultSlot {
        name: "词根词缀",
        label: "词根词缀：",
        x: 240.,
        baseline: 548.,
        font_px: 40.,
        color: "#1e1e1e",
        centered: false,
    },
    DefaultSlot {
        name: "例句",
        label: "例句：",
        x: 240.,
        baseline: 644.,
        font_px: 40.,
        color: "#1e1e1e",
        centered: false,
    },
    DefaultSlot {
        name: "例句释义",
        label: "例句释义：",
        x: 240.,
        baseline: 692.,
        font_px: 40.,
        color: "#646464",
        centered: false,
    },
];

/// Render a single slide as an SVG string for resvg-based PNG export.
///
/// Generates a 1920×1080 SVG. Fields present in `layout` use the template's
/// coordinates (with `font_size_pt` converted to canvas pixels); the remaining
/// fields fall back to [`DEFAULT_LAYOUT`], which mirrors the example template's
/// geometry.
pub fn render_slide_to_svg(
    entry: &WordEntry,
    layout: &HashMap<String, PlaceholderLayout>,
) -> String {
    let mut svg = String::new();
    let w = PNG_W;
    let h = PNG_H;
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {} {}" width="{}" height="{}">"##,
        w, h, w, h,
    ));
    // White background
    svg.push_str(&format!(
        r##"<rect width="{}" height="{}" fill="#ffffff"/>"##,
        w, h,
    ));

    let fields: [(&str, &str); 6] = [
        ("单词", &entry.word),
        ("音标", &entry.phonetic),
        ("单词释义", &entry.definition),
        ("词根词缀", &entry.morphology),
        ("例句", &entry.example),
        ("例句释义", &entry.example_definition),
    ];

    for (name, value) in fields {
        if value.is_empty() {
            continue;
        }
        match layout.get(name) {
            // 模板版式：坐标与锚点来自模板，字号由磅换算成画布像素
            Some(pl) => {
                let font_px = pl.font_size_pt * PT_TO_PX;
                let baseline = match pl.text_anchor.as_str() {
                    "ctr" => pl.y as f32 + pl.h as f32 / 2. + font_px / 3.,
                    "b" => pl.y as f32 + pl.h as f32 - 4.,
                    _ => pl.y as f32 + font_px, // "t"（默认）
                };
                let font_weight = if pl.bold { "bold" } else { "normal" };
                let color_hex =
                    format!("#{:02x}{:02x}{:02x}", pl.color[0], pl.color[1], pl.color[2]);
                let font_family = pl.font_family.as_deref().unwrap_or("sans-serif");
                svg.push_str(&format!(
                    r##"<text x="{}" y="{}" font-family="{font_family}, sans-serif" font-size="{}" font-weight="{font_weight}" fill="{color_hex}">{}</text>"##,
                    fmt_px(pl.x as f32),
                    fmt_px(baseline),
                    fmt_px(font_px),
                    xml_escape(value),
                ));
            }
            // 内置版式：与示例模板同坐标
            None => {
                let Some(slot) = DEFAULT_LAYOUT.iter().find(|slot| slot.name == name) else {
                    continue;
                };
                let text = format!("{}{value}", slot.label);
                let anchor = if slot.centered {
                    r#" text-anchor="middle""#
                } else {
                    ""
                };
                svg.push_str(&format!(
                    r#"<text x="{}" y="{}"{anchor} font-family="sans-serif" font-size="{}" fill="{}">{}</text>"#,
                    fmt_px(slot.x),
                    fmt_px(slot.baseline),
                    fmt_px(slot.font_px),
                    slot.color,
                    xml_escape(&text),
                ));
            }
        }
    }

    svg.push_str("</svg>");
    svg
}

/// 画布像素保留到 0.1，整数不写小数尾巴（120 而不是 120.0）。
fn fmt_px(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i32)
    } else {
        format!("{value:.1}")
    }
}

/// Escape special XML characters in text for SVG embedding
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── Rendering via SVG + resvg ──

/// Count the number of `<text>` elements in an SVG string.
pub fn count_text_nodes(svg: &str) -> usize {
    svg.match_indices("<text")
        .filter(|(i, _)| {
            let rest = &svg[*i..];
            rest.starts_with("<text ") || rest.starts_with("<text>") || rest.starts_with("<text/")
        })
        .count()
}

/// Count non-white pixels in a PNG byte buffer.
///
/// A pixel is considered non-white if any of its R, G, B channels is not 255.
pub fn count_non_white_pixels(png_bytes: &[u8]) -> Result<usize, PngExportError> {
    let img = image::load_from_memory(png_bytes)?;
    let rgba = img.to_rgba8();
    let count = rgba
        .pixels()
        .filter(|p| p[0] != 255 || p[1] != 255 || p[2] != 255)
        .count();
    Ok(count)
}

/// Render an SVG string to PNG bytes using resvg.
///
/// Uses the font stack from `font_config` for text rendering.
/// Unknown fonts are handled gracefully — resvg falls back to system defaults.
pub fn render_svg_to_png(
    svg: &str,
    font_config: &pipeline::FontConfig,
    diag: &mut DiagStore,
) -> Result<Vec<u8>, PngExportError> {
    let mut fontdb = usvg::fontdb::Database::new();
    let font_load_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fontdb.load_system_fonts();
    }));
    if font_load_result.is_err() {
        let msg =
            "fontdb.load_system_fonts() panicked — possibly corrupted system font".to_string();
        diag.error("render_svg_to_png", &msg, None);
        return Err(PngExportError::FontLoadError(msg));
    }

    // font_stack is a comma-separated CSS font-family list.
    // Find the first available font in the list for fontdb configuration.
    let families: Vec<&str> = font_config
        .font_stack
        .split(',')
        .map(|s| s.trim())
        .collect();
    let primary = families.first().copied().unwrap_or("sans-serif");
    fontdb.set_sans_serif_family(primary);
    fontdb.set_serif_family(primary);
    fontdb.set_monospace_family(primary);

    let opt = usvg::Options {
        fontdb: std::sync::Arc::new(fontdb),
        ..Default::default()
    };

    let rtree = usvg::Tree::from_str(svg, &opt)
        .map_err(|e| PngExportError::TemplateParse(format!("SVG parse: {e}")))?;

    let pixmap_size = rtree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(pixmap_size.width(), pixmap_size.height())
        .ok_or_else(|| PngExportError::TemplateParse("failed to create pixmap".into()))?;

    resvg::render(&rtree, usvg::Transform::default(), &mut pixmap.as_mut());

    let png_bytes = pixmap
        .encode_png()
        .map_err(|e| PngExportError::TemplateParse(format!("PNG encode: {e}")))?;

    // Post-render blank-slide detection
    let text_count = count_text_nodes(svg);
    let total_pixels: f64 = (PNG_W as u64 * PNG_H as u64) as f64;
    let non_white = count_non_white_pixels(&png_bytes).unwrap_or(0);
    let density = non_white as f64 / total_pixels;

    if density < 0.00001 && text_count > 0 {
        diag.error(
            "render_svg_to_png",
            &format!(
                "slide may be blank: {:.6}% non-white pixels ({} / {})",
                density * 100.0,
                non_white,
                total_pixels as u64,
            ),
            None,
        );
    } else if density < 0.0001 {
        diag.warn(
            "render_svg_to_png",
            &format!(
                "slide has very low content density: {:.6}% non-white pixels ({} / {})",
                density * 100.0,
                non_white,
                total_pixels as u64,
            ),
            None,
        );
    }

    Ok(png_bytes)
}

pub fn export_entries_to_png(
    entries: &[WordEntry],
    output_dir: &Path,
) -> Result<Vec<std::path::PathBuf>, PngExportError> {
    export_entries_to_png_with_cancel(entries, output_dir, None)
}

/// Like `export_entries_to_png` but with an optional cancel flag for cooperative cancellation.
pub fn export_entries_to_png_with_cancel(
    entries: &[WordEntry],
    output_dir: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<std::path::PathBuf>, PngExportError> {
    let mut diag = DiagStore::new();
    let font_cfg = pipeline::probe_fonts(&mut diag);
    let result = export_with_layout(
        entries,
        output_dir,
        &HashMap::new(),
        &font_cfg,
        cancel,
        &mut diag,
    );
    match diag.write_ndjson_to_file(&output_dir.join("export_diag.ndjson")) {
        Ok(_) => eprintln!(
            "  诊断日志: {}",
            output_dir.join("export_diag.ndjson").display()
        ),
        Err(e) => eprintln!("  警告: 无法写入诊断日志: {e}"),
    }
    result
}
/// Returns the path to the rendered PNG on success.
pub fn render_one_slide(
    entry: &WordEntry,
    layout: &HashMap<String, PlaceholderLayout>,
    font_config: &pipeline::FontConfig,
    output_dir: &Path,
    index: usize,
    diag: &mut DiagStore,
) -> Result<std::path::PathBuf, PngExportError> {
    let svg = render_slide_to_svg(entry, layout);
    let png_bytes = render_svg_to_png(&svg, font_config, diag)?;
    let path = output_dir.join(format!("slide_{}.png", index + 1));
    std::fs::write(&path, &png_bytes)?;
    Ok(path)
}

fn export_with_layout(
    entries: &[WordEntry],
    output_dir: &Path,
    layout: &HashMap<String, PlaceholderLayout>,
    font_config: &pipeline::FontConfig,
    cancel: Option<&AtomicBool>,
    diag: &mut DiagStore,
) -> Result<Vec<std::path::PathBuf>, PngExportError> {
    std::fs::create_dir_all(output_dir)?;
    let mut pngs = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(flag) = cancel {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                diag.warn("export", "export cancelled by user", None);
                break;
            }
        }
        match render_one_slide(entry, layout, font_config, output_dir, i, diag) {
            Ok(path) => pngs.push(path),
            Err(e) => {
                diag.error("export", &format!("slide {} failed: {}", i + 1, e), None);
            }
        }
    }
    if pngs.is_empty() && !entries.is_empty() {
        Err(PngExportError::Io(std::io::Error::other(
            "all slides failed",
        )))
    } else {
        Ok(pngs)
    }
}
