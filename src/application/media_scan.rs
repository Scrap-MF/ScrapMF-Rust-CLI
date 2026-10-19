//! Termux MediaStore scan — best-effort, never fails a scrape.
//! Runs `termux-media-scan -r <output dir>` once per finished download.
//! Disabled by default (opt-in via Plugins → Termux MediaScan).
//!
//! Two Android facts shape everything here:
//!
//! * `termux-media-scan` only indexes files *directly* inside the path it is
//!   given unless `-r` is passed. Every download lands in
//!   `<output>/<account>/<kind>/`, so without `-r` the scan sees nothing.
//! * The scan runs inside the **termux:api** app, a different package. It can
//!   only read world-readable paths, so it can never see Termux' own private
//!   storage (`/data/data/com.termux/files/...`). Output must live under
//!   shared storage — `~/storage/downloads/…`, which is a symlink to
//!   `/storage/emulated/0/Download/…`. That is why the path is canonicalized
//!   before it is handed over.

use std::path::{Path, PathBuf};

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
    /// Target is app-private storage — a scan there would index nothing.
    PrivateStorage(PathBuf),
    /// Command ran and exited 0.
    Done,
    /// Command ran and exited non-zero.
    NonZero(Option<i32>),
    /// Command could not be spawned.
    Failed(String),
}

/// Where a scan can actually be pointed, after expansion + symlink resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanTarget {
    /// A directory MediaStore can index.
    Ready(PathBuf),
    /// The path lives in app-private storage — MediaStore cannot see it.
    PrivateStorage(PathBuf),
    /// The directory does not exist (yet).
    Missing(PathBuf),
}

/// Expand `~`/`$HOME`, resolve symlinks and decide whether MediaStore can
/// index the result.
///
/// Canonicalization is what makes `~/storage/downloads/scrapmf` work: it is a
/// symlink into `/storage/emulated/0/Download`, and the api app cannot follow
/// Termux' internal path.
pub fn resolve_target(dir: &Path) -> ScanTarget {
    let expanded = crate::config::expand_output_dir(dir);
    // The directory may legitimately not exist yet (nothing downloaded), so a
    // failed canonicalize is a verdict of its own rather than an error.
    let resolved = canonicalize_lenient(&expanded);
    if is_private_storage(&resolved) {
        ScanTarget::PrivateStorage(resolved)
    } else if resolved.is_dir() {
        ScanTarget::Ready(resolved)
    } else {
        ScanTarget::Missing(resolved)
    }
}

/// Canonicalize as much of the path as exists, then re-append the rest.
///
/// `~/storage/downloads` is a symlink, so a plain `canonicalize` on a path
/// that has not been created yet would give up and hand back the Termux-internal
/// spelling the api app cannot read. Resolving the existing ancestor keeps the
/// real prefix (`/storage/emulated/0/…`) either way.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    while let Some(name) = cursor.file_name() {
        tail.push(name.to_os_string());
        cursor = cursor.parent().unwrap_or(Path::new("/"));
        if let Ok(resolved) = std::fs::canonicalize(cursor) {
            let mut out = resolved;
            for segment in tail.iter().rev() {
                out.push(segment);
            }
            return out;
        }
    }
    path.to_path_buf()
}

/// True when Android itself cannot see the path: Termux' private home or any
/// `/data/{data,user}` app sandbox. MediaStore only indexes world-readable
/// files, so a scan aimed here exits successfully and does nothing.
pub fn is_private_storage(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if s.starts_with("/data/data/") || s.starts_with("/data/user/") {
        return true;
    }
    // On Android `$HOME` is itself app-private, so a path under it never
    // reaches the Gallery even though it looks ordinary. Gated on Termux: on
    // a desktop host `$HOME` is ordinary storage and flagging it would make
    // `doctor` cry wolf on a plugin that cannot run there anyway.
    if !crate::plugins::is_termux_env() {
        return false;
    }
    // Compared component-wise, so `/data/dataX` is not read as a child of
    // `/data/data`.
    dirs::home_dir().is_some_and(|home| path.starts_with(canonicalize_lenient(&home)))
}

/// Narrow a scan to the part of the output tree a single download touched.
///
/// Scanning the whole output dir works but grows without bound; the common
/// ancestor of the files just written is normally `<output>/<account>`, which
/// is one or two orders of magnitude smaller. Falls back to `out_dir` whenever
/// the paths do not allow a confident answer — relative paths, files outside
/// the output dir, or nothing downloaded at all.
pub fn narrow_target(out_dir: &Path, downloaded: &[String]) -> PathBuf {
    if downloaded.is_empty() {
        return out_dir.to_path_buf();
    }
    let out_canonical = canonicalize_lenient(out_dir);
    let mut ancestor: Option<PathBuf> = None;
    for file in downloaded {
        let file = Path::new(file);
        if !file.is_absolute() {
            return out_dir.to_path_buf();
        }
        let dir = match file.parent() {
            Some(d) => d,
            // A bare `/` can only come from a provider path we do not trust.
            None => return out_dir.to_path_buf(),
        };
        let dir = canonicalize_lenient(dir);
        if !dir.starts_with(&out_canonical) {
            return out_dir.to_path_buf();
        }
        ancestor = Some(match ancestor {
            Some(current) => common_ancestor(&current, &dir),
            None => dir,
        });
    }
    match ancestor {
        // Never hand back the filesystem root: if the common ancestor climbed
        // out of the output dir, the download tree is not what we think it is.
        Some(p) if p.starts_with(&out_canonical) && p != out_canonical => p,
        _ => out_dir.to_path_buf(),
    }
}

/// Longest path prefix shared by both inputs.
fn common_ancestor(a: &Path, b: &Path) -> PathBuf {
    a.components()
        .zip(b.components())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect()
}

/// Single dir scan (1 command, prioritizes speed on Termux).
/// Best-effort: missing binary / non-Termux → silent no-op with tracing.
///
/// `downloaded`, when given, narrows the scan to just the files this run
/// produced; pass `None` to scan the whole output dir (menu "Test scan").
pub fn maybe_scan(dir: &Path, downloaded: Option<&[String]>) -> ScanOutcome {
    if !crate::plugins::termux_scan_enabled() {
        return ScanOutcome::Disabled;
    }
    let bin = std::env::var_os("SCRAPMF_TERMUX_SCAN_BIN")
        .map(std::path::PathBuf::from)
        .or_else(|| which::which("termux-media-scan").ok());
    let Some(bin) = bin else {
        crate::output::print_warn(
            "termux-media-scan not found — install the termux:api app + pkg install termux-api",
        );
        tracing::warn!(
            "termux-scan enabled but termux-media-scan not found — install termux:api app + pkg install termux-api"
        );
        return ScanOutcome::MissingBinary;
    };
    let expanded = crate::config::expand_output_dir(dir);
    let target = match downloaded {
        Some(files) => narrow_target(&expanded, files),
        None => expanded,
    };
    let target = match resolve_target(&target) {
        ScanTarget::Ready(p) => p,
        ScanTarget::PrivateStorage(p) => {
            let hint = p
                .file_name()
                .map(|n| format!("$HOME/storage/downloads/{}", n.to_string_lossy()))
                .unwrap_or_else(|| "$HOME/storage/downloads/scrapmf".to_string());
            crate::output::print_error(&format!(
                "Android Gallery cannot index app-private storage: {}",
                p.display()
            ));
            crate::output::print_note(&format!("set output_dir to {hint}"));
            tracing::warn!(dir=%p.display(), "termux scan skipped: app-private storage is invisible to MediaStore");
            return ScanOutcome::PrivateStorage(p);
        }
        ScanTarget::Missing(p) => {
            crate::output::print_error(&format!("output dir does not exist yet: {}", p.display()));
            tracing::debug!(dir=%p.display(), "termux scan skipped: dir does not exist yet");
            return ScanOutcome::MissingDir;
        }
    };

    crate::output::print_info(&format!(
        "→ running termux-media-scan -r {} (this can take a few seconds)",
        target.display()
    ));
    run_scan(&bin, &target)
}

/// Invoke the scanner with the flags it needs.
///
/// `-r` is not optional: MediaStore only indexes files sitting directly in the
/// given path, and every download lands in `<output>/<account>/<kind>/`.
fn run_scan(bin: &Path, target: &Path) -> ScanOutcome {
    match std::process::Command::new(bin)
        .arg("-r")
        .arg(target)
        .status()
    {
        Ok(s) if s.success() => {
            tracing::info!(dir=%target.display(), "termux media scan done");
            crate::output::print_info("✔ media scan done");
            ScanOutcome::Done
        }
        Ok(s) => {
            tracing::warn!(dir=%target.display(), code=?s.code(), "termux-media-scan non-zero");
            crate::output::print_error(&format!(
                "termux-media-scan exited with code {:?}",
                s.code()
            ));
            ScanOutcome::NonZero(s.code())
        }
        Err(e) => {
            tracing::warn!(dir=%target.display(), error=%e, "termux-media-scan failed");
            crate::output::print_error(&format!("termux-media-scan failed: {e}"));
            ScanOutcome::Failed(e.to_string())
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// Fresh directory per test and per process: the fixture scanner writes its
    /// recorded args to a file inside one, so a stale directory left by an
    /// earlier run would make the assertion read someone else's output.
    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrapmf_scan_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test setup");
        dir
    }

    /// The whole point of canonicalizing: `~/storage/downloads` is a symlink
    /// and the api app can only index the real path behind it.
    #[test]
    fn resolve_target_follows_symlinks() {
        let real = tmpdir("real");
        let link_parent = tmpdir("link_parent");
        let link = link_parent.join("downloads");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        std::fs::create_dir_all(real.join("scrapmf")).expect("mkdir");

        match resolve_target(&link.join("scrapmf")) {
            ScanTarget::Ready(p) => assert_eq!(p, real.join("scrapmf")),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    /// Even when the leaf does not exist yet, the symlinked prefix has to be
    /// resolved — otherwise the scanner would be handed the Termux-internal
    /// spelling it cannot read.
    #[test]
    fn resolve_target_resolves_the_prefix_of_a_missing_directory() {
        let real = tmpdir("prefix_real");
        let link = tmpdir("prefix_link").join("downloads");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        match resolve_target(&link.join("scrapmf")) {
            ScanTarget::Missing(p) => assert_eq!(p, real.join("scrapmf")),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn resolve_target_reports_missing_directory() {
        let base = tmpdir("missing");
        let gone = base.join("nope");
        assert_eq!(resolve_target(&gone), ScanTarget::Missing(gone));
    }

    #[test]
    fn private_storage_is_recognised() {
        assert!(is_private_storage(Path::new(
            "/data/data/com.termux/files/home/x"
        )));
        assert!(is_private_storage(Path::new(
            "/data/user/0/com.termux/files/home"
        )));
        // A sibling that merely shares the prefix is not a child.
        assert!(!is_private_storage(Path::new("/data/database/x")));
        assert!(!is_private_storage(Path::new(
            "/storage/emulated/0/Download/scrapmf"
        )));
    }

    #[test]
    fn narrow_picks_the_account_directory() {
        let out = tmpdir("narrow");
        let files = vec![
            out.join("alice/posts/1.jpg").display().to_string(),
            out.join("alice/posts/2.jpg").display().to_string(),
            out.join("alice/threads/3.jpg").display().to_string(),
        ];
        assert_eq!(narrow_target(&out, &files), out.join("alice"));
    }

    #[test]
    fn narrow_single_file_keeps_its_directory() {
        let out = tmpdir("narrow_one");
        std::fs::create_dir_all(out.join("alice/posts")).expect("test setup");
        let files = vec![out.join("alice/posts/1.jpg").display().to_string()];
        assert_eq!(narrow_target(&out, &files), out.join("alice/posts"));
    }

    #[test]
    fn narrow_falls_back_when_paths_are_unusable() {
        let out = tmpdir("narrow_fallback");
        // No downloads at all.
        assert_eq!(narrow_target(&out, &[]), out);
        // Relative path: provider paths we do not trust.
        assert_eq!(narrow_target(&out, &["posts/1.jpg".to_string()]), out);
        // Absolute but outside the output dir.
        assert_eq!(
            narrow_target(&out, &["/tmp/elsewhere/1.jpg".to_string()]),
            out
        );
    }

    #[test]
    fn narrow_never_climbs_above_the_output_dir() {
        let out = tmpdir("narrow_root");
        // Two accounts: the common ancestor is the output dir itself.
        let files = vec![
            out.join("alice/1.jpg").display().to_string(),
            out.join("bob/1.jpg").display().to_string(),
        ];
        assert_eq!(narrow_target(&out, &files), out);
    }

    #[test]
    fn common_ancestor_stops_at_the_first_divergence() {
        assert_eq!(
            common_ancestor(Path::new("/a/b/c"), Path::new("/a/b/d")),
            PathBuf::from("/a/b")
        );
        assert_eq!(
            common_ancestor(Path::new("/a"), Path::new("/b")),
            PathBuf::from("/")
        );
    }

    /// A stand-in for `termux-media-scan` that records the arguments it was
    /// given. Exercises `run_scan` directly: the binary path is resolved from the
    /// environment and the plugin toggle from the config file, and this crate
    /// forbids `unsafe`, so the env cannot be pointed at a fixture from a test.
    fn fake_scanner(name: &str, exit_code: i32) -> (PathBuf, PathBuf) {
        let args_file = tmpdir(&format!("{name}_args")).join("args.txt");
        let script = tmpdir(&format!("{name}_bin")).join("fake-scan.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nexit {exit_code}\n",
                args_file.display()
            ),
        )
        .expect("test setup");
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("test setup");
        (script, args_file)
    }

    fn recorded_args(args_file: &Path) -> Vec<String> {
        std::fs::read_to_string(args_file)
            .expect("args recorded")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The regression that made every scan a no-op: without `-r`, MediaStore only
    /// sees files sitting directly in the given directory, and every download lives
    /// in `<output>/<account>/<kind>/`.
    #[test]
    fn scan_is_recursive_and_aimed_at_the_resolved_path() {
        let real = tmpdir("run_flag_real");
        let link_parent = tmpdir("run_flag_link");
        let link = link_parent.join("downloads");
        std::os::unix::fs::symlink(&real, &link).expect("test setup");
        let target = link.join("alice");
        std::fs::create_dir_all(&target).expect("test setup");

        let (script, args_file) = fake_scanner("run_flag", 0);
        // The path handed to the scanner must be the symlink-resolved one, since
        // the api app cannot follow Termux' internal `~/storage/...` path.
        let resolved = crate::application::media_scan::resolve_target(&target);

        assert_eq!(
            run_scan(&script, &resolved_path(&resolved)),
            ScanOutcome::Done
        );

        let args = recorded_args(&args_file);
        assert_eq!(args[0], "-r", "scan must be recursive");
        assert_eq!(
            PathBuf::from(&args[1]),
            target.canonicalize().expect("target exists"),
            "scan must target the symlink-resolved path"
        );
    }

    #[test]
    fn scan_reports_a_non_zero_exit() {
        let (script, _) = fake_scanner("run_fail", 3);
        let target = tmpdir("run_fail_target");
        assert_eq!(run_scan(&script, &target), ScanOutcome::NonZero(Some(3)));
    }

    /// Extracts the directory a resolved target points at, failing loudly if the
    /// resolution said the path is unusable.
    fn resolved_path(target: &ScanTarget) -> PathBuf {
        match target {
            ScanTarget::Ready(p) | ScanTarget::Missing(p) | ScanTarget::PrivateStorage(p) => {
                p.clone()
            }
        }
    }
}
