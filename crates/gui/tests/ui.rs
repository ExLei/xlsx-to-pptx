//! UI 集成测试：在真实窗口里驱动输入表单，直到 PPTX 落盘。

use std::time::Duration;

use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{
    px, size, AnyWindowHandle, AppContext, Bounds, Entity, TestAppContext, WindowBounds,
    WindowOptions,
};

use gui::app::VocabApp;

/// 与 reader 的 6 个必填列一致的合法输入。
const CSV: &str = "英文单词,英文音标,词根词缀,例句,例句释义,单词释义\napple,/ˈæp.əl/,ap-ple,An apple a day.,一天一个苹果。,苹果\nbanana,,ba-na-na,Bananas are yellow.,香蕉是黄色的。,香蕉\n";

/// 缺少必填列的表头，用于验证错误被推到界面上。
const BAD_CSV: &str = "英文单词,英文音标\napple,/ˈæp.əl/\n";

fn open_app(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<VocabApp>) {
    cx.update(gpui_kit::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1040.), px(760.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| VocabApp::new(window, cx)),
        )
        .expect("open test window")
    });
    (handle, view)
}

/// 在文件输入框里键入路径并回车。
fn type_input_path(cx: &mut TestAppContext, handle: AnyWindowHandle, path: &std::path::Path) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("input-file", cx);
        window.input(path.to_str().expect("utf-8 path"), cx);
        window.press("enter", cx);
    })
    .unwrap();
}

/// 未读取数据时两个动作按钮拒绝交互，也不会启动后台任务。
#[gpui_kit::test]
fn actions_stay_disabled_until_entries_are_loaded(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("temp dir");
    let (handle, view) = open_app(cx);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
        window.click("export-png", cx);
    })
    .unwrap();

    assert!(!view.read_with(cx, |view, _| view.is_busy()));
    assert_eq!(view.read_with(cx, |view, cx| view.entry_count(cx)), 0);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// 缺少必填列时读取失败：不进入可生成状态，并把失败推到界面上。
#[gpui_kit::test]
async fn reports_a_missing_column_instead_of_loading_partial_data(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("temp dir");
    let csv = dir.path().join("broken.csv");
    std::fs::write(&csv, BAD_CSV).expect("write csv");

    let (handle, view) = open_app(cx);
    type_input_path(cx, handle, &csv);

    cx.wait_for(handle, Duration::from_secs(5), |window, cx| {
        !view.read(cx).is_busy() && window.try_find("notification").is_some()
    })
    .await;

    assert_eq!(view.read_with(cx, |view, cx| view.entry_count(cx)), 0);
    assert!(!dir.path().join("broken.pptx").exists());
}

/// 在文件输入框键入路径并回车，读取完成后生成 PPTX。
#[gpui_kit::test]
async fn types_a_csv_path_and_generates_a_pptx(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("temp dir");
    let csv = dir.path().join("words.csv");
    std::fs::write(&csv, CSV).expect("write csv");
    let pptx = dir.path().join("words.pptx");

    let (handle, view) = open_app(cx);

    // 真实输入事件：点击输入框 → 键入路径 → 回车 → 订阅触发后台读取。
    type_input_path(cx, handle, &csv);

    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;

    // 输出路径由输入路径推导，且点击后必须真正落盘。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
    })
    .unwrap();

    // 任务收尾后才算完成：既是文件落盘，也是状态离开 generating。
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        !view.read(cx).is_busy() && pptx.exists()
    })
    .await;

    assert!(!view.read_with(cx, |view, _| view.is_busy()));
    let bytes = std::fs::read(&pptx).expect("read pptx");
    assert!(bytes.starts_with(b"PK"), "PPTX 应为 ZIP 容器");
    assert!(bytes.len() > 1000, "PPTX 不应为空文件");
}
