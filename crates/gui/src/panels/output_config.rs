//! 输出区：输出路径、PPTX 模板与内置模板工具。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::VocabApp;

pub fn render(app: &VocabApp, window: &mut Window, cx: &mut Context<VocabApp>) -> impl IntoElement {
    let busy = app.status.is_busy();

    v_flex().px_4().pt_3().child(
        Form::horizontal()
            .label_width(window.rem_size() * 5.)
            .child(
                Field::new().label("输出文件").child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .w_full()
                        .child(Input::new(&app.output_file).cleanable(true).w_full())
                        .child(
                            Button::new("pick-output")
                                .icon(IconName::FolderOpen)
                                .label("浏览…")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.pick_output(window, cx)),
                                ),
                        ),
                ),
            )
            .child(
                Field::new().label("PPTX 模板").child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .w_full()
                        .child(Input::new(&app.template_file).cleanable(true).w_full())
                        .child(
                            Button::new("pick-template")
                                .icon(IconName::FolderOpen)
                                .label("浏览…")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_template(window, cx)
                                })),
                        )
                        .child(
                            Button::new("example-template")
                                .ghost()
                                .label("示例模板")
                                .icon(IconName::FileText)
                                .disabled(busy)
                                .tooltip("生成含占位符的示例 PPTX 模板并打开")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_example_pptx(window, cx)
                                })),
                        )
                        .child(
                            Button::new("excel-template")
                                .ghost()
                                .label("Excel 模板")
                                .icon(IconName::FileText)
                                .disabled(busy)
                                .tooltip("生成 Excel 词汇表模板并打开")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_excel_template(window, cx)
                                })),
                        ),
                ),
            ),
    )
}
