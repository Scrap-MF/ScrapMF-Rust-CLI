#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::path::PathBuf;
use tempfile::TempDir;

use scrapmf::config::{self, Config, Preset};

#[test]
fn resolve_preset_explicit() {
    let mut cfg = Config::default();
    cfg.presets.insert(
        "pixiv".to_string(),
        Preset {
            pattern: Some("pixiv.net".to_string()),
            output_dir: Some(PathBuf::from("/tmp/pixiv")),
            ..Default::default()
        },
    );
    let p = config::resolve_preset("https://example.com", Some("pixiv"), &cfg).expect("preset");
    assert_eq!(p.pattern.as_deref(), Some("pixiv.net"));
}

#[test]
fn resolve_preset_auto_match() {
    let mut cfg = Config::default();
    cfg.presets.insert(
        "pixiv".to_string(),
        Preset {
            pattern: Some("pixiv.net".to_string()),
            ..Default::default()
        },
    );
    let p = config::resolve_preset("https://www.pixiv.net/artworks/123", None, &cfg).expect("auto");
    assert_eq!(p.pattern.as_deref(), Some("pixiv.net"));
    assert!(config::resolve_preset("https://example.com", None, &cfg).is_none());
}

#[test]
#[allow(clippy::unwrap_used, clippy::expect_used)]
fn layered_presets_override_no_env() {
    // Test layered logic without env var by directly using file operations
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("config.toml");
    std::fs::write(
        &cfg_path,
        r#"
            [presets.inline]
            pattern = "example.com"
            output_dir = "/tmp/inline"
        "#,
    )
    .expect("write config");
    let cfg: Config =
        toml::from_str(&std::fs::read_to_string(&cfg_path).expect("read")).expect("parse");
    assert_eq!(
        cfg.presets
            .get("inline")
            .unwrap()
            .output_dir
            .as_ref()
            .unwrap(),
        &PathBuf::from("/tmp/inline")
    );
    // Simulate layered file override
    let mut cfg2 = cfg.clone();
    let preset: Preset = toml::from_str(
        r#"pattern = "example.com"
output_dir = "/tmp/layered""#,
    )
    .expect("parse preset");
    cfg2.presets.insert("inline".to_string(), preset);
    assert_eq!(
        cfg2.presets
            .get("inline")
            .unwrap()
            .output_dir
            .as_ref()
            .unwrap(),
        &PathBuf::from("/tmp/layered")
    );
}

#[test]
fn load_default_when_no_file_no_env() {
    let cfg = Config::default();
    assert_eq!(cfg.general.output_dir, PathBuf::from("~/scrapmf"));
    assert!(cfg.presets.is_empty());
}
