//! Bundled (pinned) gallery-dl backend management.
//!
//! scrapmf always prefers its OWN copy of gallery-dl, downloaded once by
//! `scrapmf setup` into the managed directory and frozen at
//! [`GALLERY_DL_PIN`]. It never updates on its own and never touches any
//! system-wide installation: when gallery-dl upstream changes something that
//! breaks us, we bump the pin in a controlled scrapmf release instead.

use std::path::{Path, PathBuf};

/// The exact gallery-dl version scrapmf is tested against.
///
/// Bump checklist (manual, never automatic):
/// 1. Confirm `gallery-dl.bin` + `SHA256SUMS` assets exist at
///    https://codeberg.org/mikf/gallery-dl/releases/tag/v{PIN}
/// 2. Update this constant, run the full test suite.
/// 3. Cut a new scrapmf release — users get the new pin via `scrapmf setup`.
pub const GALLERY_DL_PIN: &str = "1.33.0-dev";

/// Git ref (commit) of the dev build of gallery-dl that ships the Instagram
/// reels/stories fix. Only used when [`GALLERY_DL_PIN`] is a dev version.
pub const GALLERY_DL_GIT_REF: &str = "b2df7f9444f4dbfda84cc7282eae96db99ef441d";

/// Whether the pinned version is a development build (no official release
/// assets on Codeberg).
pub fn is_dev_pin() -> bool {
    GALLERY_DL_PIN.contains("dev") || GALLERY_DL_PIN.contains("rc")
}

/// Legacy constant kept for callers that reference the Linux asset directly.
pub const GALLERY_DL_ASSET: &str = "gallery-dl.bin";

/// The standalone gallery-dl asset for the current OS, as published on the
/// pinned Codeberg release. Upstream ships `gallery-dl.bin` (Linux) and
/// `gallery-dl.exe` (Windows); there is no standalone build for macOS.
pub fn gallery_dl_asset() -> &'static str {
    if cfg!(windows) {
        "gallery-dl.exe"
    } else {
        GALLERY_DL_ASSET
    }
}

/// Managed binary file name once installed into the managed dir. Windows
/// needs the `.exe` extension for direct execution.
pub fn managed_binary_name() -> &'static str {
    gallery_dl_asset()
}

/// Base URL of the pinned release assets on Codeberg.
pub fn release_base_url() -> String {
    format!("https://codeberg.org/mikf/gallery-dl/releases/download/v{GALLERY_DL_PIN}")
}

/// Managed install dir: `$XDG_DATA_HOME/scrapmf/bin`.
pub fn managed_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|p| p.join("scrapmf/bin"))
}

/// Path of the managed binary if it is already installed.
fn managed_installed() -> Option<PathBuf> {
    let p = managed_dir()?.join(managed_binary_name());
    p.is_file().then_some(p)
}

/// Where the resolved binary came from — drives doctor output and warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `SCRAPMF_GALLERY_DL` env var (dev/debug escape hatch).
    Env(PathBuf),
    /// `backend.gallery_dl_path` in config.toml.
    Config(PathBuf),
    /// scrapmf's own pinned copy — the normal case after `scrapmf setup`.
    Managed(PathBuf),
    /// System $PATH fallback (user's own gallery-dl). Works, but is not
    /// version-controlled by us.
    System(PathBuf),
    /// Nothing usable found — user should run `scrapmf setup`.
    Missing,
}

impl Source {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Source::Env(p) | Source::Config(p) | Source::Managed(p) | Source::System(p) => {
                Some(p.as_path())
            }
            Source::Missing => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Source::Env(_) | Source::Config(_) => "explicit override",
            Source::Managed(_) => "bundled (pinned)",
            Source::System(_) => "system (NOT pinned)",
            Source::Missing => "missing",
        }
    }
}

/// Inputs for resolution, gathered from the environment. Split out from
/// [`resolve`] so precedence stays pure and testable.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    pub env: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub managed: Option<PathBuf>,
    pub system: Option<PathBuf>,
}

/// Precedence: env > config > managed (pinned) > system $PATH > missing.
pub fn pick(s: &Sources) -> Source {
    if let Some(p) = &s.env {
        return Source::Env(p.clone());
    }
    if let Some(p) = &s.config {
        return Source::Config(p.clone());
    }
    if let Some(p) = &s.managed {
        return Source::Managed(p.clone());
    }
    if let Some(p) = &s.system {
        return Source::System(p.clone());
    }
    Source::Missing
}

/// Resolve using real inputs (env var, config file, managed dir, $PATH).
pub fn resolve(config_override: Option<PathBuf>) -> Source {
    let env = std::env::var_os("SCRAPMF_GALLERY_DL")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    // Dev pins have no managed binary; skip the managed dir so the pipx/pip
    // install on $PATH is picked up instead.
    let managed = if is_dev_pin() {
        None
    } else {
        managed_installed()
    };
    let system = which::which("gallery-dl").ok();

    let sources = Sources {
        env,
        config: config_override.filter(|p| p.is_file()),
        managed,
        system,
    };
    pick(&sources)
}

/// The gallery-dl version that would actually run, or `None` when it cannot be
/// asked. Probed at most once per process — this is only read while composing
/// an error or a doctor line, and spawning the binary twice for one answer is
/// waste.
pub fn installed_version() -> Option<String> {
    static CACHE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let exe = gallery_dl_executable().ok()?;
            let out = crate::process::Executor::run_capturing(
                &exe.to_string_lossy(),
                &[std::ffi::OsString::from("--version")],
            )
            .ok()?;
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!v.is_empty()).then_some(v)
        })
        .clone()
}

/// Warn when the installed gallery-dl is not the pinned build.
///
/// Upgrading scrapmf does not reinstall the backend — only `scrapmf setup`
/// does. The two drift apart silently, and then every gallery-dl bug already
/// fixed upstream keeps reproducing here until someone happens to notice. This
/// bit real debugging time: an Instagram extractor failing on `1.32.16` while
/// scrapmf pinned `1.32.17` looks exactly like a scrapmf bug until the two
/// versions are named side by side.
pub fn pin_drift_notice() -> Option<String> {
    drift_notice(installed_version()?.as_str(), GALLERY_DL_PIN)
}

/// The comparison behind [`pin_drift_notice`], split out so it can be tested
/// without spawning a process.
fn drift_notice(installed: &str, pinned: &str) -> Option<String> {
    if pinned.contains("dev") || pinned.contains("rc") {
        // Dev pins have no official build to compare against; skip the
        // version drift check to avoid false positives.
        return None;
    }
    if installed == pinned {
        return None;
    }
    Some(format!(
        "bundled gallery-dl is {installed} but this scrapmf pins {pinned} \
         — run 'scrapmf setup' to update it"
    ))
}

/// Resolved executable path, or an actionable error telling the user what to
/// run. This is THE entry point every scrape-time call site must use.
pub fn gallery_dl_executable() -> anyhow::Result<PathBuf> {
    let cfg = crate::config::load().unwrap_or_default();
    match resolve(cfg.backend.gallery_dl_path.clone()) {
        Source::Missing => Err(anyhow::anyhow!(
            "gallery-dl backend is not installed\n  help: run 'scrapmf setup' to install the \
                 bundled pinned v{GALLERY_DL_PIN} build\n  note: your system gallery-dl (if any) is \
                 never used automatically"
        )),
        src => src
            .path()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("backend resolution returned an unusable result")),
    }
}

/// Extract the expected sha256 hex digest of `filename` from the contents of
/// an official SHA256SUMS file (`<hash>  <filename>` lines).
pub fn extract_sha256_for(sums_content: &str, filename: &str) -> Option<String> {
    for line in sums_content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let hash = hash.trim();
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue; // malformed line
        }
        if name.trim_start() == filename || name.trim_start().ends_with(&format!("/{filename}")) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{GALLERY_DL_PIN, Source, Sources, drift_notice, extract_sha256_for, pick};
    use std::path::PathBuf;

    /// The whole reason this exists: a scrapmf release bumps the pin, the
    /// installed binary stays where it was until someone runs `scrapmf setup`,
    /// and every upstream bug fixed in between keeps reproducing here while
    /// looking like a scrapmf bug.
    #[test]
    fn a_stale_backend_is_named_against_the_pin() {
        let notice =
            drift_notice("1.32.16", "1.32.17").expect("a version behind the pin must be reported");
        assert!(notice.contains("1.32.16"), "{notice}");
        assert!(notice.contains("1.32.17"), "{notice}");
        assert!(notice.contains("scrapmf setup"), "{notice}");
    }

    #[test]
    fn a_matching_backend_is_silent() {
        assert!(drift_notice("1.32.17", "1.32.17").is_none());
    }

    /// A system gallery-dl newer than the pin is not drift we should chase.
    #[test]
    fn only_a_mismatch_is_reported() {
        assert!(drift_notice("1.33.0", "1.32.17").is_some());
    }

    fn pb(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    #[test]
    fn pin_is_set() {
        assert!(GALLERY_DL_PIN.starts_with("1."));
        assert!(!GALLERY_DL_PIN.is_empty());
    }

    #[test]
    fn precedence_env_config_managed_system_missing() {
        let base = Sources::default();
        assert_eq!(pick(&base), Source::Missing);

        let with_system = Sources {
            system: pb("/usr/bin/gallery-dl"),
            ..Default::default()
        };
        assert_eq!(
            pick(&with_system),
            Source::System(pb("/usr/bin/gallery-dl").unwrap())
        );

        // Managed beats system — scrapmf uses its own pinned copy
        let with_managed = Sources {
            managed: pb("~/.local/share/scrapmf/bin/gallery-dl"),
            ..with_system.clone()
        };
        assert_eq!(
            pick(&with_managed),
            Source::Managed(pb("~/.local/share/scrapmf/bin/gallery-dl").unwrap())
        );

        // Config beats managed
        let with_config = Sources {
            config: pb("/opt/gdl"),
            ..with_managed
        };
        assert_eq!(pick(&with_config), Source::Config(pb("/opt/gdl").unwrap()));

        // Env beats everything
        let with_env = Sources {
            env: pb("/tmp/dev-gdl"),
            ..with_config
        };
        assert_eq!(pick(&with_env), Source::Env(pb("/tmp/dev-gdl").unwrap()));
    }

    #[test]
    fn labels_distinguish_pinned_from_system() {
        assert_eq!(
            Source::Managed(PathBuf::from("x")).label(),
            "bundled (pinned)"
        );
        assert_eq!(
            Source::System(PathBuf::from("x")).label(),
            "system (NOT pinned)"
        );
    }

    #[test]
    fn asset_name_matches_os() {
        let asset = super::gallery_dl_asset();
        if cfg!(windows) {
            assert_eq!(asset, "gallery-dl.exe");
        } else {
            assert_eq!(asset, "gallery-dl.bin");
        }
        // The managed name is always the same as the published asset name.
        assert_eq!(super::managed_binary_name(), asset);
    }

    const SUMS: &str = "\
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  gallery-dl.bin
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  gallery-dl.exe
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde1  other-file.tar.gz
";

    #[test]
    fn sha256_sums_extraction() {
        assert_eq!(
            extract_sha256_for(SUMS, "gallery-dl.bin"),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into())
        );
        // Windows asset is listed in the same SHA256SUMS file.
        assert_eq!(
            extract_sha256_for(SUMS, "gallery-dl.exe"),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into())
        );
        assert_eq!(extract_sha256_for(SUMS, "missing.bin"), None);
        assert_eq!(extract_sha256_for("", "gallery-dl.bin"), None);
        // malformed lines are skipped, valid ones still found
        assert_eq!(
            extract_sha256_for(
                "garbage line\nzzzz  short\n\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  gallery-dl.bin",
                "gallery-dl.bin"
            ),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into())
        );
    }
}
