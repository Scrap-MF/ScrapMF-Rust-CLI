//! Termux MediaStore scan — best-effort, never fails a scrape.
//! Runs `termux-media-scan <scrapmf_dir>` once per finished download.
//! Disabled by default (opt-in via Plugins → Termux MediaScan).

use std::path::Path;

/// Result of one scan attempt, so callers (interactive menu) can report what
/// actually happened instead of always claiming success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanOutcome {
    /// Plugin toggled off — no command ran.
    Disabled,
    /// `termux-media-scan` is not on PATH (termux:api app / pkg missing).
    MissingBinary,
    /// Target directory does not exist.
    MissingDir,
    /// Command ran and exited 0.
    Done,
    /// Command ran and exited non-zero.
    NonZero(Option<i32>),
    /// Command could not be spawned.
    Failed(String),
}

/// Single dir scan (1 command, prioritizes speed on Termux).
/// Best-effort: missing binary / non-Termux → silent no-op with tracing.
pub fn maybe_scan(dir: &Path) -> ScanOutcome {
    if !crate::plugins::termux_scan_enabled() {
        return ScanOutcome::Disabled;
    }
    let bin = std::env::var_os("SCRAPMF_TERMUX_SCAN_BIN")
        .map(std::path::PathBuf::from)
        .or_else(|| which::which("termux-media-scan").ok());
    let Some(bin) = bin else {
        tracing::warn!(
            "termux-scan enabled but termux-media-scan not found — install termux:api app + pkg install termux-api"
        );
        return ScanOutcome::MissingBinary;
    };
    // Ensure dir exists before scanning (avoid scanning non-existent path)
    if !dir.exists() {
        tracing::debug!(dir=%dir.display(), "termux scan skipped: dir does not exist yet");
        return ScanOutcome::MissingDir;
    }
    match std::process::Command::new(&bin).arg(dir).status() {
        Ok(s) if s.success() => {
            tracing::info!(dir=%dir.display(), "termux media scan done");
            ScanOutcome::Done
        }
        Ok(s) => {
            tracing::warn!(dir=%dir.display(), code=?s.code(), "termux-media-scan non-zero");
            ScanOutcome::NonZero(s.code())
        }
        Err(e) => {
            tracing::warn!(dir=%dir.display(), error=%e, "termux-media-scan failed");
            ScanOutcome::Failed(e.to_string())
        }
    }
}
