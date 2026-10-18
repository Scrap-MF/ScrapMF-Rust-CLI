use std::ffi::OsString;
use std::process::{Command, Output};

use crate::error::ScrapmfError;

/// Stand-in for an empty stderr on a failed backend run.
///
/// The exit code is already printed by [`ScrapmfError::BackendFailed`], so
/// repeating it inside the stderr field only produced the doubled
/// `exit code 4 / stderr: exit code 4` a failed scrape used to show.
pub(crate) const NO_STDERR: &str = "(no output)";

/// Spawn a command, transparently retrying transient `ETXTBSY` failures.
///
/// Under heavy parallel test execution, `exec` on a file written moments ago
/// can return `Text file busy` (os error 26) — a known kernel-side race
/// (see rust-lang/rust#888). Production never execs freshly-written files,
/// so the retry loop is compiled only for tests.
fn spawn_with_retry(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    #[cfg(test)]
    {
        const ETXTBSY: i32 = 26;
        let mut attempts = 0;
        loop {
            match cmd.spawn() {
                Err(e) if e.raw_os_error() == Some(ETXTBSY) && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                other => return other,
            }
        }
    }
    #[cfg(not(test))]
    {
        cmd.spawn()
    }
}

/// Like [`spawn_with_retry`] but for `.status()` / `.output()` style calls.
/// Runs `op` and retries transient `ETXTBSY` errors (tests only).
fn io_with_retry<T, F>(mut op: F) -> std::io::Result<T>
where
    F: FnMut() -> std::io::Result<T>,
{
    #[cfg(test)]
    {
        const ETXTBSY: i32 = 26;
        let mut attempts = 0;
        loop {
            match op() {
                Err(e) if e.raw_os_error() == Some(ETXTBSY) && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                other => return other,
            }
        }
    }
    #[cfg(not(test))]
    {
        op()
    }
}

pub struct Executor;

impl Executor {
    /// Find binary in $PATH via `which` crate — cached to avoid repeated
    /// `stat` per `PATH` entry for every sub-URL (5+ calls per job).
    pub fn find_binary(name: &str) -> Option<std::path::PathBuf> {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
        // Bypass cache in tests so temp binaries created mid-test are found
        if cfg!(test) {
            return which::which(name).ok();
        }
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(guard) = cache.lock()
            && let Some(cached) = guard.get(name)
        {
            return cached.clone();
        }
        let res = which::which(name).ok();
        if let Ok(mut guard) = cache.lock() {
            guard.insert(name.to_string(), res.clone());
        }
        res
    }

    /// Run with checked errors: maps NotFound to BackendNotFound with help, and
    /// non-zero to BackendFailed.
    ///
    /// stdout is inherited so scripts and CI keep seeing the URL list or the
    /// download progress live. stderr is **captured** rather than inherited:
    /// the error it carries is the only input to `classify_failure`, and with
    /// stderr inherited the classifier saw just "exit code 4" — so a 429 was
    /// never retried, a private account was never skipped, and a "not found"
    /// message could not even be recognised well enough to name the session it
    /// came from.
    pub fn run_inherited_checked(
        binary: &str,
        args: &[OsString],
    ) -> Result<std::process::ExitStatus, ScrapmfError> {
        let found = Self::find_binary(binary);
        if found.is_none() {
            return Err(ScrapmfError::BackendNotFound {
                name: binary.to_string(),
            });
        }
        let out = io_with_retry(|| {
            Command::new(binary)
                .args(args)
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::piped())
                .output()
        })
        .map_err(ScrapmfError::Io)?;
        let status = out.status;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail = stderr
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            let tail = if tail.trim().is_empty() {
                NO_STDERR.to_string()
            } else {
                tail
            };
            tracing::error!(binary = %binary, code = %crate::error::exit_code_label(status.code()), stderr = %tail, "backend failed");
            return Err(ScrapmfError::BackendFailed {
                name: binary.to_string(),
                code: status.code(),
                stderr: tail,
            });
        }
        Ok(status)
    }

    /// Run with stdout/stderr CAPTURED and streamed line-by-line to callbacks.
    ///
    /// - `on_stdout`: one call per line (gallery-dl prints one line per
    ///   downloaded/skipped file) — used to drive progress counters.
    /// - `on_stderr`: one call per line (warnings/errors like JS challenges).
    ///
    /// A tail of the last ~40 stderr lines is kept for error reporting.
    /// stderr is drained on a separate thread to avoid pipe deadlocks.
    ///
    /// `abort`: when set to true between reads, the child process is killed
    /// and [`ScrapmfError::Aborted`] is returned.
    pub fn run_streaming(
        binary: &str,
        args: &[OsString],
        abort: &std::sync::atomic::AtomicBool,
        on_stdout_line: &mut dyn FnMut(&str),
        on_stderr_line: &mut dyn FnMut(&str),
    ) -> Result<std::process::ExitStatus, ScrapmfError> {
        use std::io::{BufRead, BufReader};
        use std::sync::atomic::Ordering;
        use std::sync::mpsc::channel;
        use std::thread;

        if Self::find_binary(binary).is_none() {
            return Err(ScrapmfError::BackendNotFound {
                name: binary.to_string(),
            });
        }

        let mut child = spawn_with_retry(
            Command::new(binary)
                .args(args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()),
        )
        .map_err(ScrapmfError::Io)?;

        // Drain stderr on a helper thread; forward lines back via channel.
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ScrapmfError::Io(std::io::Error::other("child stderr not captured")))?;
        let (tx_err, rx_err) = channel::<String>();
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx_err.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Drain stdout on a helper thread too: gallery-dl only prints one
        // stdout line per COMPLETED file, so reading stdout inline would block
        // the main loop for minutes during large downloads and make the abort
        // flag unresponsive (Ctrl+C appeared dead). With both streams on
        // channels, the main loop can check `abort` every ~100ms.
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ScrapmfError::Io(std::io::Error::other("child stdout not captured")))?;
        let (tx_out, rx_out) = channel::<String>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx_out.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Shared stderr tail for the failure summary (built from forwarded lines).
        let mut stderr_tail: std::collections::VecDeque<String> =
            std::collections::VecDeque::with_capacity(40);

        loop {
            // Abort requested → kill child, drain stderr, report Aborted
            if abort.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                while let Ok(line) = rx_err.try_recv() {
                    on_stderr_line(&line);
                }
                return Err(ScrapmfError::Aborted);
            }
            // Drain any stderr lines that arrived.
            while let Ok(line) = rx_err.try_recv() {
                on_stderr_line(&line);
                if stderr_tail.len() == 40 {
                    stderr_tail.pop_front();
                }
                stderr_tail.push_back(line);
            }
            // Wait up to 100ms for a stdout line. The bounded wait is what
            // keeps the abort check responsive during long silent downloads.
            match rx_out.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(next) => {
                    let trimmed = next.trim_end_matches(['\n', '\r']);
                    if !trimmed.is_empty() {
                        on_stdout_line(trimmed);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // stdout EOF — drain stderr until the reader thread closes
                    // its channel (its pipe hit EOF), with a hard 5s safety
                    // deadline in case an orphaned grandchild keeps the pipe
                    // open.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    loop {
                        match rx_err.recv_timeout(std::time::Duration::from_millis(100)) {
                            Ok(line) => {
                                on_stderr_line(&line);
                                if stderr_tail.len() == 40 {
                                    stderr_tail.pop_front();
                                }
                                stderr_tail.push_back(line);
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                if std::time::Instant::now() >= deadline {
                                    tracing::warn!(
                                        "stderr reader did not close within 5s; draining stopped"
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }

        let status = child.wait().map_err(ScrapmfError::Io)?;
        if !status.success() {
            tracing::error!(binary = %binary, code = %crate::error::exit_code_label(status.code()), "backend failed");
            let tail = stderr_tail
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let stderr_summary = if tail.trim().is_empty() {
                NO_STDERR.to_string()
            } else {
                tail
            };
            return Err(ScrapmfError::BackendFailed {
                name: binary.to_string(),
                code: status.code(),
                stderr: stderr_summary,
            });
        }
        Ok(status)
    }

    /// Run capturing output for --version etc., with truncating stderr.
    pub fn run_capturing(binary: &str, args: &[OsString]) -> Result<Output, ScrapmfError> {
        let found = Self::find_binary(binary);
        if found.is_none() {
            return Err(ScrapmfError::BackendNotFound {
                name: binary.to_string(),
            });
        }
        let output =
            io_with_retry(|| Command::new(binary).args(args).output()).map_err(ScrapmfError::Io)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let truncated = stderr
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            tracing::error!(binary = %binary, code = %crate::error::exit_code_label(output.status.code()), stderr = %truncated, "backend failed");
            let stderr = if truncated.trim().is_empty() {
                NO_STDERR.to_string()
            } else {
                truncated
            };
            return Err(ScrapmfError::BackendFailed {
                name: binary.to_string(),
                code: output.status.code(),
                stderr,
            });
        }
        Ok(output)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::Executor;
    use std::ffi::OsString;

    #[test]
    fn find_binary_existing() {
        // Should find sh which exists on Linux
        assert!(Executor::find_binary("sh").is_some());
    }

    #[test]
    fn find_binary_not_found() {
        assert!(Executor::find_binary("definitely-not-a-binary-12345").is_none());
    }

    #[test]
    fn run_capturing_not_found() {
        let res = Executor::run_capturing("definitely-not-a-binary-12345", &[]);
        assert!(res.is_err());
        assert!(format!("{}", res.unwrap_err()).contains("not found"));
    }

    #[test]
    fn run_inherited_checked_not_found() {
        let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let res = Executor::run_streaming(
            "definitely-not-a-binary-12345",
            &[],
            &never,
            &mut |_| {},
            &mut |_| {},
        );
        assert!(res.is_err());
    }

    fn write_script(dir: &tempfile::TempDir, name: &str, body: &str) -> OsString {
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&path).expect("meta").permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&path, perm).expect("chmod");
        }
        path.into_os_string()
    }

    #[test]
    fn run_streaming_forwards_stdout_and_stderr_lines() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let bin = write_script(
            &dir,
            "fake_ok",
            "#!/bin/sh\necho file1.jpg\necho file2.jpg\necho warn-abc >&2\necho warn-def >&2\n",
        );
        let mut stdout_lines = Vec::new();
        let mut stderr_lines = Vec::new();
        let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let status = Executor::run_streaming(
            bin.to_str().unwrap(),
            &[],
            &never,
            &mut |l: &str| stdout_lines.push(l.to_string()),
            &mut |l: &str| stderr_lines.push(l.to_string()),
        )
        .expect("ok status");
        assert!(status.success());
        assert_eq!(stdout_lines, vec!["file1.jpg", "file2.jpg"]);
        assert_eq!(stderr_lines, vec!["warn-abc", "warn-def"]);
    }

    #[test]
    fn run_streaming_failed_binary_reports_stderr_tail() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let bin = write_script(
            &dir,
            "fake_fail",
            "#!/bin/sh\necho boom-detail >&2\nexit 3\n",
        );
        let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let res =
            Executor::run_streaming(bin.to_str().unwrap(), &[], &never, &mut |_| {}, &mut |_| {});
        let err = res.expect_err("should fail");
        let msg = err.to_string();
        assert!(msg.contains("boom-detail"), "stderr tail missing: {msg}");
    }

    /// Regression test: Ctrl+C (abort flag) must stop the run even while the
    /// backend is silent mid-download (gallery-dl prints a stdout line only
    /// per COMPLETED file). The abort must land in seconds, not after the
    /// current download finishes.
    #[test]
    fn run_streaming_abort_is_responsive_during_silent_download() {
        use crate::error::ScrapmfError;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};

        let dir = tempfile::TempDir::new().expect("tempdir");
        // Prints one line (file done), then stays silent 30s (next big video
        // downloading). With the old blocking read_line, abort was only
        // evaluated after these 30s.
        let bin = write_script(
            &dir,
            "fake_slow_download",
            "#!/bin/sh\necho first-file.mp4\nsleep 30\n",
        );
        let abort = Arc::new(AtomicBool::new(false));
        let abort2 = abort.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            abort2.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let started = Instant::now();
        let res = Executor::run_streaming(
            bin.to_str().unwrap(),
            &[],
            &abort,
            &mut |line| assert_eq!(line, "first-file.mp4"),
            &mut |_| {},
        );
        let _ = setter.join();
        let elapsed = started.elapsed();

        match res {
            Err(ScrapmfError::Aborted) => {}
            other => panic!("expected Aborted, got: {:?}", other.map(|_| ())),
        }
        assert!(
            elapsed < Duration::from_secs(5),
            "abort took {elapsed:?} — stdout handling became blocking again"
        );
    }

    #[test]
    fn mock_path_with_temp_binary_no_env() {
        // Test that find_binary works without env mutation: create fake binary and check direct which
        let dir = tempfile::TempDir::new().expect("tempdir");
        let fake_bin = dir.path().join("fake-gallery-dl-test");
        std::fs::write(&fake_bin, "#!/bin/sh\necho 1.32.17\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&fake_bin).expect("meta").permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&fake_bin, perm).expect("chmod");
        }
        // Verify file exists and is executable, but not via PATH
        assert!(fake_bin.is_file());
        // find_binary should not find it without PATH modification, which is correct
        // This test just verifies no panic on temp binary creation
        let _ = Executor::find_binary("fake-gallery-dl-test");
    }

    #[test]
    fn truncates_stderr() {
        // Simulate stderr with 30 lines, ensure truncation logic would keep 20
        let stderr = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let truncated = stderr
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(truncated.lines().count(), 20);
        assert!(truncated.contains("line 29"));
        assert!(!truncated.contains("line 0"));
    }
}
