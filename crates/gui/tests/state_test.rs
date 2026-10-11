//! 状态机转移表测试：纯事件序列、无窗口（`docs/design.md` §8.4 的落地契约）。
//!
//! 状态只经 `reduce` 驱动（守卫函数按需直接调用），不涉及 exec / 视图。

use std::path::PathBuf;

use x2p_core::domain::plan::OverwritePolicy;
use x2p_core::domain::schema::{Field, SheetInfo, WordEntry};
use x2p_core::ports::Progress;
use x2p_gui::state::{
    can_export_png, can_generate, reduce, AppState, ArtifactKind, ArtifactView, Command, Event, Job,
    JobResult, LoadOutcome, NotifyLevel, OutputField, Probe, ProbeResult, Prior, RendererField,
    Screen, TemplateField,
};

// ── 夹具 ──────────────────────────────────────────────────────────────────

fn input_path() -> PathBuf {
    PathBuf::from("/tmp/vocab.xlsx")
}

fn template_path() -> PathBuf {
    PathBuf::from("/tmp/template.pptx")
}

fn word(index: usize) -> WordEntry {
    WordEntry { word: format!("word{index}"), ..WordEntry::default() }
}

fn outcome(entries: usize) -> LoadOutcome {
    LoadOutcome {
        sheets: vec![SheetInfo { name: "Sheet1".to_string() }],
        sheet: Some("Sheet1".to_string()),
        entries: (0..entries).map(word).collect(),
    }
}

/// Empty → PickInput → Busy(Load) → JobFinished::Loaded → Loaded。
fn loaded(entries: usize) -> AppState {
    let mut state = AppState::new();
    let commands = reduce(&mut state, Event::PickInput(input_path()));
    assert!(matches!(commands.as_slice(), [Command::SpawnJob(_)]), "PickInput 应派发 Load 作业");
    assert!(matches!(state.screen, Screen::Busy { job: Job::Load, .. }));
    let commands = reduce(
        &mut state,
        Event::JobFinished { job: Job::Load, result: JobResult::Loaded(outcome(entries)) },
    );
    assert!(commands.is_empty(), "加载成功不需要命令");
    assert!(matches!(state.screen, Screen::Loaded), "加载完成应进入 Loaded");
    assert_eq!(state.document.entries.len(), entries);
    state
}

/// Loaded → Generate → Busy(Generate)。
fn generating(entries: usize) -> AppState {
    let mut state = loaded(entries);
    let commands = reduce(&mut state, Event::Generate);
    assert!(matches!(commands.as_slice(), [Command::SpawnJob(_)]), "Generate 应派发作业");
    assert!(matches!(state.screen, Screen::Busy { job: Job::Generate, .. }));
    state
}

fn spawn_probe_seq(commands: Vec<Command>) -> u64 {
    match commands.as_slice() {
        [Command::SpawnProbe { seq, .. }] => *seq,
        other => panic!("期望 SpawnProbe，实际：{other:?}"),
    }
}

fn notify_text(commands: &[Command]) -> &str {
    match commands {
        [Command::Notify { text, .. }] => text,
        other => panic!("期望恰好一条 Notify，实际：{other:?}"),
    }
}

/// 选定模板并让探针落地为 `Ready`。
fn set_template_ready(state: &mut AppState) {
    let seq = spawn_probe_seq(reduce(state, Event::TemplateChanged(Some(template_path()))));
    let commands = reduce(
        state,
        Event::ProbeFinished {
            probe: Probe::Template,
            seq,
            result: ProbeResult::Template {
                slides: 1,
                fields: vec![Field::Word],
                missing_word: false,
            },
        },
    );
    assert!(commands.is_empty());
    assert!(matches!(state.document.template, TemplateField::Ready { .. }));
}

/// 用当前序号把渲染器探测结果写进字段。
fn set_renderer(state: &mut AppState, field: RendererField) {
    let seq = state.renderer_probe;
    reduce(
        state,
        Event::ProbeFinished { probe: Probe::Renderer, seq, result: ProbeResult::Renderer(field) },
    );
}

// ── 忙碌期 ────────────────────────────────────────────────────────────────

/// 八类文档输入事件 + 两个动作事件在 Busy 下全部被拒：状态不变、各有「忙碌」提示。
#[test]
fn busy_rejects_document_events() {
    let mut state = generating(3);
    let before = state.clone();
    let events = [
        // 八类文档输入
        Event::PickInput(PathBuf::from("/tmp/other.xlsx")),
        Event::DropInput(PathBuf::from("/tmp/dropped.csv")),
        Event::InputCleared,
        Event::SelectSheet("Sheet2".to_string()),
        Event::SelectEncoding("GBK".to_string()),
        Event::TemplateChanged(Some(template_path())),
        Event::OutputChanged(PathBuf::from("/tmp/out.pptx")),
        Event::FileChanged { path: input_path() },
        // 两个动作（快捷键与按钮同源，同样不得绕过）
        Event::Generate,
        Event::ExportPng,
    ];
    for event in events {
        let commands = reduce(&mut state, event.clone());
        assert_eq!(state, before, "Busy 期 {event:?} 不得改变状态");
        match commands.as_slice() {
            [Command::Notify { level: NotifyLevel::Warn, text }] => {
                assert!(text.contains("忙碌"), "{event:?} 的提示应含「忙碌」，实际：{text}");
            }
            other => panic!("{event:?} 应被显式拒绝（恰好一条 Warn 通知），实际：{other:?}"),
        }
    }
}

#[test]
fn cancel_returns_to_prior_screen() {
    let mut state = generating(3);
    assert_eq!(reduce(&mut state, Event::Cancel), vec![Command::CancelJob]);
    match &state.screen {
        Screen::Busy { cancel_requested, prior, .. } => {
            assert!(*cancel_requested, "Cancel 应置 cancel_requested");
            assert_eq!(*prior, Prior::Loaded);
        }
        other => panic!("取消请求期间仍在 Busy：{other:?}"),
    }

    let commands =
        reduce(&mut state, Event::JobFinished { job: Job::Generate, result: JobResult::Cancelled });
    assert!(matches!(state.screen, Screen::Loaded), "取消应回到发起前的 Loaded");
    assert!(!matches!(state.screen, Screen::Failed { .. }), "取消是合法终态，不是失败");
    assert_eq!(state.document.entries.len(), 3, "取消不动词条");
    assert_eq!(state.progress, None);
    match commands.as_slice() {
        [Command::Notify { level: NotifyLevel::Info, text }] => {
            assert!(text.contains("已取消"), "{text}")
        }
        other => panic!("取消应给出一条 Info 通知，实际：{other:?}"),
    }
}

#[test]
fn cancelled_load_returns_to_prior() {
    let mut state = AppState::new();
    reduce(&mut state, Event::PickInput(input_path()));
    reduce(&mut state, Event::Cancel);
    reduce(&mut state, Event::JobFinished { job: Job::Load, result: JobResult::Cancelled });
    assert_eq!(state.screen, Screen::Empty, "Empty 发起的加载取消后回 Empty");
}

// ── 守卫 ──────────────────────────────────────────────────────────────────

#[test]
fn generate_guard_matches_event_and_button() {
    // 无词条：按钮禁用，事件（快捷键路径）同样被拒且不改状态
    let mut empty = AppState::new();
    assert!(!can_generate(&empty));
    let before = empty.clone();
    let commands = reduce(&mut empty, Event::Generate);
    assert!(notify_text(&commands).contains("词条"), "{commands:?}");
    assert_eq!(empty, before, "被拒的 Generate 不得改变状态");

    // 有词条：守卫通过 → Busy + SpawnJob，词条是快照
    let mut state = loaded(2);
    assert!(can_generate(&state));
    let entries = state.document.entries.clone();
    let commands = reduce(&mut state, Event::Generate);
    let spec = match &state.screen {
        Screen::Busy { job, spec, prior, cancel_requested } => {
            assert_eq!(*job, Job::Generate);
            assert_eq!(*prior, Prior::Loaded);
            assert!(!*cancel_requested, "新作业的取消标志未置位");
            assert_eq!(spec.entries, entries, "spec.entries 是生成用的预览真值");
            assert_eq!(state.progress, None, "进入 Busy 清空进度");
            spec.clone()
        }
        other => panic!("Generate 应进入 Busy：{other:?}"),
    };
    assert_eq!(commands, vec![Command::SpawnJob(spec.clone())]);
    // 快照与状态解耦：事后改文档不影响在飞作业的参数
    state.document.entries.clear();
    assert_eq!(spec.entries, entries);
}

#[test]
fn job_spec_snapshots_document() {
    let mut state = loaded(2);
    set_template_ready(&mut state);
    let commands = reduce(&mut state, Event::Generate);
    let spec = match commands.as_slice() {
        [Command::SpawnJob(spec)] => spec.clone(),
        other => panic!("Generate 应派发 SpawnJob：{other:?}"),
    };
    assert_eq!(spec.source, Some(input_path()));
    assert_eq!(spec.sheet.as_deref(), Some("Sheet1"));
    assert_eq!(spec.encoding, "UTF-8");
    assert_eq!(spec.template, Some(template_path()));
    assert_eq!(spec.entries.len(), 2);
    assert_eq!(
        spec.output.policy,
        OverwritePolicy::Suffix,
        "GUI 生成默认 Suffix：不覆盖旧产物（D9）"
    );
}

#[test]
fn export_guard_requires_renderer_when_template() {
    let mut state = loaded(2);
    set_template_ready(&mut state);
    set_renderer(&mut state, RendererField::Missing);
    assert!(!can_export_png(&state), "模板模式下渲染器缺失不得导出 PNG");

    let before = state.clone();
    let commands = reduce(&mut state, Event::ExportPng);
    assert!(notify_text(&commands).contains("渲染器"), "{commands:?}");
    assert_eq!(state, before, "被拒的 ExportPng 不得改变状态");

    // 渲染器可用 → 放行（导出落目录、逐页原子覆盖）
    set_renderer(&mut state, RendererField::Available { name: "LibreOffice".to_string() });
    assert!(can_export_png(&state));
    let commands = reduce(&mut state, Event::ExportPng);
    match commands.as_slice() {
        [Command::SpawnJob(spec)] => {
            assert_eq!(spec.output.policy, OverwritePolicy::Overwrite)
        }
        other => panic!("ExportPng 应派发 SpawnJob：{other:?}"),
    }
    assert!(matches!(state.screen, Screen::Busy { job: Job::ExportPng, .. }));

    // 无模板：内置管线，不需要渲染器
    let mut builtin = loaded(2);
    set_renderer(&mut builtin, RendererField::Missing);
    assert!(can_export_png(&builtin), "无模板导出走内置管线");
}

// ── 作业收尾 ──────────────────────────────────────────────────────────────

#[test]
fn job_finished_failed_then_retry() {
    let mut state = generating(2);
    let spec = match &state.screen {
        Screen::Busy { spec, .. } => spec.clone(),
        other => panic!("期望 Busy：{other:?}"),
    };
    let commands = reduce(
        &mut state,
        Event::JobFinished {
            job: Job::Generate,
            result: JobResult::Failed("生成失败：模板损坏".to_string()),
        },
    );
    assert!(commands.is_empty(), "失败以状态表达（Failed 屏），不再重复通知");
    match &state.screen {
        Screen::Failed { error, retry: Some(retry) } => {
            assert!(error.contains("模板损坏"), "错误原文进状态：{error}");
            assert_eq!(retry.job, Job::Generate);
            assert_eq!(retry.spec, spec, "Retry 重放同一份 spec");
        }
        other => panic!("失败应进入 Failed{{retry: Some}}：{other:?}"),
    }

    let commands = reduce(&mut state, Event::Retry);
    assert_eq!(commands, vec![Command::SpawnJob(spec.clone())]);
    match &state.screen {
        Screen::Busy { job, spec: running, prior, .. } => {
            assert_eq!(*job, Job::Generate);
            assert_eq!(*running, spec);
            assert_eq!(*prior, Prior::Loaded, "有词条 → 取消后回到 Loaded");
        }
        other => panic!("Retry 应重放为 Busy：{other:?}"),
    }
}

#[test]
fn failed_without_retry_action_is_inert() {
    let mut state = loaded(2);
    state.screen = Screen::Failed { error: "不可重试".to_string(), retry: None };
    let before = state.clone();
    assert!(reduce(&mut state, Event::Retry).is_empty());
    assert_eq!(state, before);
}

#[test]
fn done_dismiss_returns_loaded() {
    let mut state = generating(2);
    let artifact =
        ArtifactView { kind: ArtifactKind::Pptx, path: PathBuf::from("/tmp/out.pptx"), slides: 2 };
    let commands = reduce(
        &mut state,
        Event::JobFinished { job: Job::Generate, result: JobResult::Generated(artifact.clone()) },
    );
    assert!(commands.is_empty());
    assert_eq!(state.screen, Screen::Done { artifact });
    assert_eq!(state.progress, None);

    let entries = state.document.entries.clone();
    assert!(reduce(&mut state, Event::DismissError).is_empty(), "关闭产物卡不需要命令");
    assert!(matches!(state.screen, Screen::Loaded));
    assert_eq!(state.document.entries, entries, "词条在产物屏之后保留");

    // Failed 屏同样经 DismissError 回 Loaded
    let mut state = generating(1);
    reduce(
        &mut state,
        Event::JobFinished { job: Job::Generate, result: JobResult::Failed("坏".to_string()) },
    );
    assert!(reduce(&mut state, Event::DismissError).is_empty());
    assert!(matches!(state.screen, Screen::Loaded));
    assert_eq!(state.document.entries.len(), 1);
}

#[test]
fn exported_result_lands_as_png_artifact() {
    let mut state = loaded(2);
    set_renderer(&mut state, RendererField::Available { name: "LibreOffice".to_string() });
    reduce(&mut state, Event::ExportPng);
    let dir = PathBuf::from("/tmp/png");
    reduce(
        &mut state,
        Event::JobFinished {
            job: Job::ExportPng,
            result: JobResult::Exported(ArtifactView {
                kind: ArtifactKind::Pptx, // 载荷里的 kind 不作数：作业种类才是权威
                path: dir.clone(),
                slides: 4,
            }),
        },
    );
    match &state.screen {
        Screen::Done { artifact } => {
            assert_eq!(artifact.kind, ArtifactKind::Png, "Exported → PNG 产物卡");
            assert_eq!(artifact.path, dir);
            assert_eq!(artifact.slides, 4);
        }
        other => panic!("导出完成应进入 Done：{other:?}"),
    }
}

#[test]
fn open_artifact_only_in_done() {
    let mut loaded_state = loaded(1);
    assert!(reduce(&mut loaded_state, Event::OpenArtifact).is_empty());
    let mut busy = generating(1);
    assert!(reduce(&mut busy, Event::OpenArtifact).is_empty());
    let mut empty = AppState::new();
    assert!(reduce(&mut empty, Event::OpenArtifact).is_empty());

    let path = PathBuf::from("/tmp/out.pptx");
    let mut state = generating(1);
    reduce(
        &mut state,
        Event::JobFinished {
            job: Job::Generate,
            result: JobResult::Generated(ArtifactView {
                kind: ArtifactKind::Pptx,
                path: path.clone(),
                slides: 1,
            }),
        },
    );
    assert_eq!(reduce(&mut state, Event::OpenArtifact), vec![Command::OpenPath(path)]);
}

#[test]
fn mismatched_job_signals_are_ignored() {
    let mut state = generating(2);
    let before = state.clone();
    // 别的作业完成：丢弃
    let commands = reduce(
        &mut state,
        Event::JobFinished {
            job: Job::ExportPng,
            result: JobResult::Exported(ArtifactView {
                kind: ArtifactKind::Png,
                path: PathBuf::from("/tmp/png"),
                slides: 1,
            }),
        },
    );
    assert!(commands.is_empty());
    assert_eq!(state, before);
    // 结果种类与作业不符：丢弃
    let commands = reduce(
        &mut state,
        Event::JobFinished { job: Job::Generate, result: JobResult::Loaded(outcome(1)) },
    );
    assert!(commands.is_empty());
    assert_eq!(state, before);
    // 非 Busy 的作业信号：陈旧
    let mut idle = loaded(1);
    let before_idle = idle.clone();
    let commands =
        reduce(&mut idle, Event::JobFinished { job: Job::Generate, result: JobResult::Cancelled });
    assert!(commands.is_empty());
    assert_eq!(idle, before_idle);
}

// ── 输入与重新派发 ────────────────────────────────────────────────────────

#[test]
fn empty_can_pick_input() {
    let mut state = AppState::new();
    assert_eq!(state.screen, Screen::Empty);
    assert_eq!(state.renderer, RendererField::Probing, "渲染器探测在飞（由 exec 层发起）");
    assert!(state.document.entries.is_empty());

    let commands = reduce(&mut state, Event::PickInput(input_path()));
    match &state.screen {
        Screen::Busy { job, spec, prior, cancel_requested } => {
            assert_eq!(*job, Job::Load);
            assert_eq!(*prior, Prior::Empty, "空状态取消后回到 Empty");
            assert!(!*cancel_requested);
            assert_eq!(spec.source.as_deref(), Some(input_path().as_path()));
        }
        other => panic!("PickInput 应进入 Busy(Load)：{other:?}"),
    }
    match commands.as_slice() {
        [Command::SpawnJob(spec)] => {
            assert_eq!(spec.source.as_deref(), Some(input_path().as_path()));
            assert!(spec.entries.is_empty(), "Load 没有预览真值");
        }
        other => panic!("PickInput 应派发 SpawnJob：{other:?}"),
    }
}

#[test]
fn pick_input_resets_derived_state() {
    let mut state = loaded(3);
    set_template_ready(&mut state);
    reduce(&mut state, Event::OutputChanged(PathBuf::from("/tmp/custom.pptx")));

    let commands = reduce(&mut state, Event::DropInput(PathBuf::from("/tmp/dropped.csv")));
    assert!(state.document.entries.is_empty(), "新文件清空词条");
    assert!(state.document.sheets.is_empty(), "工作表列表属于旧文件");
    assert_eq!(state.document.sheet, None);
    assert_eq!(state.document.source, Some(PathBuf::from("/tmp/dropped.csv")));
    assert_eq!(state.document.template, TemplateField::Unset, "模板复位");
    assert_eq!(state.document.template_path, None);
    assert_eq!(state.document.output, OutputField::Auto, "输出回到自动派生");
    assert!(matches!(state.screen, Screen::Busy { job: Job::Load, prior: Prior::Loaded, .. }));
    assert!(matches!(commands.as_slice(), [Command::SpawnJob(_)]));
}

#[test]
fn load_finish_updates_document() {
    let mut state = AppState::new();
    reduce(&mut state, Event::PickInput(input_path()));
    let commands = reduce(
        &mut state,
        Event::JobFinished { job: Job::Load, result: JobResult::Loaded(outcome(3)) },
    );
    assert!(commands.is_empty(), "加载成功不需要通知（数据即状态）");
    assert!(matches!(state.screen, Screen::Loaded));
    assert_eq!(state.document.source, Some(input_path()), "source 来自作业快照");
    assert_eq!(state.document.sheets, vec![SheetInfo { name: "Sheet1".to_string() }]);
    assert_eq!(state.document.sheet.as_deref(), Some("Sheet1"));
    assert_eq!(state.document.entries.len(), 3);
    assert_eq!(state.progress, None);
}

#[test]
fn select_sheet_without_source_is_refused() {
    let mut state = AppState::new();
    let before = state.clone();
    let commands = reduce(&mut state, Event::SelectSheet("Sheet2".to_string()));
    assert!(notify_text(&commands).contains("输入"), "{commands:?}");
    assert_eq!(state, before);
    let commands = reduce(&mut state, Event::SelectEncoding("GBK".to_string()));
    assert!(notify_text(&commands).contains("输入"), "{commands:?}");
    assert_eq!(state, before);
}

#[test]
fn select_encoding_redispatches_load() {
    let mut state = loaded(2);
    let commands = reduce(&mut state, Event::SelectEncoding("GBK".to_string()));
    assert_eq!(state.document.encoding, "GBK");
    match &state.screen {
        Screen::Busy { job, spec, prior, .. } => {
            assert_eq!(*job, Job::Load);
            assert_eq!(*prior, Prior::Loaded, "有词条 → 取消后回到 Loaded");
            assert_eq!(spec.encoding, "GBK", "重读用新编码");
        }
        other => panic!("改编码应重新派发 Load：{other:?}"),
    }
    assert!(matches!(commands.as_slice(), [Command::SpawnJob(_)]));

    reduce(
        &mut state,
        Event::JobFinished { job: Job::Load, result: JobResult::Loaded(outcome(1)) },
    );
    assert!(matches!(state.screen, Screen::Loaded));
    assert_eq!(state.document.entries.len(), 1);
}

#[test]
fn select_sheet_redispatches_load() {
    let mut state = loaded(2);
    let commands = reduce(&mut state, Event::SelectSheet("Sheet2".to_string()));
    assert_eq!(state.document.sheet.as_deref(), Some("Sheet2"));
    assert!(matches!(state.screen, Screen::Busy { job: Job::Load, .. }));
    assert!(matches!(commands.as_slice(), [Command::SpawnJob(_)]));
}

#[test]
fn output_changed_validates_path() {
    let mut state = loaded(1);
    reduce(&mut state, Event::OutputChanged(PathBuf::from("/tmp/out.pptx")));
    assert_eq!(state.document.output, OutputField::Taken { path: PathBuf::from("/tmp/out.pptx") });
    assert!(can_generate(&state));

    reduce(&mut state, Event::OutputChanged(PathBuf::from("/tmp/out.txt")));
    match &state.document.output {
        OutputField::Invalid { reason } => assert!(reason.contains("pptx"), "{reason}"),
        other => panic!("非 pptx 输出应判非法：{other:?}"),
    }
    assert!(!can_generate(&state), "输出非法时守卫拒绝");
    let before = state.clone();
    let commands = reduce(&mut state, Event::Generate);
    assert!(notify_text(&commands).contains("pptx"), "{commands:?}");
    assert_eq!(state, before);

    reduce(&mut state, Event::OutputChanged(PathBuf::new()));
    match &state.document.output {
        OutputField::Invalid { reason } => assert!(reason.contains("不能为空"), "{reason}"),
        other => panic!("空输出应判非法：{other:?}"),
    }
}

#[test]
fn input_cleared_resets_to_empty() {
    let mut state = loaded(2);
    assert!(reduce(&mut state, Event::InputCleared).is_empty());
    assert_eq!(state.screen, Screen::Empty);
    assert_eq!(state.document.source, None);
    assert!(state.document.entries.is_empty());
    assert!(state.document.sheets.is_empty());
    assert_eq!(state.document.output, OutputField::Auto);
    assert_eq!(state.document.template, TemplateField::Unset);
    assert_eq!(state.document.encoding, "UTF-8", "编码回到默认");
    assert_eq!(state.progress, None);
}

#[test]
fn file_changed_reloads_only_current_source() {
    let mut state = loaded(2);
    let before = state.clone();
    assert!(reduce(&mut state, Event::FileChanged { path: PathBuf::from("/tmp/other.xlsx") })
        .is_empty());
    assert_eq!(state, before, "别的文件（陈旧 watcher / 产物）不得触发重读");

    let commands = reduce(&mut state, Event::FileChanged { path: input_path() });
    assert!(matches!(state.screen, Screen::Busy { job: Job::Load, prior: Prior::Loaded, .. }));
    assert!(matches!(commands.as_slice(), [Command::SpawnJob(_)]));
}

// ── 探针与进度 ────────────────────────────────────────────────────────────

#[test]
fn probe_stale_results_are_dropped() {
    let mut state = loaded(1);
    let first =
        spawn_probe_seq(reduce(&mut state, Event::TemplateChanged(Some(PathBuf::from("/tmp/a.pptx")))));
    let second =
        spawn_probe_seq(reduce(&mut state, Event::TemplateChanged(Some(PathBuf::from("/tmp/b.pptx")))));
    assert_ne!(first, second, "每次改动换序号");
    assert_eq!(state.document.template, TemplateField::Checking);

    reduce(
        &mut state,
        Event::ProbeFinished {
            probe: Probe::Template,
            seq: first,
            result: ProbeResult::Template {
                slides: 9,
                fields: vec![Field::Word],
                missing_word: false,
            },
        },
    );
    assert_eq!(state.document.template, TemplateField::Checking, "过期序号的结果不得写回");

    reduce(
        &mut state,
        Event::ProbeFinished {
            probe: Probe::Template,
            seq: second,
            result: ProbeResult::Template {
                slides: 1,
                fields: vec![Field::Word, Field::Definition],
                missing_word: false,
            },
        },
    );
    assert_eq!(
        state.document.template,
        TemplateField::Ready { slides: 1, fields: vec![Field::Word, Field::Definition] }
    );
}

#[test]
fn template_field_tracks_path_and_missing_word() {
    let mut state = loaded(1);
    let seq = spawn_probe_seq(reduce(&mut state, Event::TemplateChanged(Some(template_path()))));
    assert_eq!(state.document.template_path, Some(template_path()), "路径进状态（生成快照要用）");
    assert_eq!(state.document.template, TemplateField::Checking);

    // 缺 {{单词}} → Invalid，理由点名占位符
    reduce(
        &mut state,
        Event::ProbeFinished {
            probe: Probe::Template,
            seq,
            result: ProbeResult::Template {
                slides: 1,
                fields: vec![Field::Definition],
                missing_word: true,
            },
        },
    );
    match &state.document.template {
        TemplateField::Invalid { reason } => assert!(reason.contains("{{单词}}"), "{reason}"),
        other => panic!("缺必填占位符应判非法：{other:?}"),
    }
    assert!(!can_generate(&state), "模板非法时守卫拒绝");

    // 清空模板 → Unset，并让在飞探针作废
    let seq = spawn_probe_seq(reduce(&mut state, Event::TemplateChanged(Some(template_path()))));
    assert!(reduce(&mut state, Event::TemplateChanged(None)).is_empty());
    assert_eq!(state.document.template, TemplateField::Unset);
    assert_eq!(state.document.template_path, None);
    reduce(
        &mut state,
        Event::ProbeFinished {
            probe: Probe::Template,
            seq,
            result: ProbeResult::Template {
                slides: 1,
                fields: vec![Field::Word],
                missing_word: false,
            },
        },
    );
    assert_eq!(state.document.template, TemplateField::Unset, "清空后到达的旧结果不得写回");
}

#[test]
fn renderer_probe_updates_field() {
    let mut state = AppState::new();
    assert_eq!(state.renderer, RendererField::Probing);
    let seq = state.renderer_probe;
    reduce(
        &mut state,
        Event::ProbeFinished {
            probe: Probe::Renderer,
            seq,
            result: ProbeResult::Renderer(RendererField::Available {
                name: "LibreOffice".to_string(),
            }),
        },
    );
    let available = RendererField::Available { name: "LibreOffice".to_string() };
    assert_eq!(state.renderer, available);

    reduce(
        &mut state,
        Event::ProbeFinished {
            probe: Probe::Renderer,
            seq: seq + 1,
            result: ProbeResult::Renderer(RendererField::Missing),
        },
    );
    assert_eq!(state.renderer, available, "过期序号丢弃");

    reduce(
        &mut state,
        Event::ProbeFinished {
            probe: Probe::Renderer,
            seq,
            result: ProbeResult::Template {
                slides: 1,
                fields: vec![Field::Word],
                missing_word: false,
            },
        },
    );
    assert_eq!(state.renderer, available, "载荷与探针不符丢弃");
    assert_eq!(state.document.template, TemplateField::Unset, "渲染器探针不碰模板字段");
}

#[test]
fn progress_updates_state() {
    let mut state = generating(3);
    assert_eq!(state.progress, None, "进入 Busy 清空进度");
    let progress = Progress::Slides { done: 1, total: 3 };
    assert!(reduce(&mut state, Event::JobProgress(progress.clone())).is_empty());
    assert_eq!(state.progress, Some(progress));

    reduce(
        &mut state,
        Event::JobFinished {
            job: Job::Generate,
            result: JobResult::Generated(ArtifactView {
                kind: ArtifactKind::Pptx,
                path: PathBuf::from("/tmp/out.pptx"),
                slides: 3,
            }),
        },
    );
    assert_eq!(state.progress, None, "离开 Busy 清空进度");

    reduce(&mut state, Event::JobProgress(Progress::Stage("填充模板")));
    assert_eq!(state.progress, None, "非 Busy 的进度信号是陈旧的");
}
