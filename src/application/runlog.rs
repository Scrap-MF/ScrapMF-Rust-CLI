//! Persistent per-run gallery-dl logs.
//!
//! The ratatui dashboard renders gallery-dl output on the alternate screen,
//! which terminals cannot copy from, and discards it when the run ends.
//! Every scrape therefore mirrors its stdout/stderr lines into a plain text
//! file under `$XDG_STATE_HOME/scrapmf/logs/` so failures stay inspectable:
//!
//! ```text
//! ~/.local/state/scrapmf/logs/20260823-210530-instagram-sample_user.log
//! ```
//!
//! Logging is best-effort by design: any IO failure disables the log for
//! that job and the scrape proceeds — a broken state directory must never
//! break a download. Paths are printed next to the failure summary so users
//! can open/copy them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default number of log files kept per directory (older ones pruned).
const KEEP_LOGS: usize = 20;

pub struct RunLog {
    file: Option<std::fs::File>,
    pub path: PathBuf,
}

impl RunLog {
    /// Open a log for one scrape job under `<state_dir>/<name>.log`.
    ///
    /// `dir_override` exists for tests; production callers pass [`logs_dir`].
    pub fn open_at(dir: &Path, site: &str, user: &str) -> Self {
        let name = format!(
            "{}-{}-{}.log",
            timestamp_compact(),
            sanitize_component(site),
            sanitize_component(user)
        );
        let path = dir.join(name);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        if file.is_some() {
            crate::config::restrict_perms(&path, false);
        }
        RunLog { file, path }
    }

    /// Production entry point: resolve/create the XDG state logs directory
    /// and prune old logs (keeping [`KEEP_LOGS`] newest).
    pub fn open(site: &str, user: &str) -> Self {
        match logs_dir() {
            Some(dir) => {
                let _ = std::fs::create_dir_all(&dir);
                crate::config::restrict_perms(&dir, true);
                let log = Self::open_at(&dir, site, user);
                let _ = Self::prune(&dir, KEEP_LOGS);
                log
            }
            None => RunLog {
                file: None,
                path: PathBuf::new(),
            },
        }
    }

    /// Mirror one gallery-dl output line (stdout or stderr).
    pub fn line(&mut self, raw: &str) {
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{raw}");
        }
    }

    /// Write the final status summary and flush.
    pub fn finish(&mut self, status: &str) {
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "--- scrapmf: {status} ---");
            let _ = f.flush();
        }
    }

    /// Remove all but the newest `keep` `*.log` files in `dir`.
    /// Filenames start with the compact timestamp, so lexicographic order
    /// equals chronological order.
    pub fn prune(dir: &Path, keep: usize) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut logs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "log"))
            .collect();
        if logs.len() <= keep {
            return 0;
        }
        logs.sort();
        let excess = logs.len() - keep;
        for path in &logs[..excess] {
            let _ = std::fs::remove_file(path);
        }
        excess
    }
}

/// XDG state logs directory: `$XDG_STATE_HOME/scrapmf/logs`
/// (default `~/.local/state/scrapmf/logs`). `None` when even the home dir
/// cannot be resolved — callers then run without a persistent log.
pub fn logs_dir() -> Option<PathBuf> {
    if let Some(state) = dirs::state_dir() {
        return Some(state.join("scrapmf").join("logs"));
    }
    dirs::home_dir().map(|h| h.join(".local").join("state").join("scrapmf").join("logs"))
}

/// Filesystem-safe name component: keep alphanumerics, `-`, `_`, `.`;
/// everything else (spaces, `/`, `:` …) collapses to `_`; capped at 32 chars.
fn sanitize_component(s: &str) -> String {
    crate::util::sanitize_component_with_dot(s, 32, "job")
}

/// `YYYYmmdd-HHMMSS` in UTC from the system clock (no external crates).
/// Days-from-civil algorithm (Howard Hinnant) inverted for date extraction.
fn timestamp_compact() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{rem_h:02}{rem_min:02}{rem_sec:02}",
        rem_h = rem / 3600,
        rem_min = (rem % 3600) / 60,
        rem_sec = rem % 60
    )
}

/// Canonical status line for a finished scrape (used in logs and summaries).
pub fn status_line(outcome: &anyhow::Result<crate::application::scraper::ScrapeOutcome>) -> String {
    match outcome {
        Ok(o) => format!(
            "done — {} sub-extractor(s), {} skipped, {} challenge-lost",
            o.success_count,
            o.skipped.len(),
            o.challenge_failures
        ),
        Err(e) => format!("failed: {e}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_hostile_components() {
        assert_eq!(sanitize_component("sample_user"), "sample_user");
        assert_eq!(
            sanitize_component("weird:name/with spaces"),
            "weird_name_with_spaces"
        );
        assert_eq!(sanitize_component(""), "job");
        assert_eq!(sanitize_component("x").len(), 1);
        // 40 identical chars cap at 32
        assert_eq!(sanitize_component(&"a".repeat(40)).len(), 32);
    }

    #[test]
    fn writes_lines_and_finish_marker() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut log = RunLog::open_at(dir.path(), "instagram", "user");
        log.line("[instagram][error] NotFoundError: Requested user could not be found");
        log.finish("failed: exit code 1");
        drop(log);

        let content = std::fs::read_to_string(
            std::fs::read_dir(dir.path())
                .expect("rd")
                .flatten()
                .next()
                .expect("entry")
                .path(),
        )
        .expect("read");
        assert!(content.contains("NotFoundError"));
        assert!(content.contains("--- scrapmf: failed: exit code 1 ---"));
    }

    #[test]
    fn disabled_log_silently_drops_lines() {
        let mut log = RunLog {
            file: None,
            path: PathBuf::new(),
        };
        log.line("ignored"); // must not panic
        log.finish("ignored");
    }

    #[test]
    fn prune_keeps_newest_files() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        for i in 0..25 {
            // zero-padded names: lexicographic == chronological
            std::fs::write(dir.path().join(format!("{i:04}.log")), "x").expect("seed");
        }
        let removed = RunLog::prune(dir.path(), 20);
        assert_eq!(removed, 5);
        let left = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(left, 20);
        // oldest (lowest index) gone, newest kept
        assert!(!dir.path().join("0000.log").exists());
        assert!(dir.path().join("0024.log").exists());
    }

    #[test]
    fn timestamp_shape_is_stable() {
        let ts = timestamp_compact();
        // YYYYmmdd-HHMMSS = 8 + 1 + 6 chars
        assert_eq!(ts.len(), 15);
        assert_eq!(ts.as_bytes()[8], b'-');
    }
}
