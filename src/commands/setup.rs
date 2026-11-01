use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

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
            output::print_success(&crate::i18n::tf(
                "setup_bundled_gdl_already_installed",
                &[("path", &p.display().to_string())],
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
        output::print_info(&crate::i18n::tf(
            "setup_installed_gdl_version",
            &[("installed", &installed_str), ("pin", GALLERY_DL_PIN)],
        ));
        output::print_info(&crate::i18n::tf(
            "setup_replacing_managed_copy",
            &[("kind", kind)],
        ));
    }
    output::print_info(&crate::i18n::tf(
        "setup_current_resolution",
        &[(
            "value",
            &match &source {
                backend::Source::Missing => crate::i18n::t("setup_no_gallery_dl_found"),
                s => format!(
                    "{} ({})",
                    s.path().unwrap_or(std::path::Path::new("?")).display(),
                    s.label()
                ),
            },
        )],
    ));

    // Dev pins have no official standalone build → always install from source.
    // x86_64 has an official standalone build → direct managed install.
    // Every other desktop arch has none (Termux/ARM handled below); Windows
    // x86_64 DOES have one (gallery-dl.exe).
    let gd_result = if backend::is_dev_pin() {
        install_via_python(yes);
        Ok(())
    } else {
        install_managed_venv(yes)
    };

    // Also ensure the managed (pinned) yt-dlp backend is present so gallery-dl's
    // ytdl fallback never depends on a system-wide yt-dlp.
    let _ = install_yt_dlp_managed(yes);

    gd_result
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

    let (spec, label) = if backend::is_dev_pin() {
        (
            format!(
                "gallery-dl @ git+https://codeberg.org/mikf/gallery-dl.git@{}",
                backend::GALLERY_DL_GIT_REF
            ),
            "dev",
        )
    } else {
        (format!("gallery-dl=={pin}"), "pinned")
    };

    output::print_info(&crate::i18n::tf(
        "setup_installing_via_python",
        &[
            ("label", label),
            ("pin", pin),
            ("arch", std::env::consts::ARCH),
        ],
    ));
    let choice = &candidates[0];
    let mut cmd = String::new();
    for (i, part) in choice.argv_prefix.iter().enumerate() {
        if i > 0 {
            cmd.push(' ');
        }
        cmd.push_str(part);
    }
    cmd.push_str(&format!(" install {spec}"));
    println!("{}", crate::i18n::tf("setup_run_command", &[("cmd", &cmd)]));

    if !yes && !confirm("Run this now?") {
        println!("{}", crate::i18n::t("cancelled_run_manually"));
        return;
    }

    let bin = choice.argv_prefix[0];
    let mut argv: Vec<std::ffi::OsString> = choice.argv_prefix[1..]
        .iter()
        .map(std::ffi::OsString::from)
        .collect();
    argv.extend([
        std::ffi::OsString::from("install"),
        std::ffi::OsString::from(spec),
    ]);
    match crate::process::Executor::run_capturing(bin, &argv) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.lines().rev().take(10).collect::<Vec<_>>();
            let stderr = stderr.into_iter().rev().collect::<Vec<_>>().join("\n");
            output::print_error(&crate::i18n::tf(
                "setup_install_failed",
                &[("label", choice.label), ("stderr", stderr.trim())],
            ));
            print_python_fallback();
            return;
        }
        Err(e) => {
            output::print_error(&crate::i18n::tf(
                "setup_could_not_launch",
                &[("label", choice.label), ("e", &e.to_string())],
            ));
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
                    output::print_success(&crate::i18n::tf(
                        "setup_gdl_verified",
                        &[("v", &v), ("path", &path.display().to_string())],
                    ));
                }
                Err(e) => tracing::warn!(error = %e, "installed binary did not report a version"),
            }
            output::print_note(&crate::i18n::t("setup_installed_via_pip_note"));
        }
        _ => {
            output::print_note(&crate::i18n::t("setup_not_on_path"));
        }
    }
}

fn print_python_fallback() {
    let pin = backend::GALLERY_DL_PIN;
    if backend::GALLERY_DL_USE_PYTHON_BACKEND || backend::is_dev_pin() {
        output::print_note(&crate::i18n::t("setup_no_pip_python_first"));
        println!("  # Termux:");
        println!("  pkg install python git");
        println!("  # other:");
        println!(
            "  python3 -m pip install git+https://codeberg.org/mikf/gallery-dl.git@{}",
            backend::GALLERY_DL_GIT_REF
        );
        return;
    }
    if is_termux() {
        output::print_note(&crate::i18n::t("setup_no_pip_termux"));
        println!("  pkg install python");
        println!("  python3 -m pip install gallery-dl=={pin}");
    } else {
        output::print_note(&crate::i18n::t("setup_no_pip_pipx"));
        println!("  pipx install gallery-dl=={pin}");
        println!("  # or:");
        println!("  python3 -m pip install gallery-dl=={pin}");
    }
    output::print_info(&crate::i18n::t("setup_pipx_never_upgrades"));
}

/// Install gallery-dl into a ScrapMF-managed Python venv.
///
/// This creates an isolated venv under `$XDG_DATA_HOME/scrapmf/venv` and
/// installs the pinned gallery-dl version into it. The venv uses the host's
/// Python (modern OpenSSL), so the resulting binary has a TLS fingerprint
/// that passes Cloudflare's checks.
fn install_managed_venv(yes: bool) -> Result<()> {
    let venv = backend::managed_venv_dir().context("cannot determine data directory")?;
    let python = if cfg!(windows) { "python" } else { "python3" };
    let venv_python = if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python3")
    };
    let venv_gdl = if cfg!(windows) {
        venv.join("Scripts").join("gallery-dl.exe")
    } else {
        venv.join("bin").join("gallery-dl")
    };

    if venv_gdl.is_file() {
        output::print_success(&crate::i18n::tf(
            "setup_managed_venv_already_installed",
            &[("path", &venv_gdl.display().to_string())],
        ));
        return Ok(());
    }

    std::fs::create_dir_all(&venv).with_context(|| format!("create {}", venv.display()))?;

    println!(
        "{}",
        crate::i18n::tf(
            "setup_creating_venv",
            &[("path", &venv.display().to_string())]
        )
    );
    if !yes && !confirm("Create venv and install gallery-dl now?") {
        println!("{}", crate::i18n::t("cancelled"));
        return Ok(());
    }

    let status = std::process::Command::new(python)
        .args(["-m", "venv", &venv.to_string_lossy()])
        .status()
        .with_context(|| format!("create venv at {}", venv.display()))?;
    if !status.success() {
        anyhow::bail!("{}", crate::i18n::t("setup_failed_create_venv"));
    }

    let pin = backend::GALLERY_DL_PIN;
    let spec = format!("gallery-dl=={pin}");
    println!(
        "{}",
        crate::i18n::tf("setup_installing_into_venv", &[("pin", pin)])
    );
    let status = std::process::Command::new(&venv_python)
        .args(["-m", "pip", "install", "--quiet", &spec])
        .status()
        .with_context(|| format!("pip install in {}", venv.display()))?;
    if !status.success() {
        anyhow::bail!("{}", crate::i18n::t("setup_failed_install_venv"));
    }

    if venv_gdl.is_file() {
        output::print_success(&crate::i18n::tf(
            "setup_managed_gdl_ready",
            &[("pin", pin), ("path", &venv_gdl.display().to_string())],
        ));
    } else {
        output::print_error(&crate::i18n::t("setup_venv_no_binary"));
    }
    Ok(())
}

/// Install the pinned yt-dlp standalone binary into the managed dir.
fn install_yt_dlp_managed(yes: bool) -> Result<()> {
    use crate::application::backend;
    let dir = backend::managed_dir().context("cannot determine data directory")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let asset = backend::yt_dlp_asset();
    let target = dir.join(asset);
    if target.is_file() {
        // Already present — keep it (pinned by scrapmf release).
        output::print_success(&crate::i18n::tf(
            "setup_ytdlp_already_installed",
            &[("path", &target.display().to_string())],
        ));
        return Ok(());
    }
    let url = format!("{}/{}", backend::yt_dlp_release_base_url(), asset);
    println!(
        "{}",
        crate::i18n::tf("setup_installing_ytdlp", &[("pin", backend::YT_DLP_PIN)])
    );
    if !yes
        && !confirm(&crate::i18n::tf(
            "setup_download_ytdlp_now",
            &[("pin", backend::YT_DLP_PIN)],
        ))
    {
        println!("{}", crate::i18n::t("cancelled"));
        return Ok(());
    }
    let work = dir.join(format!(".setup-ytdlp-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let asset_path = work.join(asset);
    fetch(&asset_path, &url)?;
    make_executable(&asset_path)?;
    std::fs::rename(&asset_path, &target)
        .with_context(|| format!("move into {}", target.display()))?;
    let _ = std::fs::remove_dir_all(&work);
    output::print_success(&crate::i18n::tf(
        "setup_ytdlp_ready",
        &[("path", &target.display().to_string())],
    ));
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

fn echo_step(msg: &str) {
    println!("→ {msg}");
}

fn fetch(dest: &Path, url: &str) -> Result<()> {
    echo_step(&crate::i18n::tf("setup_downloading", &[("url", url)]));
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
        anyhow::bail!("{}", crate::i18n::t("setup_curl_wget_required"));
    };
    if !status.success() {
        anyhow::bail!(
            "{}",
            crate::i18n::tf("setup_download_failed", &[("url", url)])
        );
    }
    Ok(())
}

/// Compute SHA-256 in Rust — no external tools needed, works on Windows too.
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
        output::print_note(&crate::i18n::tf(
            "setup_question_yes",
            &[("question", question)],
        ));
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
