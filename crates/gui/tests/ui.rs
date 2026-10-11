//! UI 集成测试：在真实窗口里驱动输入表单，直到 PPTX 落盘。

use std::path::Path;
use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{
    point, px, size, AnyWindowHandle, AppContext, Bounds, Entity, ExternalPaths, FileDropEvent,
    InputEvent as _, TestAppContext, WindowBounds, WindowOptions,
};

use gui::app::{OutputState, TemplateState, VocabApp};
use vocab_core::paths::Layout;

/// 与 reader 的 6 个必填列一致的合法输入。
const CSV: &str = "英文单词,英文音标,词根词缀,例句,例句释义,单词释义\napple,/ˈæp.əl/,ap-ple,An apple a day.,一天一个苹果。,苹果\nbanana,,ba-na-na,Bananas are yellow.,香蕉是黄色的。,香蕉\n";

/// 缺少必填列的表头，用于验证错误被推到界面上。
const BAD_CSV: &str = "英文单词,英文音标\napple,/ˈæp.əl/\n";

fn open_app(cx: &mut TestAppContext, data_root: &Path) -> (AnyWindowHandle, Entity<VocabApp>) {
    let layout = Layout::with_root(data_root.to_path_buf());
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
            |window, cx| cx.new(|cx| VocabApp::with_layout(window, cx, layout)),
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
    let data = tempfile::tempdir().expect("temp dir");
    let (handle, view) = open_app(cx, data.path());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
        window.click("export-png", cx);
        // 空态给出可点入口；Excel 模板按钮已归位输入区，仍在。
        assert!(
            window.try_find("empty-pick-input").is_some(),
            "空态应有「选择文件…」按钮"
        );
        assert!(
            window.try_find("excel-template").is_some(),
            "Excel 模板按钮存在"
        );
    })
    .unwrap();

    assert!(!view.read_with(cx, |view, _| view.is_busy()));
    assert_eq!(view.read_with(cx, |view, cx| view.entry_count(cx)), 0);
    assert!(!data.path().join("PPT").exists(), "不应产生任何输出");
}

/// 缺少必填列时读取失败：不进入可生成状态，并把失败推到界面上。
#[gpui_kit::test]
async fn reports_a_missing_column_instead_of_loading_partial_data(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("broken.csv");
    std::fs::write(&csv, BAD_CSV).expect("write csv");

    let (handle, view) = open_app(cx, data.path());
    type_input_path(cx, handle, &csv);

    cx.wait_for(handle, Duration::from_secs(5), |window, cx| {
        !view.read(cx).is_busy() && window.try_find("notification").is_some()
    })
    .await;

    assert_eq!(view.read_with(cx, |view, cx| view.entry_count(cx)), 0);
    assert!(!data.path().join("PPT").join("broken.pptx").exists());
}

/// 在文件输入框键入路径并回车，读取完成后生成 PPTX 到数据目录。
#[gpui_kit::test]
async fn types_a_csv_path_and_generates_a_pptx(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("words.csv");
    std::fs::write(&csv, CSV).expect("write csv");
    let pptx = data.path().join("PPT").join("words.pptx");

    let (handle, view) = open_app(cx, data.path());

    // 真实输入事件：点击输入框 → 键入路径 → 回车 → 订阅触发后台读取。
    type_input_path(cx, handle, &csv);

    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;

    // 输出路径推导到数据目录/PPT/，且点击后必须真正落盘。
    let derived = view.read_with(cx, |view, cx| view.output_path(cx));
    assert_eq!(
        derived,
        pptx.display().to_string(),
        "默认输出应落在数据目录"
    );

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

/// 启动即建立会话日志，状态栏提供打开数据目录的入口。
#[gpui_kit::test]
fn session_log_is_created_and_data_dir_action_is_exposed(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (handle, _view) = open_app(cx, data.path());

    let logs = data.path().join("日志");
    let log_count = std::fs::read_dir(&logs)
        .expect("日志目录应已建立")
        .filter(|entry| entry.as_ref().map(|e| e.path().is_file()).unwrap_or(false))
        .count();
    assert_eq!(log_count, 1, "GUI 启动即写入一个会话日志");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("open-data-dir").is_some(),
            "状态栏应有「打开数据目录」按钮"
        );
    })
    .unwrap();
}

/// 明暗开关：点击换到另一模式，再点回到原模式。
#[gpui_kit::test]
fn theme_toggle_switches_between_light_and_dark(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (handle, _view) = open_app(cx, data.path());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let before = cx.theme().mode;
        window.click("theme-toggle", cx);
        assert_ne!(cx.theme().mode, before, "点击开关后主题模式应改变");
        window.render_frame(cx);
        window.click("theme-toggle", cx);
        assert_eq!(cx.theme().mode, before, "再点一次应切回原模式");
    })
    .unwrap();
}

/// 把文件拖进窗口即可加载（走 OS 文件拖放事件）。
#[gpui_kit::test]
async fn dropping_a_csv_file_loads_it(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("words.csv");
    std::fs::write(&csv, CSV).expect("write csv");

    let (handle, view) = open_app(cx, data.path());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let position = point(px(120.), px(120.));
        let paths = ExternalPaths([csv.clone()].into_iter().collect());
        window.dispatch_event(
            FileDropEvent::Entered { position, paths }.to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    })
    .unwrap();

    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;
}

/// 切换 CSV 编码立即重读：GBK 文件默认按 UTF-8 失败，改选 GBK 后出现条目。
#[gpui_kit::test]
async fn switching_csv_encoding_rereads_the_file(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("gbk.csv");
    let (gbk, _, _) = encoding_rs::GBK.encode(CSV);
    std::fs::write(&csv, &gbk[..]).expect("write gbk csv");

    let (handle, view) = open_app(cx, data.path());
    type_input_path(cx, handle, &csv);

    // 默认 UTF-8：GBK 字节解码失败，没有任何条目。
    cx.wait_for(handle, Duration::from_secs(5), |window, cx| {
        !view.read(cx).is_busy() && window.try_find("notification").is_some()
    })
    .await;
    assert_eq!(view.read_with(cx, |view, cx| view.entry_count(cx)), 0);

    // 打开编码选择器：默认选中第 1 项（UTF-8），键盘移到第 2 项（GBK）并确认。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("encoding-select").click("input", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.press("down", cx);
        window.press("enter", cx);
    })
    .unwrap();

    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;
}

/// 生成完成后状态栏给出「打开 PPT」；再次生成自动另存为，不覆盖。
#[gpui_kit::test]
async fn generating_twice_renames_instead_of_overwriting(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("words.csv");
    std::fs::write(&csv, CSV).expect("write csv");
    let pptx = data.path().join("PPT").join("words.pptx");
    let renamed = data.path().join("PPT").join("words (1).pptx");

    let (handle, view) = open_app(cx, data.path());
    type_input_path(cx, handle, &csv);
    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
    })
    .unwrap();
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        !view.read(cx).is_busy() && pptx.exists()
    })
    .await;

    // 完成态：记下产物，并在状态栏给出打开入口。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("open-artifact").is_some(),
            "生成后应有「打开 PPT」入口"
        );
    })
    .unwrap();
    assert!(view.read_with(cx, |view, _| view.last_artifact() == Some(pptx.as_path())));

    // 再点一次生成：目标已存在 → 另存为 words (1).pptx。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
    })
    .unwrap();
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        !view.read(cx).is_busy() && renamed.exists()
    })
    .await;
    assert!(pptx.exists(), "第一个产物保持原样，不被覆盖");
}

/// 模板缺 {{单词}}：选定即校验，拦住生成并给出理由。
#[gpui_kit::test]
async fn an_invalid_template_blocks_generation(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("words.csv");
    std::fs::write(&csv, CSV).expect("write csv");
    // 内置模板只有 __WORD__ 标记，没有 {{单词}} 占位符。
    let bad_template = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/assets/template.pptx");

    let (handle, view) = open_app(cx, data.path());
    type_input_path(cx, handle, &csv);
    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("template-file", cx);
        window.input(bad_template.to_str().expect("utf-8 path"), cx);
    })
    .unwrap();

    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        matches!(view.read(cx).template_state(), TemplateState::Invalid(_))
    })
    .await;
    assert!(!view.read_with(cx, |view, cx| view.can_generate(cx)));

    // 生成按钮已禁用：点击不产生任何后台任务或产物。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
    })
    .unwrap();
    assert!(!view.read_with(cx, |view, _| view.is_busy()));
    assert!(!data.path().join("PPT").join("words.pptx").exists());

    // 清空模板框：回到内置版式，生成重新可用。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("template-file").click("clean", cx);
    })
    .unwrap();
    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        matches!(view.read(cx).template_state(), TemplateState::Unset)
    })
    .await;
    assert!(view.read_with(cx, |view, cx| view.can_generate(cx)));

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
    })
    .unwrap();
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        !view.read(cx).is_busy() && data.path().join("PPT").join("words.pptx").exists()
    })
    .await;
}

/// 有效模板：扫描出会被填入的占位符清单。
#[gpui_kit::test]
async fn a_valid_template_reports_its_placeholders(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let template = data.path().join("自定义模板.pptx");
    vocab_core::template_pptx::generate_example_pptx(&template).expect("生成示例模板");

    let (handle, view) = open_app(cx, data.path());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("template-file", cx);
        window.input(template.to_str().expect("utf-8 path"), cx);
    })
    .unwrap();

    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        matches!(view.read(cx).template_state(), TemplateState::Ready { .. })
    })
    .await;
    let state = view.read_with(cx, |view, _| view.template_state().clone());
    match state {
        TemplateState::Ready(placeholders) => {
            assert!(
                placeholders.iter().any(|name| name == "单词"),
                "缺少必填占位符：{placeholders:?}"
            );
        }
        other => panic!("期望模板 Ready，得到 {other:?}"),
    }
}

/// 清空输出路径：前置校验拦住生成，提示替代了点击后的失败通知。
#[gpui_kit::test]
async fn clearing_the_output_path_blocks_generation(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("words.csv");
    std::fs::write(&csv, CSV).expect("write csv");

    let (handle, view) = open_app(cx, data.path());
    type_input_path(cx, handle, &csv);
    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.within("output-file").click("clean", cx);
    })
    .unwrap();

    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        matches!(
            view.read_with(cx, |view, cx| view.output_state(cx)),
            OutputState::Invalid(_)
        )
    })
    .await;
    assert!(!view.read_with(cx, |view, cx| view.can_generate(cx)));

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
    })
    .unwrap();
    assert!(!view.read_with(cx, |view, _| view.is_busy()));
    assert!(!data.path().join("PPT").join("words.pptx").exists());
}

/// 输入焦点下 Ctrl+Enter 等同于「生成 PPT」。
#[gpui_kit::test]
async fn ctrl_enter_generates_after_loading(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let csv_dir = tempfile::tempdir().expect("temp dir");
    let csv = csv_dir.path().join("words.csv");
    std::fs::write(&csv, CSV).expect("write csv");
    let pptx = data.path().join("PPT").join("words.pptx");

    let (handle, view) = open_app(cx, data.path());
    type_input_path(cx, handle, &csv);
    cx.wait_for(handle, Duration::from_secs(5), |_, cx| {
        view.read(cx).entry_count(cx) == 2
    })
    .await;

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();

    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        !view.read(cx).is_busy() && pptx.exists()
    })
    .await;
}
