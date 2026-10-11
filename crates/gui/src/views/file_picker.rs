//! 输入区：输入文件、格式、工作表 / CSV 编码与重载。
//!
//! 只读 `AppState`（源、工作表清单、编码），动作一律翻译成 [`Event`]：
//! 回车或「浏览…」→ 打开文件，工作表/编码切换 → 重读，重载 → 对当前源再来一遍。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::Input;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::select::Select;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::state::{Event, NotifyLevel, Screen};
use crate::views::{is_excel, muted, VocabApp};

pub(crate) fn render(
    app: &VocabApp,
    window: &mut Window,
    cx: &mut Context<VocabApp>,
) -> impl IntoElement {
    let state = &app.state;
    let busy = matches!(state.screen, Screen::Busy { .. });
    let excel = is_excel(state);
    let source = state.document.source.clone();
    // 颜色先取（`Hsla` 是 Copy）：下面还要把 `cx` 借给监听器。
    let quiet = cx.theme().muted_foreground;

    // 工作表（Excel）或 CSV 编码：都来自状态机，切换即重读；忙碌期禁用（唯一可用动作是取消）。
    let secondary = if excel {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().text_color(quiet).child("工作表"))
            .child(
                Select::new(&app.sheet_select)
                    .id("sheet-select")
                    .w_40()
                    .placeholder("（无工作表）")
                    .disabled(busy || state.document.sheets.is_empty()),
            )
            .into_any_element()
    } else {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().text_color(quiet).child("编码"))
            .child(
                Select::new(&app.encoding_select)
                    .id("encoding-select")
                    .w_40()
                    .disabled(busy),
            )
            .into_any_element()
    };

    v_flex().px_4().pt_4().child(
        Form::horizontal()
            .label_width(window.rem_size() * 5.)
            .child(
                Field::new().label("输入文件").child(
                    v_flex()
                        .gap_1()
                        .w_full()
                        .child(
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
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick_input(window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("excel-template")
                                        .ghost()
                                        .icon(IconName::FileText)
                                        .label("Excel 模板")
                                        .disabled(busy)
                                        .tooltip("生成 Excel 词汇表模板并打开")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_excel_template(window, cx)
                                        })),
                                ),
                        )
                        .when(source.is_none(), |this| {
                            this.child(muted(
                                "选择 .xlsx / .xls / .csv 文件，或直接把文件拖入窗口",
                                quiet,
                            ))
                        }),
                ),
            )
            .child(
                Field::new().label("输入格式").child(
                    h_flex()
                        .gap_6()
                        .items_center()
                        .w_full()
                        // 格式由扩展名派生：这里如实显示，不作为可切换项（状态机没有格式事件）。
                        .child(
                            RadioGroup::horizontal("input-format")
                                .children(["Excel", "CSV"])
                                .selected_index(Some(usize::from(!excel)))
                                .disabled(true),
                        )
                        .child(muted("由扩展名决定", quiet))
                        .child(secondary)
                        .child(
                            Button::new("reload-input")
                                .ghost()
                                .icon(IconName::RefreshCw)
                                .tooltip("重新读取输入文件")
                                .disabled(busy || source.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reload(window, cx)
                                })),
                        ),
                ),
            ),
    )
}

impl VocabApp {
    /// 「浏览…」：系统文件对话框（阻塞式，与旧版一致）；选中即进状态机。
    pub(crate) fn pick_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = rfd::FileDialog::new()
            .add_filter("Excel/CSV", &["xlsx", "xls", "csv"])
            .pick_file();
        if let Some(path) = picked {
            self.dispatch(Event::PickInput(path), window, cx);
        }
    }

    /// 「重载」：对当前源重走一遍加载（没有源时按钮已禁用）。
    pub(crate) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.state.document.source.clone() else {
            // 按钮在没有源时禁用；真被触发就明说，不静默。
            self.notify(
                NotifyLevel::Warn,
                "尚未选择输入文件，无法重载".to_string(),
                window,
                cx,
            );
            return;
        };
        self.dispatch(Event::PickInput(source), window, cx);
    }
}
