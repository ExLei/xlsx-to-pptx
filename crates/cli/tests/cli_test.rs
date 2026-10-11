//! CLI 集成测试：走 [`run_status`]（真实的用例层 + 平台层）。

use std::path::{Path, PathBuf};

use x2p_cli::{run_status, CliFailure};

fn sample_workbook() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/样例词表.xlsx")
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("utf-8 测试路径")
}

#[test]
fn template_export_writes_xlsx_from_asset() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("词表模板.xlsx");
    run_status(["x", "template", "-o", path_str(&out)]).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.len() > 1000, "词表模板应有内容");
}

#[test]
fn generate_produces_one_slide_per_entry() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("words.pptx");
    run_status(["x", "generate", "-i", path_str(&sample_workbook()), "-o", path_str(&out)]).unwrap();
    let package = x2p_core::platform::pptx::Package::open(&out, &x2p_core::diag::Diag::new()).unwrap();
    assert_eq!(package.slide_count(), 5, "样例词表 5 行 → 5 页（内嵌版式 5 页，截断/填充后仍 5）");
}

#[test]
fn generate_refuses_existing_output_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("words.pptx");
    std::fs::write(&out, b"occupied").unwrap();
    let failure = run_status(["x", "generate", "-i", path_str(&sample_workbook()), "-o", path_str(&out)])
        .expect_err("默认 Fail 策略应拒绝覆盖");
    assert!(matches!(failure, CliFailure::Run(_)), "{failure:?}");
    assert_eq!(std::fs::read(&out).unwrap(), b"occupied", "原文件不得被改动");
}

#[test]
fn force_overwrites_existing_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("words.pptx");
    std::fs::write(&out, b"occupied").unwrap();
    run_status(["x", "generate", "-i", path_str(&sample_workbook()), "-o", path_str(&out), "--force"]).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..2], b"PK", "覆盖后应是 PPTX（ZIP）");
}

#[test]
fn batch_continues_on_error_and_fails_overall() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in");
    let output = dir.path().join("out");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(
        input.join("good.csv"),
        "英文单词,英文音标,词根词缀,例句,例句释义,单词释义\napple,ˈæp.əl,ap-ple,I ate an apple.,我吃了一个苹果。,n. 苹果\n",
    )
    .unwrap();
    // 非法 UTF-8 字节：处理失败，但不得中断批量。
    std::fs::write(input.join("bad.csv"), b"\xff\xfe\x00\x01 broken").unwrap();

    let failure = run_status([
        "x",
        "batch",
        "-i",
        path_str(&input),
        "-o",
        path_str(&output),
    ])
    .expect_err("存在失败文件时应以非零退出");
    assert!(matches!(&failure, CliFailure::Run(message) if message.contains("部分文件生成失败")), "{failure:?}");
    assert!(output.join("good.pptx").exists(), "成功文件必须照常产出");
}

#[test]
fn export_png_builtin_writes_slides() {
    let dir = tempfile::tempdir().unwrap();
    run_status(["x", "export-png", "-i", path_str(&sample_workbook()), "-o", path_str(dir.path())]).unwrap();
    for index in 1..=5 {
        let slide = dir.path().join(format!("slide_{index}.png"));
        let bytes = std::fs::read(&slide).unwrap_or_else(|_| panic!("缺少 {}", slide.display()));
        assert!(bytes.len() > 1000, "slide_{index}.png 应有内容");
    }
}

#[test]
fn example_template_can_drive_generation() {
    let dir = tempfile::tempdir().unwrap();
    let template = dir.path().join("tpl.pptx");
    run_status(["x", "template-pptx", "-o", path_str(&template)]).unwrap();
    let out = dir.path().join("words.pptx");
    run_status([
        "x",
        "generate",
        "-i",
        path_str(&sample_workbook()),
        "-t",
        path_str(&template),
        "-o",
        path_str(&out),
    ])
    .unwrap();
    let package = x2p_core::platform::pptx::Package::open(&out, &x2p_core::diag::Diag::new()).unwrap();
    assert_eq!(package.slide_count(), 5);
}

#[test]
fn export_png_rejects_renderer_without_template() {
    let dir = tempfile::tempdir().unwrap();
    let failure = run_status([
        "x",
        "export-png",
        "-i",
        path_str(&sample_workbook()),
        "-o",
        path_str(dir.path()),
        "--renderer",
        "libreoffice",
    ])
    .expect_err("无模板模式指定外部渲染器应报错");
    assert!(matches!(failure, CliFailure::Run(_)), "{failure:?}");
}

#[test]
fn unknown_renderer_is_usage_error() {
    let failure = run_status(["x", "export-png", "-i", path_str(&sample_workbook()), "--renderer", "bogus"])
        .expect_err("未知渲染器应由 clap 拒绝");
    assert!(matches!(failure, CliFailure::Usage(_)), "{failure:?}");
}

#[test]
fn diag_missing_log_is_run_error() {
    let missing = std::env::temp_dir().join("x2p-does-not-exist.log");
    let failure = run_status(["x", "diag", path_str(&missing)]).expect_err("不存在的日志应报错");
    assert!(matches!(&failure, CliFailure::Run(message) if message.contains("未找到日志")), "{failure:?}");
}

#[test]
fn diag_summary_reads_latest_session_log() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("words.pptx");
    run_status(["x", "generate", "-i", path_str(&sample_workbook()), "-o", path_str(&out)]).unwrap();
    run_status(["x", "diag", "--summary"]).unwrap();
    run_status(["x", "diag", "--json"]).unwrap();
}
