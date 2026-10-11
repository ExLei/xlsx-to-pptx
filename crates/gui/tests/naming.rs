//! 输出名自动重命名：不覆盖已有文件，也不叠加序号。

use gui::app::find_available_path;

/// 目标已存在时取下一个空位；再次生成时基于原名继续编号（不叠成 (1) (1)）。
#[test]
fn existing_target_gets_the_next_free_number() {
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("words.pptx");
    std::fs::write(&target, b"x").expect("write");

    let next = find_available_path(&target);
    assert_eq!(next.file_name().unwrap(), "words (1).pptx");

    std::fs::write(&next, b"x").expect("write");
    let third = find_available_path(&next);
    assert_eq!(third.file_name().unwrap(), "words (2).pptx");
}

/// 目标不存在时保持请求的路径不变。
#[test]
fn missing_target_keeps_the_requested_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("words.pptx");
    assert_eq!(find_available_path(&target), target);
}
