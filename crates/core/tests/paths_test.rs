//! 数据目录解析：可写优先、不可写回退、路径组合。

use std::path::Path;

use vocab_core::paths::{Layout, DATA_DIR_NAME};

#[test]
fn writable_exe_dir_wins_and_is_created() {
    let exe_dir = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();

    let layout = Layout::resolve_with(exe_dir.path(), fallback.path());

    assert_eq!(layout.root(), exe_dir.path().join(DATA_DIR_NAME));
    assert!(layout.root().is_dir(), "数据目录应已创建");
    assert!(
        !fallback.path().join(DATA_DIR_NAME).exists(),
        "可写时不应触碰回退根"
    );
}

#[test]
fn blocked_data_dir_falls_back_to_user_root() {
    let exe_dir = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    // 用同名文件堵死程序同目录下的数据目录
    std::fs::write(exe_dir.path().join(DATA_DIR_NAME), b"occupied").unwrap();

    let layout = Layout::resolve_with(exe_dir.path(), fallback.path());

    assert_eq!(layout.root(), fallback.path().join(DATA_DIR_NAME));
}

#[test]
fn layout_composes_all_default_locations() {
    let layout = Layout::with_root("/data");

    assert_eq!(layout.logs_dir(), Path::new("/data/日志"));
    assert_eq!(layout.ppt_dir(), Path::new("/data/PPT"));
    assert_eq!(layout.png_dir(), Path::new("/data/PNG"));
    assert_eq!(layout.template_xlsx(), Path::new("/data/单词表模板.xlsx"));
    assert_eq!(layout.template_pptx(), Path::new("/data/示例模板.pptx"));
    assert_eq!(layout.ppt_file("words"), Path::new("/data/PPT/words.pptx"));
}
