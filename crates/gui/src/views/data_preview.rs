//! 预览区：虚拟滚动词条表与空态。
//!
//! 列定义来自字段表（`FIELDS`：表头 + 规范顺序），单元格取值走 `WordEntry::value`——
//! 六列的表头与顺序只有一处来源，不在这里各写一份。

use gpui_kit::component::button::Button;
use gpui_kit::component::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use x2p_core::domain::schema::{Field, WordEntry};

use crate::state::{Job, Screen};
use crate::views::VocabApp;

/// 每列的初始宽度（rem 倍数，跟随主题基准字号缩放）；次序与 [`Field::ALL`] 一致。
/// 宽度是呈现细节，业务意义上唯一重要的是"六列来自字段表"。
const COLUMN_WIDTHS: [f32; 6] = [8., 10., 12., 22., 14., 14.];

/// 词汇表数据源：`document.entries` 的投影。
///
/// `TableDelegate` 拿不到 `AppState`，所以这里持有一份行数据，由 `sync_views` 在
/// 状态变化后刷新（条目只在加载结束时变化，拷贝代价可忽略）。
pub(crate) struct PreviewDelegate {
    entries: Vec<WordEntry>,
    columns: Vec<Column>,
    loading: bool,
}

impl PreviewDelegate {
    pub(crate) fn new(rem: Pixels) -> Self {
        Self {
            entries: Vec::new(),
            columns: Field::ALL
                .iter()
                .map(|field| {
                    Column::new(
                        format!("field-{}", field.index()),
                        // 表头即规范列名（`FIELDS` 是唯一来源）。
                        field.header(),
                    )
                    .width(rem * COLUMN_WIDTHS[field.index()])
                })
                .collect(),
            loading: false,
        }
    }

    pub(crate) fn entries(&self) -> &[WordEntry] {
        &self.entries
    }

    pub(crate) fn set_entries(&mut self, entries: Vec<WordEntry>) {
        self.entries = entries;
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    pub(crate) fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
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
        // 列 → 字段的映射就是 `Field::ALL` 的次序（与列定义同源）。
        let text = self
            .entries
            .get(row_ix)
            .zip(Field::ALL.get(col_ix))
            .map(|(entry, field)| entry.value(*field).to_string())
            .unwrap_or_default();

        div()
            .min_w_0()
            .w_full()
            .truncate()
            .when(col_ix == 0, |this| this.font_weight(FontWeight::MEDIUM))
            .child(text)
    }
}

/// 预览区：标题行 + 数据表；没有词条且不在读取时给出可操作的入口。
pub(crate) fn render(app: &VocabApp, cx: &mut Context<VocabApp>) -> impl IntoElement {
    let state = &app.state;
    let count = state.document.entries.len();
    let loading = matches!(state.screen, Screen::Busy { job: Job::Load, .. });
    let quiet = cx.theme().muted_foreground;

    v_flex()
        .flex_1()
        .min_h_0()
        .gap_2()
        .px_4()
        .pb_4()
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
                            .text_color(quiet)
                            .child(format!("共 {count} 条记录")),
                    )
                }),
        )
        .child(
            v_flex().flex_1().min_h_0().child(if count == 0 && !loading {
                empty_state(cx).into_any_element()
            } else {
                DataTable::new(&app.preview)
                    .stripe(true)
                    .bordered(true)
                    .into_any_element()
            }),
        )
}

/// 空态（0 词条）：说明下一步，并给一个可点的入口（也可直接把文件拖进来）。
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
