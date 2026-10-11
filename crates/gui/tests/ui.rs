//! GUI 冒烟测试：在无头窗口里构建真实视图，驱动输入 / 拖放 / 快捷键，断言状态与产物。
//!
//! 状态机的转移表在 `state_test.rs`（25 条，纯事件序列、无窗口）；这里只补最小闭环：
//! 窗口能建、空态拦截、路径/拖放进 Busy 并加载、生成到达 Done 并给出「打开产物」。
//!
//! 不依赖真实文件对话框（`rfd` 在无头环境没有窗口）；拖放走 OS 拖放事件而非鼠标。

use std::path::Path;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{
    point, px, size, AnyWindowHandle, AppContext, Bounds, Entity, ExternalPaths, FileDropEvent,
    InputEvent as _, TestAppContext, WindowBounds, WindowOptions,
};

use x2p_core::paths::Layout;
use x2p_gui::state::{ArtifactKind, Job, RendererField, Screen};
use x2p_gui::views::VocabApp;

/// 与六个必填列一致的合法输入。
const CSV: &str = "英文单词,英文音标,词根词缀,例句,例句释义,单词释义\napple,/ˈæp.əl/,ap-ple,An apple a day.,一天一个苹果。,苹果\nbanana,,ba-na-na,Bananas are yellow.,香蕉是黄色的。,香蕉\n";

/// 词条条数（上面的夹具写死两行）。
const ENTRIES: usize = 2;

fn open_app(cx: &mut TestAppContext, data_root: &Path) -> (AnyWindowHandle, Entity<VocabApp>) {
    let layout = Layout::with_root(data_root.to_path_buf());
    cx.update(gpui_kit::init);
    cx.update(|cx| {
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
    })
}

/// 写一份合法 CSV 到临时目录，返回路径。
fn write_csv() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("words.csv");
    std::fs::write(&path, CSV).expect("write csv");
    (dir, path)
}

/// 在输入框里键入路径并回车（真实输入事件，不经过文件对话框）。
fn type_input_path(cx: &mut TestAppContext, handle: AnyWindowHandle, path: &Path) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("input-file", cx);
        window.input(path.to_str().expect("utf-8 path"), cx);
        window.press("enter", cx);
    })
    .unwrap();
}

/// 词条条数（`wait_for` 的谓词拿到的是 `&mut App`，外面拿到的是 `&mut TestAppContext`，
/// 所以这两个小查询按 `AppContext` 泛化）。
fn entries<C: AppContext>(view: &Entity<VocabApp>, cx: &C) -> usize {
    view.read_with(cx, |view, _| view.state().document.entries.len())
}

fn is_busy<C: AppContext>(view: &Entity<VocabApp>, cx: &C) -> bool {
    view.read_with(cx, |view, _| view.is_busy())
}

/// 窗口可构建渲染；启动即建立会话日志（首行 session 事件、字段与 CLI 相同），
/// 状态栏的入口（日志、数据目录、诊断摘要、明暗开关）就位。
#[gpui_kit::test]
async fn window_builds_with_a_session_log_and_status_entries(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (handle, view) = open_app(cx, data.path());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("input-file").is_some(), "输入框应就位");
        assert!(window.try_find("generate").is_some(), "生成按钮应就位");
        assert!(window.try_find("export-png").is_some(), "导出按钮应就位");
        assert!(
            window.try_find("open-data-dir").is_some(),
            "状态栏应有数据目录入口"
        );
        assert!(window.try_find("open-log").is_some(), "状态栏应有日志入口");
        assert!(
            window.try_find("theme-toggle").is_some(),
            "状态栏应有明暗开关"
        );
        assert!(
            window.try_find("empty-pick-input").is_some(),
            "0 词条应给出空态入口"
        );
    })
    .unwrap();

    // 会话日志：启动一份，首行 session 事件（数据目录 / 回退标志等字段与 CLI 一致）。
    let logs = data.path().join("日志");
    let files: Vec<std::path::PathBuf> = std::fs::read_dir(&logs)
        .expect("日志目录应已建立")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    assert_eq!(files.len(), 1, "GUI 启动即写入一个会话日志");
    let content = std::fs::read_to_string(&files[0]).expect("读日志");
    let first = content.lines().next().expect("日志非空");
    assert!(
        first.contains(r#""target":"session""#),
        "首行应是 session 事件：{first}"
    );
    assert!(first.contains(r#""data_dir""#), "session 事件应含数据目录");
    assert!(
        view.read_with(cx, |view, _| view.log_path().is_some()),
        "视图应知道本会话日志路径"
    );
    assert!(
        !data.path().join("日志").join(".write-probe").exists(),
        "可写性探针文件应已清理"
    );

    // 启动时的渲染器探针必须落地（Probing → Available/Missing），否则模板模式永远拦住导出。
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        !matches!(view.read(cx).state().renderer, RendererField::Probing)
    })
    .await;
}

/// 空态下两个动作按钮拒绝交互：不产生作业，也不产生任何输出。
#[gpui_kit::test]
fn actions_stay_disabled_until_entries_are_loaded(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (handle, view) = open_app(cx, data.path());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // 禁用态断言走行为：点击不发作业（`disabled()` 在这个 Button 上不暴露事实）。
        window.click("generate", cx);
        window.click("export-png", cx);
    })
    .unwrap();

    assert!(!is_busy(&view, cx));
    assert_eq!(entries(&view, cx), 0);
    assert!(!data.path().join("PPT").exists(), "不应产生任何输出");
    assert!(!data.path().join("PNG").exists(), "不应产生任何输出");
}

/// 键入合法 CSV 路径并回车：回车当拍即进 Busy(Load)，随后词条进预览、两个动作放开。
#[gpui_kit::test]
async fn typing_a_csv_path_enters_busy_and_loads_entries(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (_src, csv) = write_csv();
    let (handle, view) = open_app(cx, data.path());

    type_input_path(cx, handle, &csv);

    // 加载是后台作业：回车之后立刻应处于 Busy(Load)。
    let screen = view.read_with(cx, |view, _| view.state().screen.clone());
    assert!(
        matches!(screen, Screen::Busy { job: Job::Load, .. }),
        "回车应立刻进入 Busy(Load)，实际：{screen:?}"
    );

    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        entries(&view, cx) == ENTRIES
    })
    .await;
    assert!(matches!(
        view.read_with(cx, |view, _| view.state().screen.clone()),
        Screen::Loaded
    ));
    // 默认输出路径跟随输入派生（同目录、同名 .pptx），由状态机在快照里决定。
    assert_eq!(
        view.read_with(cx, |view, _| view.state().document.source.clone()),
        Some(csv.clone())
    );

    // 有词条后空态让位给数据表（0 词条空态的入口消失）。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("empty-pick-input").is_none(),
            "有词条时不应再显示空态入口"
        );
    })
    .unwrap();

    // Ctrl/Cmd+Enter 与按钮同一守卫：加载完按一次能进 Busy(Generate)。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("ctrl-enter", cx);
    })
    .unwrap();
    assert!(
        is_busy(&view, cx),
        "Ctrl+Enter 应与「生成 PPT」等效（同一守卫）"
    );
}

/// 把 CSV 拖进窗口（OS 拖放事件）同样走加载流程；非表格文件被挡下并提示。
#[gpui_kit::test]
async fn dropping_a_file_loads_it_and_rejects_other_extensions(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (_src, csv) = write_csv();
    let (handle, view) = open_app(cx, data.path());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let position = point(px(120.), px(120.));
        window.dispatch_event(
            FileDropEvent::Entered {
                position,
                paths: ExternalPaths([csv.clone()].into_iter().collect()),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    })
    .unwrap();

    assert!(
        view.read_with(cx, |view, _| view.state().document.source.is_some()),
        "拖放当拍即应认下这个文件"
    );
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        entries(&view, cx) == ENTRIES
    })
    .await;

    // 非表格文件：不换输入源，只给可见提示（这里只断言状态没动）。
    let before = view.read_with(cx, |view, _| view.state().document.source.clone());
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let position = point(px(120.), px(120.));
        window.dispatch_event(
            FileDropEvent::Entered {
                position,
                paths: ExternalPaths([data.path().join("notes.txt")].into_iter().collect()),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    })
    .unwrap();
    assert_eq!(
        view.read_with(cx, |view, _| view.state().document.source.clone()),
        before,
        "不支持的扩展名不应换掉输入源"
    );
}

/// 生成走到底：产物落盘、进入 Done、状态栏给出「打开产物」。
#[gpui_kit::test]
async fn generating_reaches_done_with_the_artifact_action(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (src, csv) = write_csv();
    let pptx = src.path().join("words.pptx");
    let (handle, view) = open_app(cx, data.path());

    type_input_path(cx, handle, &csv);
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        entries(&view, cx) == ENTRIES
    })
    .await;

    // 有词条后生成按钮放开：点一次真的会发作业（同一条守卫的另一侧）。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("generate", cx);
    })
    .unwrap();

    cx.wait_for(handle, Duration::from_secs(20), |_, cx| {
        matches!(
            view.read(cx).state().screen,
            Screen::Done { .. }
        ) && pptx.exists()
    })
    .await;

    // 产物是 ZIP 容器（PPTX），并且页数 = 词条数（I5）。
    let bytes = std::fs::read(&pptx).expect("读产物");
    assert!(bytes.starts_with(b"PK"), "PPTX 应为 ZIP 容器");
    assert!(bytes.len() > 1000, "PPTX 不应为空文件");
    let slides = view.read_with(cx, |view, _| match &view.state().screen {
        Screen::Done { artifact } => artifact.slides,
        other => panic!("期望 Done，实际：{other:?}"),
    });
    assert_eq!(slides, ENTRIES, "输出页数 = 词条数");

    // Done 态：状态栏给出「打开产物」，并给一次重生成的机会（词条仍在）。
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("open-artifact").is_some(),
            "Done 态应给出打开产物的入口"
        );
    })
    .unwrap();
}

/// 无模板导出 PNG：走内置管线，逐页 `slide_N.png` 落在输出计划的目录里，进入 Done(Png)。
#[gpui_kit::test]
async fn exporting_png_writes_slide_files_and_reaches_done(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().expect("temp dir");
    let (src, csv) = write_csv();
    let (handle, view) = open_app(cx, data.path());

    type_input_path(cx, handle, &csv);
    cx.wait_for(handle, Duration::from_secs(10), |_, cx| {
        entries(&view, cx) == ENTRIES
    })
    .await;

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("export-png", cx);
    })
    .unwrap();
    assert!(
        matches!(
            view.read_with(cx, |view, _| view.state().screen.clone()),
            Screen::Busy {
                job: Job::ExportPng,
                ..
            }
        ),
        "导出应立刻进入 Busy(ExportPng)"
    );

    // 内置管线逐页渲染，2 页在调试构建下也要几秒。
    cx.wait_for(handle, Duration::from_secs(120), |_, cx| {
        !view.read(cx).is_busy()
    })
    .await;

    let (kind, slides, dir) = view.read_with(cx, |view, _| match &view.state().screen {
        Screen::Done { artifact } => (artifact.kind, artifact.slides, artifact.path.clone()),
        other => panic!("期望 Done，实际：{other:?}"),
    });
    assert_eq!(kind, ArtifactKind::Png, "产物种类应是 PNG");
    assert_eq!(slides, ENTRIES, "导出的张数 = 词条数");
    assert_eq!(dir, src.path().to_path_buf(), "落点是输出计划里的目录");

    // 命名与原子落盘：slide_1.png …（1 起），且是真 PNG。
    for number in 1..=ENTRIES {
        let png = src.path().join(format!("slide_{number}.png"));
        let bytes = std::fs::read(&png).unwrap_or_else(|error| {
            panic!("读 {} 失败: {error}", png.display());
        });
        assert!(
            bytes.starts_with(&[0x89, b'P', b'N', 0x47]),
            "{} 应是 PNG",
            png.display()
        );
        assert!(bytes.len() > 1000, "PNG 不应为空文件");
    }

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("open-artifact").is_some(),
            "Done(Png) 也应给出打开产物的入口"
        );
    })
    .unwrap();
}
