use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub backend: Backend,
    #[serde(default)]
    pub plugins: Plugins,
    #[serde(default, skip_serializing)]
    pub presets: HashMap<String, Preset>,
    #[serde(default, skip_serializing)]
    pub sites: HashMap<String, Site>,
    #[serde(default, skip_serializing)]
    pub profiles: HashMap<String, Profile>,
}

/// Optional site-provider plugins (see `crate::plugins`). Files live under
/// `$XDG_DATA_HOME/scrapmf/plugins/`; this section only carries user toggles.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugins {
    /// threads plugin manually disabled from the Plugins menu while keeping
    /// its installed files (re-enable without re-downloading).
    #[serde(default)]
    pub threads_disabled: bool,
    /// termux media-scan plugin disabled by default (opt-in). When enabled,
    /// scrapmf runs `termux-media-scan <scrapmf_dir>` once per finished
    /// download (quick / saved profile / url / CLI). Best-effort, never fails.
    #[serde(default = "default_true")]
    pub termux_scan_disabled: bool,
}

impl Default for Plugins {
    fn default() -> Self {
        Self {
            threads_disabled: false,
            termux_scan_disabled: true,
        }
    }
}

/// Backend resolution overrides. By default scrapmf uses its own bundled,
/// pinned gallery-dl (installed via `scrapmf setup`) and never the user's
/// system installation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Backend {
    /// Explicit path to a gallery-dl binary overriding even the bundled one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gallery_dl_path: Option<PathBuf>,
    /// Offer to install the bundled gallery-dl on first interactive run.
    #[serde(default = "default_auto_install")]
    pub auto_install_backends: bool,
}

fn default_auto_install() -> bool {
    true
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("~/scrapmf")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    /// Base output directory (tilde ~/ expanded at use time)
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    /// Download archive (dedup): remember downloaded media per account so
    /// re-runs skip them. Backed by JSONL files under
    /// `<config>/archive/<site>/<account>.jsonl`.
    #[serde(default = "default_true")]
    pub archive: bool,
    /// Browser to take session cookies from, for every site.
    ///
    /// This used to be baked into each generated `sites/*.toml` as
    /// `cookies_from_browser = "brave"`, which picked a browser the user may
    /// not have — and, on a machine with more than one install of that browser,
    /// picked the wrong profile directory. Chrome-family channels each use a
    /// different directory (`Brave-Browser` vs `Brave-Origin`), so a wrong guess
    /// silently reads an abandoned session while the live one sits untouched.
    ///
    /// `None` means anonymous: no browser is touched and no session is sent.
    /// Set it under Configuration → General settings, which lists only the
    /// channels actually installed here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies_from_browser: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Preset {
    /// Output directory for this preset
    pub output_dir: Option<PathBuf>,
    /// Extra args to pass to provider (allow-list validated)
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Optional URL pattern to auto-match (e.g., "pixiv.net")
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimit {
    pub sleep: Option<String>,
    #[serde(default)]
    pub sleep_request: Option<String>,
    #[serde(default)]
    pub sleep_429: Option<u32>,
    #[serde(default)]
    pub limit_rate: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Site {
    pub site: Option<String>,
    pub pattern: Option<String>,
    /// Additional URL substrings to auto-match (e.g. x.com + twitter.com).
    /// Matched in addition to `pattern` — first hit wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<String>,
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    pub cookies: Option<PathBuf>,
    pub cookies_from_browser: Option<String>,
    /// Named cookie profile (file in ~/.config/scrapmf/cookies/<name>.txt).
    /// Takes precedence over cookies_from_browser for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_profile: Option<String>,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    pub archive: Option<PathBuf>,
    #[serde(default)]
    pub extractor: HashMap<String, toml::Value>,
    pub filename_template: Option<String>,
    pub directory_template: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Account {
    #[serde(alias = "user")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub cookies: Option<PathBuf>,
    pub cookies_from_browser: Option<String>,
    /// Named cookie profile (file in ~/.config/scrapmf/cookies/<name>.txt).
    /// Takes precedence over cookies_from_browser for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_profile: Option<String>,
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

/// A saved batch: a name, the content it wants, and the accounts to run it on.
///
/// Separate from [`Profile`] on purpose — a task answers "what and for whom",
/// a profile answers "how", with per-run content choices and site overrides. A
/// task stores no output or template configuration: `{scrapmf_root}` takes the
/// task's name, so its media lands in its own tree without asking.
///
/// `kinds` holds the labels from the site registry rather than an enum: the
/// selectable kinds live in the interactive layer, and the label round-trip is
/// what the menus already speak.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Task {
    /// Task id, taken from the filename stem.
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Site key -> the accounts on it. Same shape as a profile, which is what
    /// lets both share account and cookie semantics.
    #[serde(default)]
    pub accounts: std::collections::HashMap<String, Vec<TaskAccount>>,
}

/// An account inside a task: a plain account plus the content it wants.
///
/// The content lives here, not on the task, because it is asked per account —
/// two instagram accounts in one task can want different things. Flattened so
/// the file reads like a profile's account with `kinds` alongside.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskAccount {
    #[serde(flatten)]
    pub account: Account,
    /// This account's content. `"All"` expands per site when the task runs, so
    /// a task spanning networks follows what each one actually supports.
    #[serde(default)]
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Legacy flat list: sites = ["instagram"] — kept for backward compat, migrated to accounts if accounts empty
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<String>,
    /// Flexible map: site -> list of accounts (allows multiple IG per person)
    /// TOML: [[accounts.instagram]] username = "user1"
    ///       [[accounts.instagram]] username = "user2"
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub accounts: HashMap<String, Vec<Account>>,
    pub output_dir: Option<PathBuf>,
    pub cookies: Option<PathBuf>,
    pub cookies_from_browser: Option<String>,
    /// Named cookie profile (file in ~/.config/scrapmf/cookies/<name>.txt).
    /// Takes precedence over cookies_from_browser for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_profile: Option<String>,
    #[serde(default)]
    pub overrides: HashMap<String, Site>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("~/scrapmf"),
            archive: true,
            cookies_from_browser: None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{Config, General, Preset};

    #[test]
    fn default_output_dir() {
        let g = General::default();
        assert_eq!(g.output_dir, std::path::PathBuf::from("~/scrapmf"));
    }

    #[test]
    fn toml_parse_valid() {
        let toml = r#"
            [general]
            output_dir = "/tmp/out"

            [presets.pixiv]
            pattern = "pixiv.net"
            output_dir = "/tmp/pixiv"
            extra_args = ["--sleep", "1"]
        "#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.general.output_dir, std::path::PathBuf::from("/tmp/out"));
        assert!(cfg.presets.contains_key("pixiv"));
        assert_eq!(cfg.presets["pixiv"].pattern.as_deref(), Some("pixiv.net"));
    }

    /// Removed `default_provider` / `provider` keys in old user configs must be
    /// ignored silently (serde skips unknown fields), not break parsing.
    #[test]
    fn toml_parse_ignores_removed_provider_keys() {
        let toml = r#"
            [general]
            default_provider = "gallery-dl"
            output_dir = "/tmp/out"

            [presets.pixiv]
            provider = "yt-dlp"
            pattern = "pixiv.net"
        "#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.general.output_dir, std::path::PathBuf::from("/tmp/out"));
        assert_eq!(cfg.presets["pixiv"].pattern.as_deref(), Some("pixiv.net"));
    }

    #[test]
    fn toml_parse_partial_defaults() {
        let toml = r#"
            [general]
            output_dir = "/tmp/only"
        "#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        assert_eq!(
            cfg.general.output_dir,
            std::path::PathBuf::from("/tmp/only")
        );
    }

    #[test]
    fn preset_defaults() {
        let p = Preset {
            output_dir: None,
            extra_args: Vec::new(),
            pattern: None,
        };
        assert!(p.extra_args.is_empty());
    }

    #[test]
    fn config_default_has_no_presets() {
        let cfg = Config::default();
        assert!(cfg.presets.is_empty());
    }
}
