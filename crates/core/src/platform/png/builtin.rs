//! 内置 SVG 管线（无模板模式）：字体链 → 逐页 SVG → resvg 渲染 → verify → 原子写。
//!
//! 版式数据（1920×1080 坐标、标签、字号）由 [`crate::domain::schema::FIELDS`] 驱动；
//! verify 产生 `reason ∈ {blank, low_density}` 事件（`diag --blank-slides` 契约）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use usvg::fontdb;

use crate::diag::{field, Diag, Target};
use crate::domain::schema::{WordEntry, FIELDS};
use crate::platform::io::{ensure_dir, write_atomic};
use crate::ports::{OpObserver, Progress};

use super::fonts::{self, FontConfig};
use super::PngError;

/// 画布尺寸（16:9，与示例模板同尺度）。
const CANVAS_W: u32 = 1920;
const CANVAS_H: u32 = 1080;

/// 内容密度阈值（非白像素占比）：低于 [`BLANK_DENSITY`] 且有文本 → `blank`；
/// 低于 [`LOW_DENSITY`] → `low_density`。
const BLANK_DENSITY: f64 = 0.00001;
const LOW_DENSITY: f64 = 0.0001;

/// 内置版式的槽位几何（与 [`FIELDS`] **同序同长**：`FIELDS[i]` 的槽位是 `SLOTS[i]`）。
///
/// 坐标取自示例模板的文本框：单词/音标居中，标签列左起 240px，正文 24pt、行距 96px。
struct Slot {
    /// 居中行：水平居中，且不写标签前缀（旧版此处标签为空）。
    centered: bool,
    x: f32,
    /// 基线（画布像素）。
    baseline: f32,
    font_px: f32,
    color: &'static str,
}

/// 槽位表（次序 = `FIELDS` 规范顺序：单词 / 音标 / 词根词缀 / 例句 / 例句释义 / 单词释义）。
const SLOTS: [Slot; 6] = [
    Slot {
        // 单词
        centered: true,
        x: 960.0,
        baseline: 240.0,
        font_px: 120.0,
        color: "#1e1e1e",
    },
    Slot {
        // 音标
        centered: true,
        x: 960.0,
        baseline: 325.0,
        font_px: 46.7,
        color: "#646464",
    },
    Slot {
        // 词根词缀
        centered: false,
        x: 240.0,
        baseline: 548.0,
        font_px: 40.0,
        color: "#1e1e1e",
    },
    Slot {
        // 例句
        centered: false,
        x: 240.0,
        baseline: 644.0,
        font_px: 40.0,
        color: "#1e1e1e",
    },
    Slot {
        // 例句释义
        centered: false,
        x: 240.0,
        baseline: 692.0,
        font_px: 40.0,
        color: "#646464",
    },
    Slot {
        // 单词释义
        centered: false,
        x: 240.0,
        baseline: 452.0,
        font_px: 40.0,
        color: "#1e1e1e",
    },
];

/// 渲染全部词条到 `dir`，逐页 `slide_N.png`（1 起）。
///
/// 取消（观察者返回 [`std::ops::ControlFlow::Break`]）时返回 [`PngError::Cancelled`]，
/// **已完成的页保留**；字体链每次 render 探测一次，逐页复用同一份字体库。
pub fn render(
    entries: &[WordEntry],
    dir: &Path,
    observer: &mut dyn OpObserver,
    diag: &Diag,
) -> Result<Vec<PathBuf>, PngError> {
    ensure_dir(dir, diag)?;
    let fonts = fonts::probe(diag);
    let db = fonts::render_db(&fonts);
    diag.info(
        Target::Render,
        format!("内置管线：渲染 {} 页 → {}", entries.len(), dir.display()),
        Some(json!({
            field::ENTRIES: entries.len(),
            field::DIR: dir.display().to_string(),
        })),
    );

    let mut slides: Vec<PathBuf> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let slide = index + 1;
        if observer
            .on(&Progress::Slides {
                done: index,
                total: entries.len(),
            })
            .is_break()
        {
            diag.warn(
                Target::Render,
                format!(
                    "内置管线已取消：已完成 {} / {} 页",
                    slides.len(),
                    entries.len()
                ),
                Some(json!({ field::SLIDES: slides.len(), field::TOTAL: entries.len() })),
            );
            return Err(PngError::Cancelled);
        }

        let svg = slide_svg(entry, &fonts);
        let pixmap = rasterize(&svg, &db, slide)?;
        let density_pct = verify(diag, slide, &svg, &pixmap);
        let png = pixmap
            .encode_png()
            .map_err(|error| PngError::Encode {
                slide,
                detail: error.to_string(),
            })?;
        let path = dir.join(format!("slide_{slide}.png"));
        write_atomic(&path, &png, diag)?;
        diag.info(
            Target::Render,
            format!("第 {slide} 页渲染完成"),
            Some(json!({
                field::SLIDE: slide,
                field::PATH: path.display().to_string(),
                field::DENSITY_PCT: density_pct,
            })),
        );
        slides.push(path);
    }

    diag.info(
        Target::Render,
        format!("内置管线完成：{} 页", slides.len()),
        Some(json!({
            field::SLIDES: slides.len(),
            field::DIR: dir.display().to_string(),
        })),
    );
    Ok(slides)
}

/// 一个词条 → 一页 SVG（1920×1080）。
///
/// 字段顺序与标签来自 [`FIELDS`]（规范顺序 + `label()`），几何来自 [`SLOTS`]；
/// 取值按原样渲染（不 trim、不换行），空字段整行跳过。
pub fn slide_svg(entry: &WordEntry, fonts: &FontConfig) -> String {
    let mut svg = String::with_capacity(768);
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {CANVAS_W} {CANVAS_H}\" \
         width=\"{CANVAS_W}\" height=\"{CANVAS_H}\">"
    ));
    svg.push_str(&format!(
        "<rect width=\"{CANVAS_W}\" height=\"{CANVAS_H}\" fill=\"#ffffff\"/>"
    ));
    svg.push_str("<g font-family=\"");
    escape_into(&mut svg, &fonts.stack());
    svg.push_str("\">");

    for (meta, slot) in FIELDS.iter().zip(&SLOTS) {
        let value = entry.value(meta.field);
        if value.is_empty() {
            continue;
        }
        let anchor = if slot.centered {
            " text-anchor=\"middle\""
        } else {
            ""
        };
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\"{anchor} font-size=\"{}\" fill=\"{}\">",
            fmt_px(slot.x),
            fmt_px(slot.baseline),
            fmt_px(slot.font_px),
            slot.color,
        ));
        if !slot.centered {
            escape_into(&mut svg, meta.label);
        }
        escape_into(&mut svg, value);
        svg.push_str("</text>");
    }

    svg.push_str("</g></svg>");
    svg
}

/// SVG → 位图（usvg 解析 + tiny-skia 光栅化）。
fn rasterize(
    svg: &str,
    db: &Arc<fontdb::Database>,
    slide: usize,
) -> Result<tiny_skia::Pixmap, PngError> {
    let options = usvg::Options {
        fontdb: Arc::clone(db),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).map_err(|error| PngError::Svg {
        slide,
        detail: error.to_string(),
    })?;
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).ok_or_else(|| {
        PngError::Render {
            slide,
            detail: format!("无法建立 {}×{} 位图", size.width(), size.height()),
        }
    })?;
    resvg::render(&tree, usvg::Transform::default(), &mut pixmap.as_mut());
    Ok(pixmap)
}

/// 渲染后验证：文本节点数 + 非白像素密度；异常记 warn（`reason ∈ {blank, low_density}`）。
///
/// 返回 `density_pct`（非白像素占比 × 100），供逐页事件复用。
fn verify(diag: &Diag, slide: usize, svg: &str, pixmap: &tiny_skia::Pixmap) -> f64 {
    let text_nodes = count_text_nodes(svg);
    let non_white = count_non_white_pixels(pixmap);
    let total = u64::from(CANVAS_W) * u64::from(CANVAS_H);
    let density = non_white as f64 / total as f64;
    let density_pct = density * 100.0;

    let reason = if density < BLANK_DENSITY && text_nodes > 0 {
        Some(field::REASON_BLANK)
    } else if density < LOW_DENSITY {
        Some(field::REASON_LOW_DENSITY)
    } else {
        None
    };
    if let Some(reason) = reason {
        diag.warn(
            Target::Verify,
            format!(
                "第 {slide} 页内容异常（{reason}）：{density_pct:.6}% 非白像素（{non_white} / {total}，{text_nodes} 个文本节点）"
            ),
            Some(json!({
                field::SLIDE: slide,
                field::REASON: reason,
                field::DENSITY_PCT: density_pct,
            })),
        );
    }
    density_pct
}

/// `<text>` 节点数（`<text ` / `<text>` / `<text/` 起始；`<textPath>` 之类不算）。
fn count_text_nodes(svg: &str) -> usize {
    svg.match_indices("<text")
        .filter(|(start, _)| {
            svg[*start..].starts_with("<text ") || svg[*start..].starts_with("<text/")
        })
        .count()
}

/// 非白像素数（R/G/B 任一通道非 255 即算；画布不透明，直接读位图，不必解码 PNG）。
fn count_non_white_pixels(pixmap: &tiny_skia::Pixmap) -> usize {
    pixmap
        .data()
        .chunks_exact(4)
        .filter(|pixel| pixel[0] != 255 || pixel[1] != 255 || pixel[2] != 255)
        .count()
}

/// 画布像素保留到 0.1，整数不写小数尾巴（120 而不是 120.0）。
fn fmt_px(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i32)
    } else {
        format!("{value:.1}")
    }
}

/// XML 转义后追加（不为每段文本分配中间字符串）。
fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::{Diag, Level, MemorySink};

    fn bitmap(svg: &str) -> tiny_skia::Pixmap {
        rasterize(svg, &Arc::new(fontdb::Database::new()), 1).expect("SVG 必须能光栅化")
    }

    /// 有文本节点但一个像素都没画出来 → `reason = blank`（warn，可被 `diag --blank-slides` 命中）。
    #[test]
    fn verify_flags_blank_when_text_is_invisible() {
        let svg = concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1920 1080\" ",
            "width=\"1920\" height=\"1080\">",
            "<rect width=\"1920\" height=\"1080\" fill=\"#ffffff\"/>",
            "<text x=\"80\" y=\"180\" font-size=\"72\" fill=\"#ffffff\">看不见</text>",
            "</svg>"
        );
        assert_eq!(count_text_nodes(svg), 1);

        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
        let density_pct = verify(&diag, 3, svg, &bitmap(svg));

        assert_eq!(density_pct, 0.0, "全白位图密度必须为 0");
        let events = sink.events();
        assert_eq!(events.len(), 1, "只应有一条 verify 事件");
        assert_eq!(events[0].target, "verify");
        assert_eq!(events[0].level, Level::Warn);
        let fields = events[0].fields.as_ref().expect("verify 事件必须带 fields");
        assert_eq!(fields[field::SLIDE], json!(3));
        assert_eq!(fields[field::REASON], json!("blank"));
        assert_eq!(fields[field::DENSITY_PCT], json!(0.0));
    }

    /// 有色块、且密度高于阈值 → 不报（verify 只在异常时发声）。
    #[test]
    fn verify_stays_quiet_above_threshold() {
        let svg = concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1920 1080\" ",
            "width=\"1920\" height=\"1080\">",
            "<rect width=\"1920\" height=\"1080\" fill=\"#ffffff\"/>",
            "<rect x=\"0\" y=\"0\" width=\"600\" height=\"400\" fill=\"#1e1e1e\"/>",
            "</svg>"
        );
        assert_eq!(count_text_nodes(svg), 0);

        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
        let density_pct = verify(&diag, 1, svg, &bitmap(svg));

        assert!(density_pct > 10.0, "色块密度应很高: {density_pct}");
        assert!(sink.events().is_empty(), "正常页不产生 verify 事件");
    }

    /// 槽位表与字段表一一对应（`FIELDS[i]` ↔ `SLOTS[i]`）：坐标、标签、居中规则都对得上。
    #[test]
    fn slots_match_field_canonical_order() {
        let entry = WordEntry {
            word: "apple".into(),
            phonetic: "/ˈæpl/".into(),
            morphology: "a-pple".into(),
            example: "I eat an apple every day.".into(),
            example_definition: "我每天吃一个苹果。".into(),
            definition: "苹果".into(),
        };
        let svg = slide_svg(&entry, &FontConfig::default());

        // 居中行：画布水平居中、不写标签前缀（单词 72pt → 120px，音标 28pt → 46.7px）。
        assert!(
            svg.contains(
                r##"x="960" y="240" text-anchor="middle" font-size="120" fill="#1e1e1e">apple</text>"##
            ),
            "单词槽位不符: {svg}"
        );
        assert!(
            svg.contains(
                r##"x="960" y="325" text-anchor="middle" font-size="46.7" fill="#646464">/ˈæpl/</text>"##
            ),
            "音标槽位不符: {svg}"
        );
        // 标签列：左起 240px，正文 24pt → 40px；labels 取自 FIELDS。
        for (label, baseline, value) in [
            ("词根词缀：", 548, "a-pple"),
            ("例句：", 644, "I eat an apple every day."),
            ("例句释义：", 692, "我每天吃一个苹果。"),
            ("单词释义：", 452, "苹果"),
        ] {
            assert!(
                svg.contains(&format!(r#"x="240" y="{baseline}" font-size="40""#)),
                "{label} 应坐在 x=240 y={baseline}: {svg}"
            );
            assert!(
                svg.contains(&format!("{label}{value}</text>")),
                "{label} 的标签前缀缺失: {svg}"
            );
        }

        // 出现次序 = FIELDS 规范顺序（单词 → 音标 → 词根词缀 → 例句 → 例句释义 → 单词释义）。
        let mut cursor = 0;
        for meta in &FIELDS {
            let value = entry.value(meta.field);
            let at = svg[cursor..]
                .find(value)
                .unwrap_or_else(|| panic!("{} 不在规范顺序上", meta.header));
            cursor += at + value.len();
        }
    }
}
