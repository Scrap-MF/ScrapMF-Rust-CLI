//! Termux MediaStore scan — best-effort, never fails a scrape.
//! Runs `termux-media-scan <scrapmf_dir>` once per finished download.
//! Disabled by default (opt-in via Plugins → Termux MediaScan).

use std::path::Path;

/// Single dir scan (1 command, prioritizes speed on Termux).
/// Best-effort: missing binary / non-Termux → silent no-op with tracing.
pub fn maybe_scan(dir: &Path) {
    if !crate::plugins::termux_scan_enabled() {
        return;
    }
    let bin = std::env::var_os("SCRAPMF_TERMUX_SCAN_BIN")
        .map(std::path::PathBuf::from)
        .or_else(|| which::which("termux-media-scan").ok());
    let Some(bin) = bin else {
        tracing::warn!(
            "termux-scan enabled but termux-media-scan not found — install termux:api app + pkg install termux-api"
        );
        return;
    };
    // Ensure dir exists before scanning (avoid scanning non-existent path)
    if !dir.exists() {
        tracing::debug!(dir=%dir.display(), "termux scan skipped: dir does not exist yet");
        return;
    }
    match std::process::Command::new(&bin).arg(dir).status() {
        Ok(s) if s.success() => {
            tracing::info!(dir=%dir.display(), "termux media scan done");
        }
        Ok(s) => {
            tracing::warn!(dir=%dir.display(), code=?s.code(), "termux-media-scan non-zero");
        }
        Err(e) => {
            tracing::warn!(dir=%dir.display(), error=%e, "termux-media-scan failed");
        }
    }
}
