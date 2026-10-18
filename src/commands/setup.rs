use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::Digest;

use crate::application::backend::{self, GALLERY_DL_PIN};
use crate::{config, output};

/// `scrapmf setup` — install the bundled pinned gallery-dl.
///
/// The managed copy lives in `$XDG_DATA_HOME/scrapmf/bin/` and is frozen at
/// [`GALLERY_DL_PIN`]: it never updates on its own and never touches the
/// user's system gallery-dl. Upgrades happen only by upgrading scrapmf (which
/// ships a new pin) and re-running this command.
pub fn run(yes: bool) -> Result<()> {
    let source = backend::resolve(
        config::load()
            .unwrap_or_default()
            .backend
            .gallery_dl_path
            .clone(),
    );
    if let backend::Source::Managed(p) = &source {
        let installed = installed_version(p);
        if installed.as_deref() == Some(GALLERY_DL_PIN) {
            output::print_success(&format!(
                "bundled gallery-dl v{GALLERY_DL_PIN} already installed at {}",
                p.display()
            ));
            return Ok(());
        }
        // The file exists but is not the pinned build. Previously this branch
        // returned unconditionally, so a new pin could never reach anyone who
        // had already installed a backend once: `setup` reported "already
        // installed" while the binary on disk was the old version. A user who
        // bumped the pin got a scraper still wired to a gallery-dl too old for
        // the site's current API, with no error anywhere to say so.
        let (installed_str, kind) = match installed {
            Some(v) => (v, "older"),
            None => ("unreadable".to_string(), "unknown"),
        };
        output::print_info(&format!(
            "installed gallery-dl is {installed_str}, pinned build is {GALLERY_DL_PIN}"
        ));
        output::print_info(&format!(
            "replacing the managed copy ({kind} than the pin) — the pin follows \
             scrapmf's release, so this is expected after an update"
        ));
    }
    output::print_info(&format!(
        "Current resolution: {}",
        match &source {
            backend::Source::Missing => "no gallery-dl found".to_string(),
            s => format!(
                "{} ({})",
                s.path().unwrap_or(std::path::Path::new("?")).display(),
                s.label()
            ),
        }
    ));

    // x86_64 has an official standalone build → direct managed install.
    // Every other desktop arch has none (Termux/ARM handled below); Windows
    // x86_64 DOES have one (gallery-dl.exe).
    if std::env::consts::ARCH == "x86_64" || cfg!(windows) {
        install_managed(yes)
    } else {
        install_via_python(yes);
        Ok(())
    }
}

/// One candidate installer for the pinned gallery-dl on non-x86_64 hosts.
struct PipInstaller {
    /// Display name for messages.
    label: &'static str,
    /// argv[] used to invoke it (binary + leading args before "install").
    argv_prefix: &'static [&'static str],
}

/// Detect usable installers, best first: pipx keeps the version frozen in an
/// isolated env; python3 -m pip is the universal fallback (Termux, venvs…).
fn detect_installers() -> Vec<PipInstaller> {
    let mut out = Vec::new();
    if which::which("pipx").is_ok() {
        out.push(PipInstaller {
            label: "pipx",
            argv_prefix: &["pipx"],
        });
    }
    let pip_ok = ["python3", "python"]
        .iter()
        .any(|py| which::which(py).is_ok());
    if pip_ok {
        out.push(PipInstaller {
            label: "pip",
            argv_prefix: &["python3", "-m", "pip"],
        });
    }
    out
}

fn is_termux() -> bool {
    std::env::var_os("TERMUX_VERSION").is_some()
        || std::env::var_os("PREFIX").is_some_and(|p| p.to_string_lossy().contains("com.termux"))
}

/// Offer and run a pinned gallery-dl install through pipx/pip. The installed
/// binary lands on $PATH, so backend resolution picks it up as
/// Source::System ("NOT pinned" — the user must resist upgrading it).
fn install_via_python(yes: bool) {
    let pin = backend::GALLERY_DL_PIN;
    let candidates = detect_installers();
    if candidates.is_empty() {
        print_python_fallback();
        return;
    }

    output::print_info(&format!(
        "no standalone build for {} — installing pinned gallery-dl via Python instead",
        std::env::consts::ARCH
    ));
    let choice = &candidates[0];
    let mut cmd = String::new();
    for (i, part) in choice.argv_prefix.iter().enumerate() {
        if i > 0 {
            cmd.push(' ');
        }
        cmd.push_str(part);
    }
    cmd.push_str(&format!(" install gallery-dl=={pin}"));
    println!("→ {cmd}");

    if !yes && !confirm("Run this now?") {
        println!("cancelled — run it manually whenever you like");
        return;
    }

    let bin = choice.argv_prefix[0];
    let mut argv: Vec<std::ffi::OsString> = choice.argv_prefix[1..]
        .iter()
        .map(std::ffi::OsString::from)
        .collect();
    argv.extend([
        std::ffi::OsString::from("install"),
        std::ffi::OsString::from(format!("gallery-dl=={pin}")),
    ]);
    match crate::process::Executor::run_capturing(bin, &argv) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.lines().rev().take(10).collect::<Vec<_>>();
            let stderr = stderr.into_iter().rev().collect::<Vec<_>>().join("\n");
            output::print_error(&format!(
                "{} install failed:\n{}",
                choice.label,
                stderr.trim()
            ));
            print_python_fallback();
            return;
        }
        Err(e) => {
            output::print_error(&format!("could not launch {}: {e}", choice.label));
            print_python_fallback();
            return;
        }
    }
    verify_system_install();
}

/// Post-install sanity check: the binary must be on $PATH now.
fn verify_system_install() {
    match crate::application::backend::resolve(None) {
        source @ (crate::application::backend::Source::System(_)
        | crate::application::backend::Source::Env(_)) => {
            let Some(path) = source.path() else {
                return;
            };
            match crate::process::Executor::run_capturing(
                &path.to_string_lossy(),
                &[std::ffi::OsString::from("--version")],
            ) {
                Ok(out) => {
                    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    output::print_success(&format!(
                        "gallery-dl {v} verified working at {}",
                        path.display()
                    ));
                }
                Err(e) => tracing::warn!(error = %e, "installed binary did not report a version"),
            }
            output::print_note(
                "installed via pip/pipx: scrapmf sees it as system (NOT pinned). \
                 Avoid upgrading it manually; new pins ship with scrapmf releases.",
            );
        }
        _ => {
            output::print_note(
                "install finished but 'gallery-dl' is still not on $PATH — \
                 open a new shell or add its bin dir to PATH.",
            );
        }
    }
}

fn print_python_fallback() {
    let pin = backend::GALLERY_DL_PIN;
    if is_termux() {
        output::print_note(
            "no pip/pipx found — on Termux install Python first, then the pinned backend:",
        );
        println!("  pkg install python");
        println!("  python3 -m pip install gallery-dl=={pin}");
    } else {
        output::print_note(
            "no pipx or python3 found — install one of them, then the pinned version:",
        );
        println!("  pipx install gallery-dl=={pin}");
        println!("  # or:");
        println!("  python3 -m pip install gallery-dl=={pin}");
    }
    output::print_info("  pipx never auto-upgrades it, so the version stays frozen.");
}

fn install_managed(yes: bool) -> Result<()> {
    let dir = backend::managed_dir().context("cannot determine data directory")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let asset = backend::gallery_dl_asset();
    let url = format!("{}/{}", backend::release_base_url(), asset);
    println!("→ Installing bundled gallery-dl v{GALLERY_DL_PIN} (~23 MB download)");
    if !yes
        && !confirm(&format!(
            "Download and install gallery-dl v{GALLERY_DL_PIN} now?"
        ))
    {
        println!("cancelled");
        return Ok(());
    }

    let work = dir.join(format!(".setup-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let asset_path = work.join(asset);
    let sums_path = work.join("SHA256SUMS");

    fetch(&asset_path, &url)?;
    fetch(
        &sums_path,
        &format!("{}/SHA256SUMS", backend::release_base_url()),
    )?;

    echo_step("Verifying SHA256 checksum");
    let sums = std::fs::read_to_string(&sums_path).context("read SHA256SUMS")?;
    let expected = backend::extract_sha256_for(&sums, asset).context(format!(
        "{asset} not listed in official SHA256SUMS — aborting"
    ))?;
    verify_sha256(&asset_path, &expected)?;

    let target = dir.join(backend::managed_binary_name());
    make_executable(&asset_path)?;
    std::fs::rename(&asset_path, &target)
        .with_context(|| format!("move into {}", target.display()))?;
    let _ = std::fs::remove_dir_all(&work);

    output::print_success(&format!(
        "bundled gallery-dl v{GALLERY_DL_PIN} ready at {}",
        target.display()
    ));
    output::print_info("  It never updates on its own; new pins ship with scrapmf releases.");
    print_version(&target);
    Ok(())
}

/// Version string reported by an installed gallery-dl binary, if it answers.
///
/// `None` means "could not ask" — a missing file, a binary that will not run,
/// or output that does not start with a version-looking token. Callers treat
/// that as "not the pinned build" and reinstall, which is the safe direction:
/// a failed probe must never freeze a stale backend in place.
fn installed_version(path: &Path) -> Option<String> {
    let out = crate::process::Executor::run_capturing(
        &path.to_string_lossy(),
        &[std::ffi::OsString::from("--version")],
    )
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // "1.32.9" or "1.32.9\n..." — take the first whitespace-delimited token.
    let token = text.split_whitespace().next()?.to_string();
    token
        .starts_with(|c: char| c.is_ascii_digit())
        .then_some(token)
}

fn print_version(path: &Path) {
    match crate::process::Executor::run_capturing(
        &path.to_string_lossy(),
        &[std::ffi::OsString::from("--version")],
    ) {
        Ok(out) => {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            output::print_success(&format!("gallery-dl {v} verified working"));
        }
        Err(e) => tracing::warn!(error = %e, "could not verify installed binary"),
    }
}

/// First-run interactive offer: called from main before entering interactive
/// mode. Never prompts outside a TTY; respects auto_install_backends.
pub fn offer_if_missing() {
    use std::io::IsTerminal;
    let cfg = config::load().unwrap_or_default();
    if !cfg.backend.auto_install_backends || !std::io::stdin().is_terminal() {
        return;
    }
    if !matches!(
        backend::resolve(cfg.backend.gallery_dl_path.clone()),
        backend::Source::Managed(_)
    ) {
        let _ = run(false); // confirm prompt inside; cancellation is non-fatal
    }
}

// --- helpers -----------------------------------------------------------------

fn echo_step(msg: &str) {
    println!("→ {msg}");
}

fn fetch(dest: &Path, url: &str) -> Result<()> {
    echo_step(&format!("Downloading {url}"));
    let status = if command_exists("curl") {
        std::process::Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(dest)
            .arg(url)
            .status()
            .context("launch curl")?
    } else if command_exists("wget") {
        std::process::Command::new("wget")
            .arg("-qO")
            .arg(dest)
            .arg(url)
            .status()
            .context("launch wget")?
    } else {
        anyhow::bail!("curl or wget required to download the backend");
    };
    if !status.success() {
        anyhow::bail!("download failed ({url})");
    }
    Ok(())
}

/// Compute SHA-256 in Rust — no external tools needed, works on Windows too.
fn verify_sha256(file: &Path, expected_hex: &str) -> Result<()> {
    use std::io::Read;
    let mut f = std::fs::File::open(file).with_context(|| format!("open {}", file.display()))?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).context("read file for hashing")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got: String = format!("{:x}", hasher.finalize());
    if got != expected_hex.to_ascii_lowercase() {
        anyhow::bail!(
            "checksum mismatch for {}: expected {expected_hex}, got {got}",
            file.display()
        );
    }
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(path)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

fn command_exists(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

fn confirm(question: &str) -> bool {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        // Non-interactive: never escalate or download without explicit --yes
        output::print_note(&format!("{question} Use 'scrapmf setup --yes' to proceed."));
        return false;
    }
    print!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    matches!(answer.trim(), "" | "y" | "Y" | "yes" | "Yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Installer detection must always return candidates in priority order:
    /// pipx (frozen env) before plain pip. On CI/dev machines at least one of
    /// the two exists; if none does, the list is simply empty.
    #[test]
    fn detect_installers_prefers_pipx_over_pip() {
        let found = detect_installers();
        if found.len() >= 2 {
            assert_eq!(found[0].label, "pipx");
            assert_eq!(found[1].label, "pip");
        }
    }

    #[test]
    fn detect_installers_argv_ends_before_install_subcommand() {
        for c in detect_installers() {
            assert!(!c.argv_prefix.contains(&"install"));
            assert!(!c.argv_prefix.is_empty());
        }
    }

    #[test]
    fn pip_command_string_matches_detected_prefix() {
        for c in detect_installers() {
            let mut cmd = c.argv_prefix.join(" ");
            cmd.push_str(&format!(" install gallery-dl=={GALLERY_DL_PIN}"));
            assert!(cmd.starts_with(c.argv_prefix[0]));
            assert!(cmd.ends_with(GALLERY_DL_PIN));
            assert!(!cmd.contains(';'));
        }
    }
}
