//! Optional site-provider plugins, installed into scrapmf's OWN managed
//! environment (never the user's system Python) — same philosophy as the
//! bundled gallery-dl backend.
//!
//! One plugin today: `threads` (upstream package `threadstractormf`,
//! repository <https://github.com/ExtractorsMF/ThreadstractorMF>).
//!
//! Layout under `$XDG_DATA_HOME/scrapmf/`:
//! ```text
//!   plugins/threads/venv/     dedicated virtualenv (python3 -m venv)
//!   plugins/threads/version   installed pin, e.g. "v1.2.0"
//!   bin/threadstractormf      entry-point link to the venv binary
//! ```
//!
//! Binary resolution mirrors [`crate::application::backend`]: env override
//! `SCRAPMF_THREADS_BIN` beats the managed install beats a PATH fallback.
//!
//! A plugin is *enabled* when its managed venv exists AND the user has not
//! toggled it off (`[plugins] threads_disabled`). While not enabled, every
//! threads UI surface (site menus, cookies prompts, startup site template)
//! hides itself; direct threads URLs fail with an actionable message.
//!
//! Removing the plugin deletes the whole `plugins/threads/` tree — using
//! threads again requires reinstalling from scratch.

use std::path::{Path, PathBuf};

/// Exact upstream release scrapmf installs. Bump manually per scrapmf
/// release (same policy as `application::backend::GALLERY_DL_PIN`).
pub const THREADSTRACTOR_PIN: &str = "v1.2.0";

// ─── Plugin registry ────────────────────────────────────────────────────────

/// Static descriptor for a plugin. Adding a new plugin = one entry here plus
/// its state/install dispatch; menus render straight from this registry.
#[derive(Debug, Clone)]
pub struct PluginDef {
    /// Stable identifier (config keys, dispatch).
    pub id: &'static str,
    /// Display name shown in menus.
    pub title: &'static str,
    /// Vendor line, e.g. "MFApplications".
    pub vendor: &'static str,
}

/// Every plugin scrapmf knows about, in menu order.
pub const REGISTRY: &[PluginDef] = &[
    PluginDef {
        id: "threads",
        title: "ThreadstractorMF",
        vendor: "MFApplications",
    },
    PluginDef {
        id: "termux-scan",
        title: "Termux MediaScan",
        vendor: "Termux",
    },
];

/// Look up a plugin by id.
pub fn by_id(id: &str) -> Option<&'static PluginDef> {
    REGISTRY.iter().find(|p| p.id == id)
}

/// Public git source used by `pip install` during Enable.
pub const THREADSTRACTOR_REPO: &str = "https://github.com/ExtractorsMF/ThreadstractorMF.git";

const PKG_SPEC: &str = concat!(
    "threadstractormf[browser] @ git+",
    "https://github.com/ExtractorsMF/ThreadstractorMF.git@v1.2.0"
);

/// Lifecycle state of the threads plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginState {
    /// No managed venv — threads surfaces hidden everywhere.
    NotInstalled,
    /// Managed venv exists but the user toggled it off; files are kept.
    Disabled,
    /// Active; carries the installed pin read from the version file.
    Enabled(String),
}

// ─── Paths ──────────────────────────────────────────────────────────────────

/// Base dir for plugin payloads: `$XDG_DATA_HOME/scrapmf/plugins`.
fn plugins_base() -> Option<PathBuf> {
    dirs::data_dir().map(|p| p.join("scrapmf/plugins"))
}

/// Per-plugin dir: `…/scrapmf/plugins/<name>` (name: "threads").
fn plugin_dir(name: &str) -> Option<PathBuf> {
    plugins_base().map(|p| p.join(name))
}

fn threads_venv() -> Option<PathBuf> {
    plugin_dir("threads").map(|p| p.join("venv"))
}

fn threads_version_file() -> Option<PathBuf> {
    plugin_dir("threads").map(|p| p.join("version"))
}

/// Managed entry point exposed next to the gallery-dl backend:
/// `…/scrapmf/bin/threadstractormf`.
fn managed_binary_path() -> Option<PathBuf> {
    dirs::data_dir().map(|p| p.join("scrapmf/bin/threadstractormf"))
}

/// The actual executable inside the venv (platform-aware).
fn venv_binary() -> Option<PathBuf> {
    let venv = threads_venv()?;
    #[cfg(windows)]
    let p = venv.join("Scripts").join("threadstractormf.exe");
    #[cfg(not(windows))]
    let p = venv.join("bin").join("threadstractormf");
    Some(p)
}

// ─── State ──────────────────────────────────────────────────────────────────

/// Pure state computation — unit-testable without touching the filesystem.
fn compute_state(venv_exists: bool, disabled: bool) -> PluginState {
    let version = threads_version_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    compute_state_with(venv_exists, disabled, &version)
}

/// Pure state from injected inputs (no filesystem access).
fn compute_state_with(venv_exists: bool, disabled: bool, version_file: &str) -> PluginState {
    match (venv_exists, disabled) {
        (false, _) => PluginState::NotInstalled,
        (true, true) => PluginState::Disabled,
        (true, false) => {
            let trimmed = version_file.trim();
            if trimmed.is_empty() {
                PluginState::Enabled(format!("unknown (pin {THREADSTRACTOR_PIN})"))
            } else {
                PluginState::Enabled(trimmed.to_string())
            }
        }
    }
}

/// Current threads plugin lifecycle state (filesystem + config).
pub fn threads_state() -> PluginState {
    let venv_exists = threads_venv().is_some_and(|p| p.is_dir());
    let disabled = crate::config::load()
        .map(|c| c.plugins.threads_disabled)
        .unwrap_or(false);
    compute_state(venv_exists, disabled)
}

/// Shorthand used by UI gating: true only when the plugin can be used.
pub fn threads_enabled() -> bool {
    matches!(threads_state(), PluginState::Enabled(_))
}

// ─── Termux MediaScan (lightweight, no venv) ──────────────────────────────────

fn is_termux_env() -> bool {
    std::env::var_os("TERMUX_VERSION").is_some()
        || std::env::var_os("PREFIX").is_some_and(|p| p.to_string_lossy().contains("com.termux"))
}

/// Whether termux-media-scan is usable on this host (Termux + binary present).
pub fn is_termux_available() -> bool {
    which::which("termux-media-scan").is_ok()
}

/// Current termux-scan plugin state (no venv, just toggle + binary check).
pub fn termux_scan_state() -> PluginState {
    let disabled = crate::config::load()
        .map(|c| c.plugins.termux_scan_disabled)
        .unwrap_or(true); // disabled by default (opt-in)
    if disabled {
        return PluginState::Disabled;
    }
    if is_termux_available() {
        PluginState::Enabled("termux-media-scan".to_string())
    } else if is_termux_env() {
        // Termux without termux-api installed — show as NotInstalled with help
        PluginState::NotInstalled
    } else {
        // Non-Termux: treat as Disabled (silent no-op, never fails)
        PluginState::Disabled
    }
}

pub fn termux_scan_enabled() -> bool {
    matches!(termux_scan_state(), PluginState::Enabled(_))
}

pub fn set_termux_scan_disabled(disabled: bool) -> anyhow::Result<()> {
    crate::config::update(|cfg| {
        cfg.plugins.termux_scan_disabled = disabled;
    })
}

// ─── Binary resolution ──────────────────────────────────────────────────────

/// Resolve the threads provider binary.
///
/// Priority: `SCRAPMF_THREADS_BIN` env override > managed venv entry point >
/// `threadstractormf` on PATH (dev convenience). Errors carry install hints;
/// callers should have gated UI flows behind [`threads_enabled`] already.
pub fn resolve_binary() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("SCRAPMF_THREADS_BIN") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(managed) = managed_binary_path()
        && managed.is_file()
    {
        return Ok(managed);
    }
    if let Some(venv_bin) = venv_binary()
        && venv_bin.is_file()
    {
        return Ok(venv_bin);
    }
    if let Ok(path) = which::which("threadstractormf") {
        return Ok(path);
    }
    anyhow::bail!(
        "threads plugin is not enabled. Enable it in scrapmf → Plugins (installs \
         threadstractormf {THREADSTRACTOR_PIN} into a dedicated scrapmf venv)"
    )
}

// ─── Install / update / remove ──────────────────────────────────────────────

/// Install (or re-install/update) the threads plugin at the pinned version.
///
/// Steps: create venv → `pip install` pinned package (+ `[browser]` extra so
/// the Playwright fallback works) → download Chromium into the shared
/// ms-playwright cache (~150MB, skipped automatically when already present)
/// → smoke-launch headless Chromium → expose the entry point → create the
/// threads site template. Progress goes straight to the terminal (inherited
/// stdio) because this runs from the plain-terminal Plugins menu.
pub fn install() -> anyhow::Result<()> {
    use crate::output;
    let venv =
        threads_venv().ok_or_else(|| anyhow::anyhow!("cannot determine scrapmf data dir"))?;
    std::fs::create_dir_all(&venv)?;
    output::print_info(&format!("creating virtualenv at {}", venv.display()));
    crate::process::Executor::run_inherited_checked(
        "python3",
        &["-m".into(), "venv".into(), venv.as_os_str().to_owned()],
    )?;

    let pip = venv_pip(&venv);
    output::print_info(&format!(
        "installing threadstractormf {THREADSTRACTOR_PIN} (this compiles/downloads deps)"
    ));
    crate::process::Executor::run_inherited_checked(&pip.0, &pip.1)?;

    // Chromium for the Playwright DOM fallback. Shared user-level cache:
    // re-runs are cheap when it already exists.
    output::print_info("downloading Chromium for Playwright (~150MB, cached across runs)");
    let pw_install = playwright_cli(&venv);
    if let Err(e) = crate::process::Executor::run_inherited_checked(&pw_install.0, &pw_install.1) {
        output::print_help(&format!(
            "Chromium download failed ({e}). The plugin still works for GraphQL-only \
             fetches; retry later with: {pw} install chromium (system libs may need \
             '{pw_alt} install-deps chromium' with sudo)",
            pw = pw_install.0,
            pw_alt = pw_install.0,
        ));
    }

    // Smoke test: headless launch proves browser + system libs are usable.
    let py = venv_python(&venv);
    output::print_info("verifying headless Chromium launch");
    crate::process::Executor::run_inherited_checked(&py.0, &py.1)?;

    // Version marker.
    if let Some(vf) = threads_version_file() {
        std::fs::write(&vf, THREADSTRACTOR_PIN)?;
    }

    // Expose the entry point where doctor/backend look for it.
    if let (Some(src), Some(dst)) = (venv_binary(), managed_binary_path()) {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&dst);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&src, &dst)
                .or_else(|_| std::fs::copy(&src, &dst).map(|_| ()))?;
        }
        #[cfg(windows)]
        {
            std::fs::copy(&src, &dst)?;
        }
    }

    // Re-enable in config in case it was toggled off before.
    set_disabled(false)?;

    // Site template lives only while the plugin exists.
    crate::config::ensure_threads_site()?;
    output::print_success(&format!(
        "threads plugin enabled (threadstractormf {THREADSTRACTOR_PIN})"
    ));
    Ok(())
}

/// Delete the plugin payload completely (venv, version marker, entry point).
/// Re-enabling afterwards performs a fresh install.
pub fn remove() -> anyhow::Result<()> {
    if let Some(dir) = plugin_dir("threads")
        && dir.exists()
    {
        std::fs::remove_dir_all(&dir)?;
    }
    if let Some(link) = managed_binary_path() {
        let _ = std::fs::remove_file(link);
    }
    Ok(())
}

/// Toggle the user-facing enable flag without touching installed files.
pub fn set_disabled(disabled: bool) -> anyhow::Result<()> {
    crate::config::update(|cfg| {
        cfg.plugins.threads_disabled = disabled;
    })
}

// ─── Interactive menu ───────────────────────────────────────────────────────
// UI lives in `cli/interactive/plugins.rs`; this module stays logic-only.

// ─── venv tool paths ────────────────────────────────────────────────────────

fn venv_python(venv: &Path) -> (String, Vec<std::ffi::OsString>) {
    #[cfg(windows)]
    let py = venv.join("Scripts").join("python.exe");
    #[cfg(not(windows))]
    let py = venv.join("bin").join("python");
    let script = "from playwright.sync_api import sync_playwright; \
                  p = sync_playwright().start(); \
                  b = p.chromium.launch(headless=True); b.close(); p.stop()";
    (
        py.to_string_lossy().into_owned(),
        vec!["-c".into(), script.into()],
    )
}

fn venv_pip(venv: &Path) -> (String, Vec<std::ffi::OsString>) {
    #[cfg(windows)]
    let py = venv.join("Scripts").join("python.exe");
    #[cfg(not(windows))]
    let py = venv.join("bin").join("python");
    // browser-cookie3 is required by threadstractormf.auth.load_from_browser
    // but upstream's `[browser]` extra only ships playwright (fixed upstream
    // in a later release); install it explicitly so cookie loading works.
    (
        py.to_string_lossy().into_owned(),
        vec![
            "-m".into(),
            "pip".into(),
            "install".into(),
            "--upgrade".into(),
            PKG_SPEC.into(),
            "browser-cookie3>=0.20".into(),
        ],
    )
}

fn playwright_cli(venv: &Path) -> (String, Vec<std::ffi::OsString>) {
    #[cfg(windows)]
    let cli = venv.join("Scripts").join("playwright.exe");
    #[cfg(not(windows))]
    let cli = venv.join("bin").join("playwright");
    (
        cli.to_string_lossy().into_owned(),
        vec!["install".into(), "chromium".into()],
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn state_matrix() {
        assert_eq!(
            compute_state_with(false, false, ""),
            PluginState::NotInstalled
        );
        assert_eq!(
            compute_state_with(false, true, "v1.2.0"),
            PluginState::NotInstalled
        );
        assert_eq!(
            compute_state_with(true, true, "v1.2.0"),
            PluginState::Disabled
        );
        match compute_state_with(true, false, "v1.2.0") {
            PluginState::Enabled(v) => assert_eq!(v, "v1.2.0"),
            other => panic!("expected Enabled, got {other:?}"),
        }
        // empty/missing version file falls back to the pin marker
        match compute_state_with(true, false, "") {
            PluginState::Enabled(v) => assert!(v.contains(THREADSTRACTOR_PIN), "got {v}"),
            other => panic!("expected Enabled fallback, got {other:?}"),
        }
    }

    #[test]
    fn pkg_spec_pins_version() {
        assert!(PKG_SPEC.contains("@v1.2.0"));
        assert!(PKG_SPEC.starts_with("threadstractormf[browser] @ git+"));
    }

    #[test]
    fn repo_url_is_public_https() {
        assert!(THREADSTRACTOR_REPO.starts_with("https://github.com/"));
    }
}
