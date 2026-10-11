use std::path::{Path, PathBuf};
use std::process::Command;

use crate::diag::Diag;

/// Error from the external rendering step.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no external renderer available")]
    NoRenderer,
    #[error("renderer not found: {0}")]
    NotFound(String),
    #[error("renderer failed: {0}")]
    Failed(String),
}

/// Help text shown when no external renderer is found.
pub const RENDERER_HELP: &str = "\
未检测到可用的 PPTX→PNG 渲染器。请安装以下任一软件：

  Microsoft PowerPoint (Windows):
    已安装 Office 即可自动使用（通过 COM 自动化）

  LibreOffice (推荐，跨平台):
    Arch:     sudo pacman -S libreoffice-fresh
    Ubuntu:   sudo apt install libreoffice-impress
    macOS:    brew install --cask libreoffice
    Windows:  winget install LibreOffice.LibreOffice

  WPS Office:
    Linux:    https://www.wps.com/download/
    Windows:  https://www.wps.com/download/

安装后重新运行即可。";

#[derive(Debug, Clone, Copy, PartialEq)]
enum RendererKind {
    PowerPoint,
    LibreOffice,
    Wps,
}

#[derive(Debug, Clone)]
pub struct DetectedRenderer {
    pub name: &'static str,
    kind: RendererKind,
    exe: String,
}

/// Try to detect an available external PPTX→PNG renderer.
///
/// Priority: PowerPoint → LibreOffice → WPS → (none)
pub fn detect_renderer(diag: &Diag) -> Option<DetectedRenderer> {
    // Microsoft PowerPoint (Windows only, via COM)
    if cfg!(windows) {
        if let Some(r) = detect_powerpoint(diag) {
            return Some(r);
        }
    }

    // LibreOffice
    for name in &["libreoffice", "soffice"] {
        if Command::new(name).arg("--version").output().is_ok() {
            let r = DetectedRenderer {
                name: "LibreOffice",
                kind: RendererKind::LibreOffice,
                exe: name.to_string(),
            };
            diag.info(
                "renderer",
                format!("detected: {} ({name})", r.name),
                Some(serde_json::json!({"renderer": r.name, "exe": name})),
            );
            return Some(r);
        }
    }

    // WPS Office
    if Command::new("wps").arg("--version").output().is_ok() {
        let r = DetectedRenderer {
            name: "WPS Office",
            kind: RendererKind::Wps,
            exe: "wps".into(),
        };
        diag.info(
            "renderer",
            "detected: WPS Office (wps)",
            Some(serde_json::json!({"renderer": "WPS Office", "exe": "wps"})),
        );
        return Some(r);
    }

    diag.warn(
        "renderer",
        "no external renderer found",
        Some(serde_json::json!({"candidates": ["PowerPoint", "LibreOffice", "WPS"]})),
    );
    None
}

/// Check if PowerPoint COM automation is available.
fn detect_powerpoint(diag: &Diag) -> Option<DetectedRenderer> {
    // Test via PowerShell: can we create the PowerPoint COM object?
    let script = r#"try { $p = New-Object -ComObject PowerPoint.Application; $p.Quit(); Write-Output 'OK' } catch { Write-Output 'FAIL' }"#;
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let output = cmd.output().ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim() == "OK" {
        let r = DetectedRenderer {
            name: "Microsoft PowerPoint",
            kind: RendererKind::PowerPoint,
            exe: "powershell".into(),
        };
        diag.info(
            "renderer",
            "detected: Microsoft PowerPoint (COM)",
            Some(serde_json::json!({"renderer": "Microsoft PowerPoint", "via": "COM"})),
        );
        Some(r)
    } else {
        diag.info(
            "renderer",
            "PowerPoint COM not available",
            Some(serde_json::json!({"renderer": "PowerPoint", "available": false})),
        );
        None
    }
}

/// Render a PPTX file to PNG images using an external renderer.
pub fn render_pptx(
    pptx_path: &Path,
    output_dir: &Path,
    diag: &Diag,
) -> Result<Vec<PathBuf>, RenderError> {
    let renderer = detect_renderer(diag).ok_or(RenderError::NoRenderer)?;

    if let Err(error) = std::fs::create_dir_all(output_dir) {
        diag.error(
            "renderer",
            format!("创建输出目录失败: {}: {error}", output_dir.display()),
            Some(serde_json::json!({"output_dir": output_dir.display().to_string()})),
        );
        return Err(RenderError::Failed(format!("create output dir: {error}")));
    }

    match renderer.kind {
        RendererKind::PowerPoint => render_with_powerpoint(pptx_path, output_dir, diag),
        RendererKind::LibreOffice => {
            render_with_libreoffice(&renderer, pptx_path, output_dir, diag)
        }
        RendererKind::Wps => render_with_wps(&renderer, pptx_path, output_dir, diag),
    }
}

// ── PowerPoint COM renderer ──

fn render_with_powerpoint(
    pptx_path: &Path,
    output_dir: &Path,
    diag: &Diag,
) -> Result<Vec<PathBuf>, RenderError> {
    // Escape single quotes in paths for PowerShell string interpolation
    let pptx = pptx_path.to_string_lossy().replace('\'', "''");
    let out = output_dir.to_string_lossy().replace('\'', "''");

    // -WindowStyle Hidden: suppresses PowerShell console window
    // try/catch on Visible: some Office policies block hiding, but we try
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$ppt = New-Object -ComObject PowerPoint.Application
try {{ $ppt.Visible = [Microsoft.Office.Core.MsoTriState]::msoFalse }} catch {{ Write-Error "VISIBLE_WARN: $_" }}
try {{
    $pres = $ppt.Presentations.Open('{pptx}')
    foreach ($s in $pres.Slides) {{
        $path = Join-Path '{out}' "slide_$($s.SlideNumber).png"
        $s.Export($path, 'PNG', 1920, 1080)
    }}
    $pres.Close()
}} finally {{
    $ppt.Quit()
    [System.Runtime.InteropServices.Marshal]::ReleaseComObject($ppt) | Out-Null
}}"#
    );

    diag.info(
        "renderer",
        "calling Microsoft PowerPoint via COM",
        Some(serde_json::json!({"tool": "powerpoint"})),
    );

    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]);
    // CREATE_NO_WINDOW on Windows prevents any console flash
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let output = cmd
        .output()
        .map_err(|e| RenderError::Failed(format!("spawn PowerShell: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        diag.error(
            "renderer",
            format!("PowerPoint COM failed: {stderr}"),
            Some(serde_json::json!({"tool": "powerpoint"})),
        );
        return Err(RenderError::Failed(format!("PowerPoint: {stderr}")));
    }

    // 非致命警告（如 Visible 策略拦截）逐条转发，但有上限，防止噪声淹没日志。
    const RELAY_LIMIT: usize = 5;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    for line in lines.iter().copied().take(RELAY_LIMIT) {
        if line.contains("VISIBLE_WARN:") {
            diag.warn(
                "renderer",
                format!("PowerPoint: {line}"),
                Some(serde_json::json!({"source": "powershell", "stderr_line": line})),
            );
        } else {
            diag.info(
                "renderer",
                format!("PowerShell stderr: {line}"),
                Some(serde_json::json!({"source": "powershell", "stderr_line": line})),
            );
        }
    }
    if lines.len() > RELAY_LIMIT {
        diag.info(
            "renderer",
            format!("PowerShell stderr 共 {} 行，其余未转发", lines.len()),
            Some(serde_json::json!({"stderr_lines": lines.len(), "relayed": RELAY_LIMIT})),
        );
    }

    diag.info(
        "renderer",
        "PowerPoint: conversion complete",
        Some(serde_json::json!({"tool": "powerpoint"})),
    );

    collect_pngs_case_insensitive(output_dir, diag)
}

// ── LibreOffice renderer ──

fn render_with_libreoffice(
    r: &DetectedRenderer,
    pptx_path: &Path,
    output_dir: &Path,
    diag: &Diag,
) -> Result<Vec<PathBuf>, RenderError> {
    diag.info(
        "renderer",
        format!(
            "calling LibreOffice: {} --headless --convert-to png --outdir {} {}",
            r.exe,
            output_dir.display(),
            pptx_path.display(),
        ),
        Some(serde_json::json!({
            "tool": "libreoffice",
            "exe": r.exe,
            "output_dir": output_dir.display().to_string(),
        })),
    );

    let output = Command::new(&r.exe)
        .arg("--headless")
        .arg("--convert-to")
        .arg("png")
        .arg("--outdir")
        .arg(output_dir)
        .arg(pptx_path)
        .output()
        .map_err(|e| RenderError::Failed(format!("spawn LibreOffice: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        diag.error(
            "renderer",
            format!("LibreOffice failed: {stderr}"),
            Some(serde_json::json!({"tool": "libreoffice"})),
        );
        return Err(RenderError::Failed(format!("LibreOffice: {stderr}")));
    }

    diag.info(
        "renderer",
        "LibreOffice: conversion complete",
        Some(serde_json::json!({"tool": "libreoffice"})),
    );
    collect_pngs(output_dir, diag)
}

// ── WPS renderer ──

fn render_with_wps(
    r: &DetectedRenderer,
    pptx_path: &Path,
    output_dir: &Path,
    diag: &Diag,
) -> Result<Vec<PathBuf>, RenderError> {
    diag.info(
        "renderer",
        format!(
            "calling WPS: {} --headless --convert-to png --outdir {} {}",
            r.exe,
            output_dir.display(),
            pptx_path.display(),
        ),
        Some(serde_json::json!({
            "tool": "wps",
            "exe": r.exe,
            "output_dir": output_dir.display().to_string(),
        })),
    );

    let output = Command::new(&r.exe)
        .arg("--headless")
        .arg("--convert-to")
        .arg("png")
        .arg("--outdir")
        .arg(output_dir)
        .arg(pptx_path)
        .output()
        .map_err(|e| RenderError::Failed(format!("spawn WPS: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        diag.error(
            "renderer",
            format!("WPS failed: {stderr}"),
            Some(serde_json::json!({"tool": "wps"})),
        );
        return Err(RenderError::Failed(format!("WPS: {stderr}")));
    }

    diag.info(
        "renderer",
        "WPS: conversion complete",
        Some(serde_json::json!({"tool": "wps"})),
    );
    collect_pngs(output_dir, diag)
}

// ── Helpers ──

/// Collect PNG paths from output dir, sorted by slide number.
fn collect_pngs(dir: &Path, diag: &Diag) -> Result<Vec<PathBuf>, RenderError> {
    collect_pngs_impl(dir, diag, false)
}

/// Like `collect_pngs` but case-insensitive on extension (for PowerPoint .PNG output).
fn collect_pngs_case_insensitive(dir: &Path, diag: &Diag) -> Result<Vec<PathBuf>, RenderError> {
    collect_pngs_impl(dir, diag, true)
}

fn collect_pngs_impl(
    dir: &Path,
    diag: &Diag,
    case_insensitive: bool,
) -> Result<Vec<PathBuf>, RenderError> {
    let mut pngs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| RenderError::Failed(format!("read output dir: {e}")))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|e| {
                    if case_insensitive {
                        e.to_ascii_lowercase() == "png"
                    } else {
                        e == "png"
                    }
                })
                .unwrap_or(false)
        })
        .collect();

    if pngs.is_empty() {
        diag.warn(
            "renderer",
            "no PNG files found in output directory",
            Some(serde_json::json!({"output_dir": dir.display().to_string()})),
        );
        return Err(RenderError::Failed("no PNG output produced".into()));
    }

    pngs.sort();
    diag.info(
        "renderer",
        format!("collected {} PNG(s)", pngs.len()),
        Some(serde_json::json!({"count": pngs.len()})),
    );
    Ok(pngs)
}
