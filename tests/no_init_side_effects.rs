//! Regression tests for the "no side effects before arg parsing" fix:
//! `--version` / `--help` must not create or modify anything under
//! `$XDG_CONFIG_HOME`, while a real command still initializes config.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use tempfile::TempDir;

fn run_with_clean_xdg(args: &[&str]) -> (std::process::ExitStatus, TempDir) {
    let xdg = TempDir::new().expect("tempdir");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_scrapmf"))
        .args(args)
        .env("XDG_CONFIG_HOME", xdg.path())
        .env_remove("SCRAPMF_NO_INIT")
        .status()
        .expect("run binary");
    (status, xdg)
}

fn entries(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn version_does_not_touch_config_dir() {
    let (_status, xdg) = run_with_clean_xdg(&["--version"]);
    assert!(
        entries(xdg.path()).is_empty(),
        "--version created files in XDG_CONFIG_HOME: {:?}",
        entries(xdg.path())
    );
}

#[test]
fn help_does_not_touch_config_dir() {
    let (_status, xdg) = run_with_clean_xdg(&["--help"]);
    assert!(
        entries(xdg.path()).is_empty(),
        "--help created files in XDG_CONFIG_HOME: {:?}",
        entries(xdg.path())
    );
}

#[test]
fn doctor_initializes_config() {
    let (status, xdg) = run_with_clean_xdg(&["doctor"]);
    // doctor may fail if gallery-dl is absent; we only care about init side effects
    let _ = status;
    assert!(
        Path::new(&xdg.path().join("scrapmf/config.toml")).exists(),
        "doctor should initialize ~/.config/scrapmf/config.toml, found: {:?}",
        entries(xdg.path())
    );
}
