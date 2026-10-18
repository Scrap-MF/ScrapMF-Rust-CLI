pub mod cookies;
pub mod model;

pub(crate) use fs::restrict_perms;
pub use model::{Account, Config, General, Preset, Profile, RateLimit, Site, Task, TaskAccount};

use std::path::{Path, PathBuf};

use anyhow::Context;

pub(crate) mod fs;
mod migrations;
mod templates;
use fs::write_config_file;
pub use migrations::{
    migrate_all_sites_filenames, migrate_all_sites_highlights, migrate_inline_config_to_files,
    migrate_legacy_placeholders,
};
pub use templates::{
    ensure_example_sites, ensure_facebook_site, ensure_threads_site, ensure_tiktok_site,
    ensure_twitter_site, ensure_vsco_site, write_profile_file, write_task_file,
};

fn expand_tilde(path: &Path) -> PathBuf {
    crate::util::expand_tilde(path)
}

pub fn sites_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/sites"))
}

pub fn profiles_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/profiles"))
}

/// Saved tasks live beside profiles but never inside them: same one-file-per-
/// entry layout, different directory, different struct, different menus.
pub fn tasks_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/tasks"))
}

/// Site matches URL if `pattern` or any of `patterns` is contained in it.
fn site_matches(site: &Site, url: &str) -> bool {
    if let Some(ref pat) = site.pattern
        && url.contains(pat.as_str())
    {
        return true;
    }
    site.patterns.iter().any(|p| url.contains(p.as_str()))
}

pub fn resolve_site<'a>(
    url: &str,
    preset_name: Option<&str>,
    config: &'a Config,
) -> Option<(&'a String, &'a Site)> {
    if let Some(name) = preset_name {
        if let Some((k, site)) = config.sites.get_key_value(name) {
            return Some((k, site));
        }
        return None;
    }
    config
        .sites
        .iter()
        .find(|(_, site)| site_matches(site, url))
}

use std::sync::{Arc, Mutex, OnceLock};

fn config_cache() -> &'static Mutex<Option<Arc<Config>>> {
    static CACHE: OnceLock<Mutex<Option<Arc<Config>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Clear the in-memory config cache (called after save/update and in tests).
pub fn clear_cache() {
    if let Ok(mut g) = config_cache().lock() {
        *g = None;
    }
}

/// Load config from XDG `~/.config/scrapmf/config.toml` and presets from `presets/**/*.toml`.
pub fn load() -> anyhow::Result<Config> {
    // Fast path: use cached config if available (skip FS scans). Bypass in
    // tests so each test can use its own isolated temp XDG dir.
    if !cfg!(test)
        && let Ok(guard) = config_cache().lock()
        && let Some(cached) = guard.clone()
    {
        return Ok((*cached).clone());
    }
    let cfg = load_inner()?;
    if !cfg!(test)
        && let Ok(mut guard) = config_cache().lock()
    {
        *guard = Some(Arc::new(cfg.clone()));
    }
    Ok(cfg)
}

fn load_inner() -> anyhow::Result<Config> {
    // One-time migration: inline [sites]/[presets]/[profiles] → separate files.
    // Idempotent and cheap (early return if no inline tables). Not fatal: a
    // failure here must not stop scrapmf from running with the rest of the
    // config, but it was previously dropped entirely, so it never even reached
    // the startup warn that would have said what went wrong.
    if let Err(e) = migrations::migrate_inline_config_to_files() {
        tracing::warn!(error = %e, "inline config migration failed — continuing without it");
    }
    let Some(path) = config_path() else {
        return Ok(Config::default());
    };
    let mut cfg = if path.exists() {
        let s = std::fs::read_to_string(&path)
            .with_context(|| format!("read config {}", path.display()))?;
        let c: Config =
            toml::from_str(&s).map_err(|e| crate::error::ScrapmfError::ConfigParse {
                path: path.clone(),
                source: e,
            })?;
        // Validate extra_args early
        for (name, preset) in &c.presets {
            crate::application::scraper::validate_extra_args(&preset.extra_args)
                .with_context(|| format!("preset '{name}' extra_args"))?;
        }
        c
    } else {
        Config::default()
    };

    // Expand tilde for general output_dir for consistency (store expanded)
    // Keep original tilde in file, but in-memory expand for use
    // Do not mutate file, just in-memory
    // Layered presets: sites/persons/*.toml override inline (legacy)
    if let Some(dir) = presets_dir() {
        load_layered_presets(&mut cfg, &dir);
        if let Some(sites) = presets_sites_dir() {
            load_layered_presets(&mut cfg, &sites);
        }
        if let Some(persons) = presets_persons_dir() {
            load_layered_presets(&mut cfg, &persons);
        }
    }
    // New 3-layer: sites/ and profiles/ (user requested structure)
    if let Some(dir) = sites_dir() {
        load_layered_sites(&mut cfg, &dir);
    }
    if let Some(dir) = profiles_dir() {
        load_layered_profiles(&mut cfg, &dir);
    }

    Ok(cfg)
}

fn for_each_toml_file<F>(dir: &Path, f: &mut F)
where
    F: FnMut(String, String),
{
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            for_each_toml_file(&path, f);
            continue;
        }
        if path.extension().is_some_and(|e| e == "toml")
            && let Some(stem) = path.file_stem().and_then(|n| n.to_str())
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            f(stem.to_string(), content);
        }
    }
}

fn load_layered_presets(cfg: &mut Config, dir: &Path) {
    for_each_toml_file(dir, &mut |stem, content| {
        if let Ok(preset) = toml::from_str::<Preset>(&content)
            && crate::application::scraper::validate_extra_args(&preset.extra_args).is_ok()
        {
            cfg.presets.insert(stem, preset);
        }
    });
}

fn load_layered_sites(cfg: &mut Config, dir: &Path) {
    for_each_toml_file(dir, &mut |stem, content| {
        if let Ok(site) = toml::from_str::<Site>(&content)
            && crate::application::scraper::validate_extra_args(&site.extra_args).is_ok()
        {
            cfg.sites.insert(stem, site);
        }
    });
}

/// Read every `tasks/*.toml`, keyed by filename stem.
///
/// Separate from [`load_layered_profiles`] on purpose: a malformed task must
/// not be able to take a profile down with it, and vice versa.
pub fn load_tasks() -> std::collections::HashMap<String, Task> {
    let mut out = std::collections::HashMap::new();
    let Some(dir) = tasks_dir() else {
        return out;
    };
    for_each_toml_file(&dir, &mut |stem, content| {
        if let Ok(mut task) = toml::from_str::<Task>(&content) {
            if task.task.is_none() {
                task.task = Some(stem.to_string());
            }
            out.insert(stem.to_string(), task);
        }
    });
    out
}

fn load_layered_profiles(cfg: &mut Config, dir: &Path) {
    for_each_toml_file(dir, &mut |stem, content| {
        if let Ok(mut profile) = toml::from_str::<Profile>(&content) {
            if profile.accounts.is_empty() && !profile.sites.is_empty() {
                for site in std::mem::take(&mut profile.sites) {
                    profile.accounts.entry(site).or_default().push(Account {
                        username: None,
                        ..Default::default()
                    });
                }
            }
            cfg.profiles.insert(stem, profile);
        }
    });
}

/// Resolve preset by explicit name or auto-match URL pattern.
pub fn resolve_preset(url: &str, preset_name: Option<&str>, config: &Config) -> Option<Preset> {
    if let Some(name) = preset_name {
        return config.presets.get(name).cloned();
    }
    // Auto-match by pattern contains
    config
        .presets
        .values()
        .find(|preset| {
            preset
                .pattern
                .as_ref()
                .is_some_and(|pat| url.contains(pat.as_str()))
        })
        .cloned()
}

/// Save config to XDG path with 0o600.
pub fn save(cfg: &Config) -> anyhow::Result<()> {
    let Some(path) = config_path() else {
        anyhow::bail!("cannot determine config path");
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create config dir")?;
        fs::restrict_perms(parent, true);
    }
    let content = toml::to_string_pretty(cfg).context("serialize config")?;
    write_config_file(&path, &content)?;
    clear_cache();
    Ok(())
}

/// Load, mutate in place, and persist the main config.
pub fn update(f: impl FnOnce(&mut Config)) -> anyhow::Result<()> {
    let mut cfg = load().unwrap_or_default();
    f(&mut cfg);
    save(&cfg)
}

/// Config file path (XDG)
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/config.toml"))
}

/// Presets directory
pub fn presets_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/presets"))
}

pub fn presets_sites_dir() -> Option<PathBuf> {
    presets_dir().map(|p| p.join("sites"))
}

pub fn presets_persons_dir() -> Option<PathBuf> {
    presets_dir().map(|p| p.join("persons"))
}

pub fn expand_output_dir(path: &Path) -> PathBuf {
    expand_tilde(path)
}

/// One-time migration from the pre-rename XDG directories
/// (`scarpmf` -> `scrapmf`). Idempotent: if the new directory already exists,
/// the legacy one is left untouched; nothing is ever deleted.
pub fn migrate_legacy_dirs() {
    if let Some(cfg) = dirs::config_dir() {
        migrate_one(&cfg.join("scarpmf"), &cfg.join("scrapmf"));
    }
    if let Some(data) = dirs::data_dir() {
        migrate_one(&data.join("scarpmf"), &data.join("scrapmf"));
    }
}

fn migrate_one(old: &Path, new: &Path) {
    if !old.is_dir() || new.exists() {
        return;
    }
    match std::fs::rename(old, new) {
        Ok(()) => tracing::info!(
            from = %old.display(),
            to = %new.display(),
            "migrated legacy data directory to new app name"
        ),
        Err(e) => tracing::warn!(
            error = %e,
            from = %old.display(),
            to = %new.display(),
            "could not migrate legacy data directory"
        ),
    }
}

/// Ensure base config directories exist with 0o700 (idempotent).
pub fn ensure_config_dirs() -> anyhow::Result<()> {
    if let Some(base) = dirs::config_dir().map(|p| p.join("scrapmf")) {
        std::fs::create_dir_all(&base).context("create scrapmf config dir")?;
        crate::config::fs::restrict_perms(&base, true);
    }
    // tasks_dir() belongs here too: it is the only reason `tasks/*.toml` can be
    // written at all, since the atomic writer does not create parents.
    for d in [sites_dir(), profiles_dir(), tasks_dir()]
        .into_iter()
        .flatten()
    {
        std::fs::create_dir_all(&d).with_context(|| format!("create {}", d.display()))?;
        crate::config::fs::restrict_perms(&d, true);
    }
    Ok(())
}

/// Ensure ~/.config/scrapmf/config.toml exists with 0o600 (no clobber).
pub fn ensure_default_config() -> anyhow::Result<()> {
    let Some(path) = config_path() else {
        return Ok(());
    };
    if path.exists() {
        // Migrate old default output_dir that resolves to $HOME (e.g. "~", "~/", "/home/tester")
        // to "~/scrapmf" — writing directly to $HOME would pollute it.
        if let Ok(s) = std::fs::read_to_string(&path)
            && let Ok(cfg) = toml::from_str::<crate::config::Config>(&s)
        {
            let expanded = expand_tilde(&cfg.general.output_dir);
            if let Some(home) = dirs::home_dir()
                && (expanded == home
                    || s.contains("output_dir = \"~\"")
                        && !s.contains("output_dir = \"~/scrapmf\""))
            {
                let new_cfg = Config {
                    general: General {
                        output_dir: std::path::PathBuf::from("~/scrapmf"),
                        archive: cfg.general.archive,
                        cookies_from_browser: cfg.general.cookies_from_browser.clone(),
                    },
                    backend: cfg.backend.clone(),
                    ..Default::default()
                };
                if let Ok(body) = toml::to_string_pretty(&new_cfg) {
                    let header = r#"# scrapmf — main config
# XDG: ~/.config/scrapmf/config.toml (0o600, dir 0o700)
# This file is the global defaults. Site and profile files override it.
#
# [general]
#   output_dir = "~/scrapmf"                    # base output dir (HOME/scrapmf; CLI --output overrides; tilde ~/ expanded)
#   archive = true                              # download archive (dedup per-account)
# See: sites/*.toml for per-site options

"#;
                    let content = format!("{header}{body}");
                    match write_config_file(&path, &content) {
                        Ok(()) => tracing::info!(
                            path = %path.display(),
                            "migrated output_dir that resolved to $HOME -> \"~/scrapmf\""
                        ),
                        Err(e) => tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "could not migrate output_dir"
                        ),
                    }
                }
            }
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create config parent")?;
        crate::config::fs::restrict_perms(parent, true);
    }
    let cfg = Config::default();
    let body = toml::to_string_pretty(&cfg).context("serialize default config")?;
    let header = r#"# scrapmf — main config
# XDG: ~/.config/scrapmf/config.toml (0o600, dir 0o700)
# This file is the global defaults. Site and profile files override it.
#
# [general]
#   output_dir = "~/scrapmf"                    # base output dir (HOME/scrapmf; CLI --output overrides; tilde ~/ expanded)
#   archive = true                              # download archive (dedup per-account)
# See: sites/*.toml for per-site options

"#;
    let content = format!("{header}{body}");
    write_config_file(&path, &content)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod legacy_migration_tests {
    use super::migrate_one;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn migrates_legacy_dir_when_new_missing() {
        let base = TempDir::new().expect("tempdir");
        let old = base.path().join("scarpmf");
        let new = base.path().join("scrapmf");
        std::fs::create_dir_all(old.join("sites")).expect("seed");
        std::fs::write(old.join("config.toml"), "x = 1").expect("seed file");

        migrate_one(&old, &new);

        assert!(!old.exists(), "legacy dir should be moved, not copied");
        assert!(new.join("config.toml").is_file());
        assert!(new.join("sites").is_dir());
    }

    #[test]
    fn no_touch_when_new_already_exists() {
        let base = TempDir::new().expect("tempdir");
        let old = base.path().join("scarpmf");
        let new = base.path().join("scrapmf");
        std::fs::create_dir_all(&old).expect("seed old");
        std::fs::write(old.join("old.toml"), "o").expect("seed");
        std::fs::create_dir_all(&new).expect("seed new");
        std::fs::write(new.join("new.toml"), "n").expect("seed");

        migrate_one(&old, &new);

        // Both survive untouched
        assert!(old.exists());
        assert!(Path::new(&new).join("new.toml").is_file());
        assert!(!Path::new(&new).join("old.toml").exists());
    }

    #[test]
    fn no_op_when_nothing_exists() {
        let base = TempDir::new().expect("tempdir");
        migrate_one(&base.path().join("ghost"), &base.path().join("target"));
        assert!(!base.path().join("target").exists());
    }
}
