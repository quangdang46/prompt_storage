//! Clipboard via platform tools (bead P3.2) — no clipboard crate (locked
//! decision: keep the binary lean). Probes PATH once per process.

use std::process::{Command, Stdio};

/// Candidate tools in probe order per platform.
#[cfg(target_os = "macos")]
pub const CANDIDATES: &[&str] = &["pbcopy"];
#[cfg(all(unix, not(target_os = "macos")))]
pub const CANDIDATES: &[&str] = &["xclip", "xsel", "wl-copy"];
#[cfg(target_os = "windows")]
pub const CANDIDATES: &[&str] = &["clip"];

/// Find the first available clipboard tool.
pub fn detect_tool() -> Option<&'static str> {
    CANDIDATES.iter().copied().find(|t| which(t))
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Copy text to the clipboard by spawning the tool with a stdin pipe.
/// Returns Err(tool-name) on failure so callers can emit actionable errors.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let Some(tool) = detect_tool() else {
        return Err("no clipboard tool found".to_string());
    };
    let mut child = Command::new(tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{tool}: {e}"))?;
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .expect("piped")
        .write_all(text.as_bytes())
        .map_err(|e| format!("{tool}: {e}"))?;
    drop(child.stdin.take());
    let status = child.wait().map_err(|e| format!("{tool}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{tool} exited {status}"))
    }
}
