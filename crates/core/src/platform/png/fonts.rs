//! 字体链探测：latin / cjk / emoji 三链（`diag --font-trace` 的数据来源）。
//!
//! 候选表只有一份（[`LATIN`] / [`CJK`] / [`EMOJI`]），探测（[`probe`]）与渲染用的
//! 字体库装配（[`render_db`]）共用它；缺字回退顺序 = **latin → cjk → emoji**（设计 §6.4）。
//!
//! 旧版把探测到的 `fontdb` 配好族名后丢弃、渲染时重建第二份（死配置）；
//! 这里探测只产出名字，配置发生在真正使用的那个库上。

use std::sync::Arc;

use serde_json::json;
use usvg::fontdb;

use crate::diag::{field, Diag, Target};

/// 探测到的字体配置。
#[derive(Debug, Clone, Default)]
pub struct FontConfig {
    pub latin: Option<String>,
    pub cjk: Option<String>,
    pub emoji: Option<String>,
}

impl FontConfig {
    /// CSS 字体栈：latin → cjk → emoji → `sans-serif`（缺字回退顺序）。
    ///
    /// 未命中的链直接省略；三条链全缺时只剩 `sans-serif`。
    pub fn stack(&self) -> String {
        let mut families: Vec<&str> = Vec::with_capacity(4);
        families.extend(self.latin.as_deref());
        families.extend(self.cjk.as_deref());
        families.extend(self.emoji.as_deref());
        families.push("sans-serif");
        families.join(", ")
    }

    /// 首个命中的字体族（渲染库的通用族兜底）。
    fn primary(&self) -> Option<&str> {
        self.latin
            .as_deref()
            .or(self.cjk.as_deref())
            .or(self.emoji.as_deref())
    }
}

/// 链名（`fields.chain` 的取值，`diag --font-trace` 消费）。
const CHAIN_LATIN: &str = "latin";
const CHAIN_CJK: &str = "cjk";
const CHAIN_EMOJI: &str = "emoji";

/// latin 链候选（顺序即优先级；兼容面：改动即 `diag --font-trace` 语义变化）。
const LATIN: [&str; 4] = ["Segoe UI", "Helvetica", "Arial", "DejaVu Sans"];
/// cjk 链候选。
const CJK: [&str; 4] = [
    "Microsoft YaHei",
    "PingFang SC",
    "Noto Sans CJK SC",
    "WenQuanYi Micro Hei",
];
/// emoji 链候选。
const EMOJI: [&str; 3] = ["Segoe UI Emoji", "Apple Color Emoji", "Noto Color Emoji"];

/// 探测系统字体：逐链找第一个可用字体；整链缺失记 warn（含 `chain` / `missing` 字段）。
///
/// 无字体环境（CI 容器）不报错：三条链为 `None`，渲染照样出图（只是没有文字）。
pub fn probe(diag: &Diag) -> FontConfig {
    let db = load_system();
    let faces = db.faces().count();
    diag.info(
        Target::Fonts,
        format!("扫描到 {faces} 个字体面"),
        Some(json!({ field::COUNT: faces })),
    );

    let config = FontConfig {
        latin: find_first(&db, &LATIN, CHAIN_LATIN, diag),
        cjk: find_first(&db, &CJK, CHAIN_CJK, diag),
        emoji: find_first(&db, &EMOJI, CHAIN_EMOJI, diag),
    };
    let stack = config.stack();
    diag.info(
        Target::Fonts,
        format!("字体栈: {stack}"),
        Some(json!({ field::FONT: stack })),
    );
    config
}

/// 渲染用字体库：系统字体 + 由 [`FontConfig`] 指定的兜底族。
///
/// 通用族（`sans-serif` / `serif` / `monospace`）指向首个命中链，
/// 让字体栈末尾的 `sans-serif` 在缺字回退时落在同一个字体上。
pub(crate) fn render_db(config: &FontConfig) -> Arc<fontdb::Database> {
    let mut db = load_system();
    if let Some(primary) = config.primary() {
        db.set_sans_serif_family(primary);
        db.set_serif_family(primary);
        db.set_monospace_family(primary);
    }
    Arc::new(db)
}

/// 加载系统字体。
///
/// 旧版在这里包了 `catch_unwind`——release 是 `panic = "abort"`，该保护无效，已删除
/// （设计 §6.3「放弃的假保护」）；坏字体的风险面由探测事件 + 逐页 verify 覆盖。
fn load_system() -> fontdb::Database {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db
}

/// 逐链找第一个命中：每个候选都发一条事件，整链缺失额外发 warn。
fn find_first(
    db: &fontdb::Database,
    candidates: &[&str],
    chain: &'static str,
    diag: &Diag,
) -> Option<String> {
    for (order, name) in candidates.iter().enumerate() {
        let faces = db
            .faces()
            .filter(|face| face.families.iter().any(|(family, _)| family == name))
            .count();
        if faces > 0 {
            diag.info(
                Target::Fonts,
                format!("{chain}: 选中 \"{name}\"（{faces} 个字体面, order={order}）"),
                Some(json!({
                    field::CHAIN: chain,
                    field::FONT: name,
                    field::COUNT: faces,
                    "order": order,
                    "selected": true,
                })),
            );
            return Some((*name).to_string());
        }
        diag.info(
            Target::Fonts,
            format!("{chain}: \"{name}\" 缺失（order={order}）"),
            Some(json!({ field::CHAIN: chain, field::FONT: name, "order": order, "found": false })),
        );
    }
    diag.warn(
        Target::Fonts,
        format!(
            "{chain}: {} 个候选字体全部缺失（{}）",
            candidates.len(),
            candidates.join(" / ")
        ),
        Some(json!({ field::CHAIN: chain, field::MISSING: candidates })),
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空配置：只剩通用族，栈永不为空。
    #[test]
    fn empty_config_keeps_generic_family() {
        let config = FontConfig::default();
        assert_eq!(config.stack(), "sans-serif");
        assert!(config.primary().is_none());
    }

    /// 栈顺序 = 缺字回退顺序：latin → cjk → emoji。
    #[test]
    fn stack_follows_fallback_order() {
        let config = FontConfig {
            latin: Some("Helvetica".into()),
            cjk: Some("PingFang SC".into()),
            emoji: None,
        };
        assert_eq!(config.stack(), "Helvetica, PingFang SC, sans-serif");
        assert_eq!(config.primary(), Some("Helvetica"));
    }

    /// 探测结构完整：链名齐全；缺链必有带 `chain`/`missing` 的 warn。
    #[test]
    fn probe_reports_every_chain() {
        let sink = crate::diag::MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
        let config = probe(&diag);
        let stack = config.stack();
        assert!(stack.ends_with("sans-serif"), "栈必须含通用族: {stack}");

        let events = sink.events();
        assert!(
            events.iter().all(|event| event.target == "fonts"),
            "探测事件必须全部落在 fonts target"
        );
        for (chain, hit) in [
            (CHAIN_LATIN, &config.latin),
            (CHAIN_CJK, &config.cjk),
            (CHAIN_EMOJI, &config.emoji),
        ] {
            let chain_events: Vec<&crate::diag::DiagEvent> = events
                .iter()
                .filter(|event| {
                    event
                        .fields
                        .as_ref()
                        .and_then(|fields| fields.get(field::CHAIN))
                        .and_then(|value| value.as_str())
                        == Some(chain)
                })
                .collect();
            assert!(!chain_events.is_empty(), "{chain} 链没有任何事件");
            if hit.is_none() {
                let warn = chain_events
                    .iter()
                    .find(|event| event.level == crate::diag::Level::Warn)
                    .unwrap_or_else(|| panic!("{chain} 整链缺失必须记 warn"));
                let fields = warn.fields.as_ref().expect("warn 必须带 fields");
                assert!(
                    fields.get(field::MISSING).map(|v| v.is_array()).unwrap_or(false),
                    "{chain} 的 warn 必须带 missing 名单: {fields}"
                );
            }
        }
    }
}
