//! 输入区：输入文件、格式、工作表与 CSV 编码。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::Input;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::select::Select;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{InputMode, VocabApp};

pub fn render(app: &VocabApp, window: &mut Window, cx: &mut Context<VocabApp>) -> impl IntoElement {
    let sheet_count = app.sheets.len();

    let secondary = match app.input_mode {
        // Excel：工作表选择器，未读到工作表时禁用
        InputMode::Excel => h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("工作表"),
            )
            .child(
                Select::new(&app.sheet_select)
                    .w_40()
                    .placeholder("（无工作表）")
                    .disabled(sheet_count == 0),
            )
            .into_any_element(),
        // CSV：文本编码
        InputMode::Csv => h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("编码"),
            )
            .child(
                Select::new(&app.encoding_select)
                    .id("encoding-select")
                    .w_32(),
            )
            .into_any_element(),
    };

    v_flex().px_4().pt_4().child(
        Form::horizontal()
            .label_width(window.rem_size() * 5.)
            .child(
                Field::new().label("输入文件").child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .w_full()
                        .child(
                            Input::new(&app.input_file)
                                .id("input-file")
                                .cleanable(true)
                                .w_full(),
                        )
                        .child(
                            Button::new("pick-input")
                                .icon(IconName::FolderOpen)
                                .label("浏览…")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.pick_input(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("excel-template")
                                .ghost()
                                .icon(IconName::FileText)
                                .label("Excel 模板")
                                .tooltip("生成 Excel 词汇表模板并打开")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_excel_template(window, cx)
                                })),
                        ),
                ),
            )
            .child(
                Field::new().label("输入格式").child(
                    h_flex()
                        .gap_6()
                        .items_center()
                        .w_full()
                        .child(
                            RadioGroup::new("input-mode")
                                .children(["Excel", "CSV"])
                                .selected_index(Some(app.input_mode.index()))
                                .on_change(cx.listener(|this, ix, window, cx| {
                                    this.set_input_mode(*ix, window, cx)
                                })),
                        )
                        .child(secondary)
                        .child(
                            Button::new("reload-input")
                                .ghost()
                                .icon(IconName::RefreshCw)
                                .tooltip("重新读取输入文件")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.reload(window, cx)),
                                ),
                        ),
                ),
            ),
    )
}
