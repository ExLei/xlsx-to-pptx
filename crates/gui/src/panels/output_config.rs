//! 输出区：输出路径、PPTX 模板与前置校验提示。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{OutputState, RendererStatus, TemplateState, VocabApp};

pub fn render(app: &VocabApp, window: &mut Window, cx: &mut Context<VocabApp>) -> impl IntoElement {
    let busy = app.status.is_busy();
    let output_note = output_note(app, cx);
    let (template_text, template_color) = template_note(app, cx);
    let renderer_note = renderer_note(app);

    v_flex().px_4().pt_3().child(
        Form::horizontal()
            .label_width(window.rem_size() * 5.)
            .child(
                Field::new().label("输出文件").child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .w_full()
                                .child(
                                    Input::new(&app.output_file)
                                        .id("output-file")
                                        .cleanable(true)
                                        .w_full(),
                                )
                                .child(
                                    Button::new("pick-output")
                                        .icon(IconName::FolderOpen)
                                        .label("浏览…")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick_output(window, cx)
                                        })),
                                ),
                        )
                        .when_some(output_note, |this, (note, color)| {
                            this.child(div().text_xs().text_color(color).child(note))
                        }),
                ),
            )
            .child(
                Field::new().label("PPTX 模板").child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .w_full()
                                .child(
                                    Input::new(&app.template_file)
                                        .id("template-file")
                                        .cleanable(true)
                                        .w_full(),
                                )
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
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(template_color)
                                .child(template_text),
                        )
                        .when_some(renderer_note, |this, text| {
                            this.child(div().text_xs().text_color(cx.theme().warning).child(text))
                        }),
                ),
            ),
    )
}

/// 输出框提示：不可用原因，或「已存在，将自动另存为 …」。
fn output_note(app: &VocabApp, cx: &App) -> Option<(SharedString, Hsla)> {
    let theme = cx.theme();
    match app.output_state(cx) {
        OutputState::Invalid(reason) => Some((reason.into(), theme.danger)),
        OutputState::Taken(next) => Some((
            format!(
                "目标已存在，生成时将自动另存为 {}（不覆盖）",
                next.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
            .into(),
            theme.muted_foreground,
        )),
        OutputState::Free => None,
    }
}

/// 模板框主提示：内置版式 / 扫描中 / 不可用 / 将填入哪些占位符。
fn template_note(app: &VocabApp, cx: &App) -> (SharedString, Hsla) {
    let theme = cx.theme();
    match &app.template_state {
        TemplateState::Unset => (
            "未选择模板——使用内置版式；点「示例模板」可得到一份可编辑的模板".into(),
            theme.muted_foreground,
        ),
        TemplateState::Scanning => ("正在检查模板…".into(), theme.muted_foreground),
        TemplateState::Invalid(reason) => (reason.clone().into(), theme.danger),
        TemplateState::Ready(placeholders) => (
            format!(
                "模板将填入：{}（{} 个占位符）",
                placeholders.join("、"),
                placeholders.len()
            )
            .into(),
            theme.muted_foreground,
        ),
    }
}

/// 渲染器前置条件：选定模板且未检测到渲染器时提示——模板模式的 PNG 导出依赖它。
fn renderer_note(app: &VocabApp) -> Option<SharedString> {
    let template_selected = !matches!(
        app.template_state,
        TemplateState::Unset | TemplateState::Invalid(_)
    );
    (template_selected && matches!(app.renderer, RendererStatus::Missing))
        .then(|| "未检测到 Office / LibreOffice / WPS：PNG 导出不可用（生成 PPT 不受影响）".into())
}
