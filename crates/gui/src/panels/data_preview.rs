//! 预览区：虚拟滚动词汇表与空状态。

use gpui_kit::component::button::Button;
use gpui_kit::component::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use vocab_core::types::WordEntry;

use crate::app::VocabApp;

/// 列定义：键、表头、初始宽度（rem 倍数，跟随主题基准字号缩放）。
const COLUMNS: [(&str, &str, f32); 6] = [
    ("word", "单词", 8.),
    ("phonetic", "音标", 10.),
    ("morphology", "词根词缀", 12.),
    ("example", "例句", 22.),
    ("example_definition", "例句释义", 14.),
    ("definition", "单词释义", 14.),
];

/// 词汇表数据源；行渲染只读已读取的条目，滚动与选择由表格状态负责。
pub struct PreviewDelegate {
    entries: Vec<WordEntry>,
    columns: Vec<Column>,
    loading: bool,
}

impl PreviewDelegate {
    pub fn new(rem: Pixels) -> Self {
        Self {
            entries: Vec::new(),
            columns: COLUMNS
                .iter()
                .map(|(key, name, width)| Column::new(*key, *name).width(rem * *width))
                .collect(),
            loading: false,
        }
    }

    pub fn entries(&self) -> &[WordEntry] {
        &self.entries
    }

    pub fn set_entries(&mut self, entries: Vec<WordEntry>) {
        self.entries = entries;
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }
}

impl TableDelegate for PreviewDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.entries.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self
            .entries
            .get(row_ix)
            .map(|entry| match self.columns[col_ix].key.as_ref() {
                "word" => entry.word.clone(),
                "phonetic" => entry.phonetic.clone(),
                "morphology" => entry.morphology.clone(),
                "example" => entry.example.clone(),
                "example_definition" => entry.example_definition.clone(),
                "definition" => entry.definition.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();

        div()
            .min_w_0()
            .w_full()
            .truncate()
            .when(col_ix == 0, |this| this.font_weight(FontWeight::MEDIUM))
            .child(text)
    }
}

/// 预览区：标题行 + 数据表；无数据且未在读取时给出可操作的入口。
pub fn render(app: &VocabApp, cx: &mut Context<VocabApp>) -> impl IntoElement {
    let count = app.entry_count(cx);
    let loading = app.preview_loading(cx);

    v_flex()
        .flex_1()
        .min_h_0()
        .gap_2()
        .px_4()
        .pb_2()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("预览"),
                )
                .when(count > 0, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("共 {count} 条记录")),
                    )
                }),
        )
        .child(
            v_flex()
                .flex_1()
                .min_h_0()
                .child(if count == 0 && !loading {
                    empty_state(cx).into_any_element()
                } else {
                    DataTable::new(&app.preview)
                        .stripe(true)
                        .bordered(true)
                        .into_any_element()
                }),
        )
}

/// 空态：说明下一步，给出可点的入口，并提示可以直接把文件拖进来。
fn empty_state(cx: &mut Context<VocabApp>) -> impl IntoElement {
    Empty::new()
        .header(
            EmptyHeader::new()
                .media(
                    EmptyMedia::new()
                        .with_variant(EmptyMediaVariant::Icon)
                        .child(Icon::new(IconName::Inbox)),
                )
                .title(EmptyTitle::new().child("还没有词汇数据"))
                .description(
                    EmptyDescription::new().child("选择 Excel / CSV 文件，或直接把文件拖入窗口"),
                ),
        )
        .child(
            Button::new("empty-pick-input")
                .icon(IconName::FolderOpen)
                .label("选择文件…")
                .on_click(cx.listener(|this, _, window, cx| this.pick_input(window, cx))),
        )
}
