//! 输出区：输出路径、PPTX 模板、模板/渲染器提示与三个动作（生成 / 导出 PNG / 取消）。
//!
//! 提示文案全部由状态机的字段推导（`OutputField` / `TemplateField` / `RendererField`）：
//! 输出提示与模板提示不再分居两处，禁用逻辑与事件守卫共用 `state::can_generate` /
//! `state::can_export_png`（键盘路径绕不过去）。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::Input;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::state::{
    can_export_png, can_generate, AppState, Event, OutputField, RendererField, Screen,
    TemplateField,
};
use crate::views::VocabApp;

/// 生成快捷键的展示文本：secondary 在 macOS 是 ⌘，其他平台是 Ctrl。
const GENERATE_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "Cmd+Enter"
} else {
    "Ctrl+Enter"
};

pub(crate) fn render(
    app: &VocabApp,
    window: &mut Window,
    cx: &mut Context<VocabApp>,
) -> impl IntoElement {
    let state = &app.state;
    let busy = matches!(state.screen, Screen::Busy { .. });
    let can_generate = can_generate(state);
    let can_export = can_export_png(state);
    // 颜色先取（`Hsla` 是 Copy）：下面还要把 `cx` 借给监听器。
    let quiet = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let warning = cx.theme().warning;
    let output_note = output_note(state, quiet, danger);
    let (template_text, template_color) = template_note(state, quiet, danger);
    let renderer_note = renderer_note(state, quiet, warning);

    v_flex().px_4().pt_3().child(
        Form::horizontal()
            .label_width(window.rem_size() * 5.)
            .child(
                Field::new().label("输出文件").child(
                    v_flex()
                        .gap_1()
                        .w_full()
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
                                        .disabled(busy)
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
                        .w_full()
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
                                        .icon(IconName::FileText)
                                        .label("示例模板")
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
                        .when_some(renderer_note, |this, (note, color)| {
                            this.child(div().text_xs().text_color(color).child(note))
                        }),
                ),
            )
            .footer(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(busy, |this| {
                        this.child(
                            Button::new("cancel-job")
                                .ghost()
                                .label("取消")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.dispatch(Event::Cancel, window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("export-png")
                            .outline()
                            .icon(IconName::GalleryVerticalEnd)
                            .label("导出 PNG")
                            .tooltip(
                                "把词条导出为 PNG 图片；模板模式需要 Office / LibreOffice / WPS",
                            )
                            .disabled(!can_export)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch(Event::ExportPng, window, cx)
                            })),
                    )
                    .child(
                        Button::new("generate")
                            .primary()
                            .icon(IconName::Play)
                            .label("生成 PPT")
                            .tooltip(format!("生成 PPT（{GENERATE_SHORTCUT}）"))
                            .disabled(!can_generate)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch(Event::Generate, window, cx)
                            })),
                    ),
            ),
    )
}

/// 输出框提示：非法原因，或目标已存在时的覆盖说明（策略由状态机决定，这里只解释）。
fn output_note(state: &AppState, quiet: Hsla, danger: Hsla) -> Option<(String, Hsla)> {
    match &state.document.output {
        OutputField::Invalid { reason } => Some((reason.clone(), danger)),
        OutputField::Taken { path } if path.exists() => Some((
            "目标已存在：生成会另存为带序号的文件（不覆盖），导出 PNG 则直接覆盖".to_string(),
            quiet,
        )),
        _ => None,
    }
}

/// 模板框主提示：内置版式 / 检查中 / 不可用 / 将填入哪些占位符。
fn template_note(state: &AppState, quiet: Hsla, danger: Hsla) -> (String, Hsla) {
    match &state.document.template {
        TemplateField::Unset => (
            "未选择模板——使用内置版式；点「示例模板」可得到一份可编辑的模板".to_string(),
            quiet,
        ),
        TemplateField::Checking => ("正在检查模板…".to_string(), quiet),
        TemplateField::Invalid { reason } => (reason.clone(), danger),
        TemplateField::Ready { slides, fields } => (
            format!(
                "模板 {slides} 页 · 将填入：{}（{} 个占位符）",
                fields
                    .iter()
                    .map(|field| field.placeholder())
                    .collect::<Vec<_>>()
                    .join("、"),
                fields.len()
            ),
            quiet,
        ),
    }
}

/// 渲染器提示：只有模板模式下的 PNG 导出依赖它；探测中/可用/缺失三种状态都可见。
fn renderer_note(state: &AppState, quiet: Hsla, warning: Hsla) -> Option<(String, Hsla)> {
    state.document.template_path.as_ref()?;
    match &state.renderer {
        RendererField::Probing => Some(("正在探测渲染器…".to_string(), quiet)),
        RendererField::Available { name } => Some((
            format!("渲染器：{name}——模板模式的 PNG 导出由它完成"),
            quiet,
        )),
        RendererField::Missing => Some((
            "未检测到 Office / LibreOffice / WPS：模板模式下 PNG 导出不可用（生成 PPT 不受影响）"
                .to_string(),
            warning,
        )),
    }
}

impl VocabApp {
    /// 「浏览…」输出：选中即进状态机（受控控件由投影写回，不在这里直接改框）。
    pub(crate) fn pick_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = rfd::FileDialog::new()
            .add_filter("PPTX", &["pptx"])
            .save_file();
        if let Some(path) = picked {
            self.dispatch(Event::OutputChanged(path), window, cx);
        }
    }

    /// 「浏览…」模板：选中即进状态机（顺带触发探针校验）。
    pub(crate) fn pick_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = rfd::FileDialog::new()
            .add_filter("PPTX 模板", &["pptx"])
            .pick_file();
        if let Some(path) = picked {
            self.dispatch(Event::TemplateChanged(Some(path)), window, cx);
        }
    }
}
