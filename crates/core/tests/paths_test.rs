//! 数据目录解析：可写优先、不可写回退、路径组合。

use std::path::Path;

use x2p_core::paths::{Layout, DATA_DIR_NAME};

#[test]
fn writable_exe_dir_wins_and_is_created() {
    let exe_dir = tempfile::tempdir().unwrap();
    let user_root = tempfile::tempdir().unwrap();

    let layout = Layout::resolve_with(exe_dir.path(), user_root.path());

    assert_eq!(layout.root(), exe_dir.path().join(DATA_DIR_NAME));
    assert!(!layout.is_fallback(), "程序同目录可写时不回退");
    assert!(layout.root().is_dir(), "数据目录应已创建");
    assert!(
        !user_root.path().join(DATA_DIR_NAME).exists(),
        "可写时不应触碰回退根"
    );
}

#[test]
fn blocked_data_dir_falls_back_to_user_root() {
    let exe_dir = tempfile::tempdir().unwrap();
    let user_root = tempfile::tempdir().unwrap();
    // 用同名文件堵死程序同目录下的数据目录（跨平台都成立）。
    std::fs::write(exe_dir.path().join(DATA_DIR_NAME), b"occupied").unwrap();

    let layout = Layout::resolve_with(exe_dir.path(), user_root.path());

    assert_eq!(layout.root(), user_root.path().join(DATA_DIR_NAME));
    assert!(layout.is_fallback());
}

#[cfg(unix)]
#[test]
fn read_only_exe_dir_falls_back_to_user_root() {
    use std::os::unix::fs::PermissionsExt;

    let exe_dir = tempfile::tempdir().unwrap();
    let user_root = tempfile::tempdir().unwrap();

    let mut permissions = std::fs::metadata(exe_dir.path()).unwrap().permissions();
    permissions.set_mode(0o555);
    std::fs::set_permissions(exe_dir.path(), permissions).unwrap();

    if std::fs::write(exe_dir.path().join(".writable-check"), b"").is_ok() {
        // root 身份（CI 容器常见）能越过权限位，该分支在本次运行中不可测：恢复权限并跳过。
        let mut permissions = std::fs::metadata(exe_dir.path()).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(exe_dir.path(), permissions).unwrap();
        eprintln!("跳过：当前身份可越过只读权限位，无法构造不可写的程序目录");
        return;
    }

    let layout = Layout::resolve_with(exe_dir.path(), user_root.path());

    assert_eq!(layout.root(), user_root.path().join(DATA_DIR_NAME));
    assert!(layout.is_fallback());
}

#[test]
fn resolve_uses_process_paths_and_data_dir_name() {
    let layout = Layout::resolve();

    assert!(layout.root().ends_with(DATA_DIR_NAME));
    assert!(layout.logs_dir().ends_with("日志"));
}

#[test]
fn with_root_composes_all_default_locations() {
    let layout = Layout::with_root("/data");

    assert!(!layout.is_fallback(), "显式根不是回退");
    assert_eq!(layout.root(), Path::new("/data"));
    assert_eq!(layout.logs_dir(), Path::new("/data/日志"));
    assert_eq!(layout.ppt_dir(), Path::new("/data/PPT"));
    assert_eq!(layout.png_dir(), Path::new("/data/PNG"));
    assert_eq!(layout.template_xlsx(), Path::new("/data/单词表模板.xlsx"));
    assert_eq!(layout.template_pptx(), Path::new("/data/示例模板.pptx"));
    assert_eq!(layout.ppt_file("words"), Path::new("/data/PPT/words.pptx"));
}
