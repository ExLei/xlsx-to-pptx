//! PNG 管线端到端：内置 SVG 渲染与 verify、字体链探测、外呼渲染器的选择/收集/取消。
//!
//! 结构与几何断言取自 `docs/design.md` §6.3/§6.4 与旧版 `png_export_test.rs` 的回归面。

use std::cell::RefCell;
use std::ops::ControlFlow;

use serde_json::{json, Value};
use x2p_core::diag::{field, Diag, DiagEvent, Level, MemorySink, Target};
use x2p_core::domain::schema::WordEntry;
use x2p_core::platform::png::external::Backend;
use x2p_core::platform::png::{builtin, external, fonts, PngError, RenderError, RendererChoice};
use x2p_core::ports::{NoopObserver, OpObserver, Progress};

const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// 六字段齐全的词条。
fn entry(word: &str) -> WordEntry {
    WordEntry {
        word: word.into(),
        phonetic: "/ˈæpl/".into(),
        morphology: "a-pple".into(),
        example: "I eat an apple every day.".into(),
        example_definition: "我每天吃一个苹果。".into(),
        definition: "苹果".into(),
    }
}

/// 记录进度、从不取消。
#[derive(Default)]
struct Recorder {
    seen: Vec<Progress>,
}

impl OpObserver for Recorder {
    fn on(&mut self, progress: &Progress) -> ControlFlow<()> {
        self.seen.push(progress.clone());
        ControlFlow::Continue(())
    }
}

/// 第 `cancel_at` 次进度回调（1 起）请求取消。
struct CancelAt {
    cancel_at: usize,
    calls: usize,
}

impl OpObserver for CancelAt {
    fn on(&mut self, _progress: &Progress) -> ControlFlow<()> {
        self.calls += 1;
        if self.calls >= self.cancel_at {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// 某 target 上的事件。
fn events_on<'a>(events: &'a [DiagEvent], target: Target) -> Vec<&'a DiagEvent> {
    events
        .iter()
        .filter(|event| event.target == target.as_str())
        .collect()
}

/// 事件的 `fields[key]`。
fn field_of<'a>(event: &'a DiagEvent, key: &str) -> Option<&'a Value> {
    event.fields.as_ref()?.get(key)
}

// ── 内置管线：出图 ──

#[test]
fn builtin_render_writes_one_slide_per_entry() {
    let tmp = tempfile::tempdir().expect("临时目录");
    let out = tmp.path().join("png");
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
    let mut observer = Recorder::default();

    let slides = builtin::render(&[entry("apple"), entry("banana")], &out, &mut observer, &diag)
        .expect("内置管线必须出图");

    assert_eq!(
        slides,
        vec![out.join("slide_1.png"), out.join("slide_2.png")]
    );
    for path in &slides {
        let bytes = std::fs::read(path).expect("slide 必须落盘");
        assert!(!bytes.is_empty(), "{} 不能是空文件", path.display());
        assert_eq!(&bytes[..8], &PNG_MAGIC, "{} 必须是 PNG", path.display());
        let image = image::load_from_memory(&bytes).expect("PNG 必须可解码");
        assert_eq!((image.width(), image.height()), (1920, 1080));
    }

    assert_eq!(
        observer.seen,
        vec![
            Progress::Slides { done: 0, total: 2 },
            Progress::Slides { done: 1, total: 2 },
        ]
    );

    // 逐页 info 事件带 slide / path / density_pct（`diag --slide N` 的数据来源）。
    let all = sink.events();
    let page = all
        .iter()
        .find(|event| {
            event.target == Target::Render.as_str()
                && field_of(event, field::SLIDE) == Some(&json!(1))
        })
        .expect("应有 slide=1 的逐页事件");
    assert_eq!(
        field_of(page, field::PATH),
        Some(&json!(out.join("slide_1.png").display().to_string()))
    );
    assert!(
        field_of(page, field::DENSITY_PCT)
            .and_then(Value::as_f64)
            .is_some(),
        "逐页事件必须带 density_pct: {page:?}"
    );

    // 本机有可用字体时，文字必须真的画出来（无字体环境只剩白底，渲染照样成功）。
    let probed = fonts::probe(&diag);
    if probed.latin.is_some() || probed.cjk.is_some() || probed.emoji.is_some() {
        let verify_events = events_on(&all, Target::Verify);
        assert!(
            verify_events.is_empty(),
            "有字体时不该出现 verify 异常: {verify_events:?}"
        );
    }
}

#[test]
fn builtin_render_escapes_xml_and_survives_odd_text() {
    let tmp = tempfile::tempdir().expect("临时目录");
    let diag = Diag::new();
    let tricky = WordEntry {
        word: "A<B & \"C\" > D".into(),
        morphology: "it's".into(),
        ..WordEntry::default()
    };

    let slides =
        builtin::render(&[tricky], tmp.path(), &mut NoopObserver, &diag).expect("转义后必须能渲染");

    assert_eq!(slides.len(), 1);
    let bytes = std::fs::read(&slides[0]).expect("slide 必须落盘");
    assert!(!bytes.is_empty());
    let image = image::load_from_memory(&bytes).expect("PNG 必须可解码");
    assert_eq!(image.width(), 1920);
}

#[test]
fn builtin_cancel_keeps_completed_slides() {
    let tmp = tempfile::tempdir().expect("临时目录");
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
    let mut observer = CancelAt {
        cancel_at: 2,
        calls: 0,
    };

    let error =
        builtin::render(&[entry("apple"), entry("banana")], tmp.path(), &mut observer, &diag)
            .expect_err("取消不是成功");

    assert!(
        matches!(error, PngError::Cancelled),
        "取消形态不符: {error:?}"
    );
    assert!(tmp.path().join("slide_1.png").exists(), "已完成的页必须保留");
    assert!(
        !tmp.path().join("slide_2.png").exists(),
        "未渲染的页不得留文件"
    );

    let all = sink.events();
    let warn = all
        .iter()
        .find(|event| {
            event.level == Level::Warn
                && event.target == Target::Render.as_str()
                && field_of(event, field::SLIDES) == Some(&json!(1))
        })
        .expect("取消必须有一条带已完成页数的 warn");
    assert_eq!(field_of(warn, field::TOTAL), Some(&json!(2)));
}

// ── SVG 版式：字段顺序与标签 ──

#[test]
fn slide_svg_carries_every_field_with_its_label_in_canonical_order() {
    let config = fonts::probe(&Diag::new());
    let svg = builtin::slide_svg(&entry("apple"), &config);

    let markers = [
        "apple</text>",
        "/ˈæpl/</text>",
        "词根词缀：a-pple</text>",
        "例句：I eat an apple every day.</text>",
        "例句释义：我每天吃一个苹果。</text>",
        "单词释义：苹果</text>",
    ];
    for marker in markers {
        assert!(svg.contains(marker), "{marker} 缺失: {svg}");
    }

    // 出现次序 = FIELDS 规范顺序（单词 → 音标 → 词根词缀 → 例句 → 例句释义 → 单词释义）。
    let mut cursor = 0;
    for marker in markers {
        let at = svg[cursor..]
            .find(marker)
            .unwrap_or_else(|| panic!("{marker} 不在规范顺序上: {svg}"));
        cursor += at + marker.len();
    }

    // 几何：与示例模板同尺度（1920×1080；单词 120px 居中，标签列左起 240px）。
    assert!(svg.contains(r#"viewBox="0 0 1920 1080""#));
    assert!(svg.contains(r#"x="960" y="240" text-anchor="middle""#));
    assert!(svg.contains(r#"font-size="120""#));
    assert!(svg.contains(r#"font-size="46.7""#));
    for baseline in [452, 548, 644, 692] {
        assert!(
            svg.contains(&format!(r#"x="240" y="{baseline}""#)),
            "标签行缺 y={baseline}: {svg}"
        );
    }
    // 字体族来自字体链探测（无字体环境只剩通用族）。
    assert!(svg.contains("font-family=\""), "字体族缺失: {svg}");
    assert!(svg.contains("sans-serif"), "通用族兜底缺失: {svg}");
}

#[test]
fn slide_svg_skips_empty_fields() {
    let config = fonts::probe(&Diag::new());
    let sparse = WordEntry {
        word: "test".into(),
        morphology: "test-morph".into(),
        ..WordEntry::default()
    };

    let svg = builtin::slide_svg(&sparse, &config);

    assert!(svg.contains("test-morph"));
    assert_eq!(svg.matches("<text ").count(), 2, "只有两个非空字段出行: {svg}");
    assert!(!svg.contains("例句："), "空字段不得留标签: {svg}");
}

// ── verify：字段形状（`diag --blank-slides` 契约） ──

#[test]
fn verify_reports_low_density_with_queryable_fields() {
    let tmp = tempfile::tempdir().expect("临时目录");
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);

    let slides = builtin::render(
        &[WordEntry::default(), entry("apple")],
        tmp.path(),
        &mut NoopObserver,
        &diag,
    )
    .expect("空词条也要出图（页数守恒）");

    assert_eq!(slides.len(), 2);
    assert!(std::fs::metadata(&slides[0])
        .map(|meta| meta.len() > 0)
        .unwrap_or(false));

    let all = sink.events();
    let verify_events = events_on(&all, Target::Verify);
    let low = verify_events
        .iter()
        .find(|event| field_of(event, field::REASON) == Some(&json!(field::REASON_LOW_DENSITY)))
        .expect("全空的一页必须报 low_density");
    assert_eq!(low.level, Level::Warn);
    assert_eq!(field_of(low, field::SLIDE), Some(&json!(1)));
    assert!(
        field_of(low, field::DENSITY_PCT)
            .and_then(Value::as_f64)
            .unwrap_or(1.0)
            < 0.01,
        "密度必须接近 0: {low:?}"
    );
    assert!(
        verify_events
            .iter()
            .all(|event| field_of(event, field::SLIDE).is_some()),
        "verify 事件必须都带 slide"
    );
}

// ── 字体链探测 ──

#[test]
fn fonts_probe_is_structurally_complete() {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);

    let config = fonts::probe(&diag);
    assert!(config.stack().ends_with("sans-serif"), "栈必须含通用族");

    let all = sink.events();
    assert!(!all.is_empty(), "探测必须发事件");
    assert_eq!(
        events_on(&all, Target::Fonts).len(),
        all.len(),
        "探测事件必须全在 fonts target"
    );

    for (chain, hit) in [
        ("latin", &config.latin),
        ("cjk", &config.cjk),
        ("emoji", &config.emoji),
    ] {
        let chain_events: Vec<&DiagEvent> = all
            .iter()
            .filter(|event| field_of(event, field::CHAIN) == Some(&json!(chain)))
            .collect();
        assert!(!chain_events.is_empty(), "{chain} 链没有任何事件");
        match hit {
            Some(font) => assert!(
                chain_events.iter().any(|event| {
                    event.level == Level::Info && field_of(event, field::FONT) == Some(&json!(font))
                }),
                "{chain} 命中 {font} 却没有对应 info 事件"
            ),
            None => {
                let warn = chain_events
                    .iter()
                    .find(|event| event.level == Level::Warn)
                    .unwrap_or_else(|| panic!("{chain} 整链缺失必须记 warn"));
                assert!(
                    field_of(warn, field::MISSING)
                        .map(Value::is_array)
                        .unwrap_or(false),
                    "{chain} 的 warn 必须带缺失名单: {warn:?}"
                );
            }
        }
    }
}

// ── 外呼渲染器：选择顺序（注入假探测器，不需要真装 Office） ──

/// 假探测器：记录探测顺序，按名单返回可用的可执行文件。
struct FakeProbe {
    available: Vec<(Backend, &'static str)>,
    seen: RefCell<Vec<Backend>>,
}

impl FakeProbe {
    fn new(available: &[(Backend, &'static str)]) -> FakeProbe {
        FakeProbe {
            available: available.to_vec(),
            seen: RefCell::new(Vec::new()),
        }
    }

    fn run(&self, backend: Backend, _diag: &Diag) -> Option<String> {
        self.seen.borrow_mut().push(backend);
        self.available
            .iter()
            .find(|(candidate, _)| *candidate == backend)
            .map(|(_, exe)| (*exe).to_string())
    }

    fn seen(&self) -> Vec<Backend> {
        self.seen.borrow().clone()
    }
}

#[test]
fn select_auto_prefers_powerpoint() {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
    let probe = FakeProbe::new(&[
        (Backend::PowerPoint, "powershell"),
        (Backend::LibreOffice, "soffice"),
        (Backend::Wps, "wps"),
    ]);

    let renderer = external::select_with(
        RendererChoice::Auto,
        &|backend, diag| probe.run(backend, diag),
        &diag,
    )
    .expect("PowerPoint 可用时必须选中它");

    assert_eq!(renderer.name(), "Microsoft PowerPoint");
    assert_eq!(probe.seen(), vec![Backend::PowerPoint], "命中即停");

    let all = sink.events();
    let detected = all
        .iter()
        .find(|event| {
            event.target == Target::Renderer.as_str()
                && field_of(event, field::EXE) == Some(&json!("powershell"))
        })
        .expect("选择必须发带 exe 的 renderer 事件");
    assert_eq!(
        field_of(detected, field::RENDERER),
        Some(&json!("Microsoft PowerPoint"))
    );
}

#[test]
fn select_auto_falls_back_to_libreoffice() {
    let diag = Diag::new();
    let probe = FakeProbe::new(&[(Backend::LibreOffice, "soffice")]);

    let renderer = external::select_with(
        RendererChoice::Auto,
        &|backend, diag| probe.run(backend, diag),
        &diag,
    )
    .expect("缺 PowerPoint 时退到 LibreOffice");

    assert_eq!(renderer.name(), "LibreOffice");
    assert_eq!(
        probe.seen(),
        vec![Backend::PowerPoint, Backend::LibreOffice],
        "必须按 PowerPoint → LibreOffice 顺序探测，命中即停"
    );
}

#[test]
fn select_auto_falls_back_to_wps() {
    let diag = Diag::new();
    let probe = FakeProbe::new(&[(Backend::Wps, "wps")]);

    let renderer = external::select_with(
        RendererChoice::Auto,
        &|backend, diag| probe.run(backend, diag),
        &diag,
    )
    .expect("缺 PowerPoint/LibreOffice 时退到 WPS");

    assert_eq!(renderer.name(), "WPS Office");
    assert_eq!(
        probe.seen(),
        vec![Backend::PowerPoint, Backend::LibreOffice, Backend::Wps]
    );
}

#[test]
fn select_auto_without_renderer_is_no_renderer() {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
    let probe = FakeProbe::new(&[]);

    let error = match external::select_with(
        RendererChoice::Auto,
        &|backend, diag| probe.run(backend, diag),
        &diag,
    ) {
        Ok(renderer) => panic!("一个后端都没有，却选中了 {}", renderer.name()),
        Err(error) => error,
    };

    assert!(
        matches!(error, RenderError::NoRenderer),
        "错误形态不符: {error:?}"
    );
    let all = sink.events();
    let warn = all
        .iter()
        .find(|event| event.level == Level::Warn && event.target == Target::Renderer.as_str())
        .expect("全无可用渲染器必须记 warn");
    assert!(
        field_of(warn, field::MISSING)
            .map(Value::is_array)
            .unwrap_or(false),
        "warn 必须带缺失名单: {warn:?}"
    );
}

#[test]
fn select_explicit_missing_renderer_reports_its_name() {
    let sink = MemorySink::new();
    let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
    let probe = FakeProbe::new(&[(Backend::PowerPoint, "powershell")]);

    let error = match external::select_with(
        RendererChoice::Wps,
        &|backend, diag| probe.run(backend, diag),
        &diag,
    ) {
        Ok(renderer) => panic!("WPS 缺失却选中了 {}", renderer.name()),
        Err(error) => error,
    };

    match error {
        RenderError::Missing { name } => assert_eq!(name, "WPS"),
        other => panic!("错误形态不符: {other:?}"),
    }
    assert_eq!(probe.seen(), vec![Backend::Wps], "指定后端只探它自己");

    let all = sink.events();
    let error_event = all
        .iter()
        .find(|event| event.level == Level::Error && event.target == Target::Renderer.as_str())
        .expect("指定后端缺失必须记 error");
    assert_eq!(field_of(error_event, field::RENDERER), Some(&json!("WPS")));
}

// ── 外呼渲染器：真子进程路径（假渲染器脚本） ──

#[cfg(unix)]
mod subprocess {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use x2p_core::platform::png::PngRenderer;

    /// 假渲染器脚本（可执行）：POSIX sh，忽略传入的 argv，只做脚本里写的事。
    fn fake_renderer(dir: &Path, name: &str, body: &str) -> PathBuf {
        let script = dir.join(name);
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("写假渲染器失败");
        let mut permissions = std::fs::metadata(&script)
            .expect("脚本元数据")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("加可执行位失败");
        script
    }

    /// 等到子进程真的跑起来（pid 文件落地）再请求取消。
    struct CancelWhenRunning {
        pid_file: PathBuf,
    }

    impl OpObserver for CancelWhenRunning {
        fn on(&mut self, _progress: &Progress) -> ControlFlow<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.pid_file.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            ControlFlow::Break(())
        }
    }

    #[test]
    fn external_render_collects_slides_sorted_by_number() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let out = tmp.path().join("out");
        let script = fake_renderer(
            tmp.path(),
            "fake-renderer.sh",
            &format!(
                "touch '{out}/slide_10.png' '{out}/slide_2.png' '{out}/SLIDE_1.PNG' '{out}/cover.png'",
                out = out.display()
            ),
        );
        let diag = Diag::new();
        let renderer = external::ExternalCli::new(Backend::LibreOffice, &script);

        let slides = renderer
            .render(&tmp.path().join("deck.pptx"), &out, &mut NoopObserver, &diag)
            .expect("应收集到 3 页");

        let names: Vec<String> = slides
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["SLIDE_1.PNG", "slide_2.png", "slide_10.png"]);
        assert_eq!(renderer.name(), "LibreOffice");
    }

    #[test]
    fn external_render_without_output_fails() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let out = tmp.path().join("out");
        let script = fake_renderer(tmp.path(), "silent.sh", "exit 0");
        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
        let renderer = external::ExternalCli::new(Backend::LibreOffice, &script);

        let error = renderer
            .render(&tmp.path().join("deck.pptx"), &out, &mut NoopObserver, &diag)
            .expect_err("没有输出必须失败");

        match error {
            RenderError::Failed { renderer, detail } => {
                assert_eq!(renderer, "LibreOffice");
                assert!(detail.contains("slide_N.png"), "详情应说明缺什么: {detail}");
            }
            other => panic!("错误形态不符: {other:?}"),
        }
        let all = sink.events();
        let warn = all
            .iter()
            .find(|event| event.level == Level::Warn && event.target == Target::Renderer.as_str())
            .expect("无输出必须记 warn");
        assert_eq!(field_of(warn, field::RENDERER), Some(&json!("LibreOffice")));
    }

    #[test]
    fn external_render_failure_reports_stderr_and_cleans_partial_output() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let out = tmp.path().join("out");
        let script = fake_renderer(
            tmp.path(),
            "broken.sh",
            &format!(
                "touch '{out}/slide_1.png'\necho 'boom: 转换失败' >&2\nexit 3",
                out = out.display()
            ),
        );
        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
        let renderer = external::ExternalCli::new(Backend::LibreOffice, &script);

        let error = renderer
            .render(&tmp.path().join("deck.pptx"), &out, &mut NoopObserver, &diag)
            .expect_err("非零退出必须失败");

        match error {
            RenderError::Failed { detail, .. } => {
                assert!(detail.contains("boom"), "详情应带 stderr: {detail}");
            }
            other => panic!("错误形态不符: {other:?}"),
        }
        assert!(!out.join("slide_1.png").exists(), "半成品必须清理（I1）");
        let all = sink.events();
        assert!(
            all.iter().any(|event| {
                event.level == Level::Error && event.target == Target::Renderer.as_str()
            }),
            "渲染失败必须记 error: {all:?}"
        );
    }

    #[test]
    fn external_render_relays_stderr_warnings() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let out = tmp.path().join("out");
        let script = fake_renderer(
            tmp.path(),
            "warn.sh",
            &format!(
                "echo 'VISIBLE_WARN: 无法隐藏窗口' >&2\ntouch '{out}/slide_1.png'",
                out = out.display()
            ),
        );
        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
        let renderer = external::ExternalCli::new(Backend::PowerPoint, &script);

        let slides = renderer
            .render(&tmp.path().join("deck.pptx"), &out, &mut NoopObserver, &diag)
            .expect("成功的渲染必须收下输出");

        assert_eq!(slides.len(), 1);
        let all = sink.events();
        assert!(
            all.iter().any(|event| {
                event.level == Level::Warn && event.message.contains("VISIBLE_WARN: 无法隐藏窗口")
            }),
            "非致命 stderr 必须转发成 warn: {all:?}"
        );
    }

    #[test]
    fn external_cancel_kills_child_process() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let out = tmp.path().join("out");
        let pid_file = tmp.path().join("child.pid");
        let marker = tmp.path().join("finished.marker");
        let script = fake_renderer(
            tmp.path(),
            "slow.sh",
            &format!(
                "echo $$ > '{pid}'\nsleep 30\ntouch '{marker}'",
                pid = pid_file.display(),
                marker = marker.display()
            ),
        );
        let sink = MemorySink::new();
        let diag = Diag::with_sinks(vec![Box::new(sink.clone())]);
        let renderer = external::ExternalCli::new(Backend::LibreOffice, &script);
        let mut observer = CancelWhenRunning {
            pid_file: pid_file.clone(),
        };

        let started = Instant::now();
        let error = renderer
            .render(&tmp.path().join("deck.pptx"), &out, &mut observer, &diag)
            .expect_err("取消不是成功");
        let elapsed = started.elapsed();

        assert!(
            matches!(error, RenderError::Cancelled),
            "错误形态不符: {error:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "取消必须立刻返回（说明是杀掉的，不是等它自己跑完）: {elapsed:?}"
        );
        assert!(
            !marker.exists(),
            "子进程必须被杀（否则 30s 后会写出 marker）"
        );

        let pid = std::fs::read_to_string(&pid_file)
            .expect("脚本应写出自己的 pid")
            .trim()
            .to_string();
        assert!(!pid.is_empty(), "pid 不能为空");
        let alive = std::process::Command::new("ps")
            .args(["-p", &pid])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        assert!(!alive, "PID {pid} 必须已被 kill 并回收");

        let all = sink.events();
        assert!(
            all.iter().any(|event| {
                event.level == Level::Warn
                    && event.target == Target::Renderer.as_str()
                    && event.message.contains("取消")
            }),
            "取消必须记 warn: {all:?}"
        );
    }
}
