//! "Plugins" top-level submenu — pick a plugin, then manage it
//! (update/disable/remove). Thin UI over [`crate::plugins`] logic, rendered
//! from the plugin registry so future plugins slot in without new menus.

use crate::cli::interactive::{clear_screen, select_menu};
use crate::plugins::{self, PluginDef, PluginState, REGISTRY, THREADSTRACTOR_PIN};

/// Entry point from the main menu.
pub(super) fn menu() {
    loop {
        clear_screen();
        let mut options: Vec<String> = REGISTRY
            .iter()
            .map(|p| format!("{}  ·  {}", plugin_line(p), status_short(p.id)))
            .collect();
        options.push("Back".to_string());
        let choice = match select_menu("Plugins:", options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        if choice == "Back" {
            return;
        }
        // Match back by title (labels are stable registry data).
        if let Some(def) = REGISTRY.iter().find(|p| choice.starts_with(p.title)) {
            plugin_submenu(def);
        }
    }
}

fn plugin_submenu(def: &PluginDef) {
    if def.id == "termux-scan" {
        termux_scan_submenu(def);
        return;
    }
    let state = state_for(def);
    loop {
        clear_screen();
        let status = match &state {
            PluginState::NotInstalled => "not installed".to_string(),
            PluginState::Disabled => "disabled (files kept)".to_string(),
            PluginState::Enabled(v) => format!("enabled ({v})"),
        };
        println!("── {} by {} ──", def.title, def.vendor);
        println!("   {status}");
        println!();

        let mut options: Vec<String> = Vec::new();
        match state {
            PluginState::NotInstalled => options.push(format!(
                "Enable — install at pin {THREADSTRACTOR_PIN} (~150MB Chromium download)"
            )),
            PluginState::Disabled => {
                options.push("Enable".to_string());
                options.push("Remove (deletes all files)".to_string());
            }
            PluginState::Enabled(_) => {
                options.push(format!("Update / reinstall at pin {THREADSTRACTOR_PIN}"));
                options.push("Disable (hide, keep files)".to_string());
                options.push("Remove (deletes all files)".to_string());
            }
        }
        options.push("Back".to_string());

        let choice = match select_menu("Action:", options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "Back" => return,
            c if c.starts_with("Enable") || c.starts_with("Update / reinstall") => {
                if let Err(e) = plugins::install() {
                    eprintln!("✖ install failed: {e}");
                } else if matches!(state, PluginState::NotInstalled | PluginState::Disabled) {
                    println!("✔ {} by {} enabled", def.title, def.vendor);
                }
                pause();
            }
            "Disable (hide, keep files)" => match plugins::set_disabled(true) {
                Ok(()) => println!("✔ disabled — options hidden, files kept"),
                Err(e) => eprintln!("✖ failed: {e}"),
            },
            "Remove (deletes all files)" => match plugins::remove() {
                Ok(()) => println!("✔ removed completely"),
                Err(e) => eprintln!("✖ remove failed: {e}"),
            },
            _ => {}
        }
    }
}

fn termux_scan_submenu(def: &PluginDef) {
    loop {
        clear_screen();
        let state = crate::plugins::termux_scan_state();
        let status = match &state {
            PluginState::NotInstalled => "not installed (termux-media-scan not found)".to_string(),
            PluginState::Disabled => "disabled (opt-in, enable to scan gallery)".to_string(),
            PluginState::Enabled(v) => format!("enabled ({v})"),
        };
        println!("── {} by {} ──", def.title, def.vendor);
        println!("   {status}");
        println!("   Runs termux-media-scan <scrapmf_dir> once per finished download");
        println!("   Requires termux:api app + pkg install termux-api (Termux only)");
        println!();

        let mut options: Vec<String> = Vec::new();
        match state {
            PluginState::NotInstalled => {
                options.push("Enable (requires termux-media-scan on PATH)".to_string());
                options.push("Help — how to install termux-api".to_string());
            }
            PluginState::Disabled => {
                options.push("Enable".to_string());
                options.push("Help — how to install termux-api".to_string());
            }
            PluginState::Enabled(_) => {
                options.push("Disable".to_string());
                options.push("Test scan now (scans ~/scrapmf)".to_string());
            }
        }
        options.push("Back".to_string());

        let choice = match select_menu("Action:", options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "Back" => return,
            c if c.starts_with("Enable") => match crate::plugins::set_termux_scan_disabled(false) {
                Ok(()) => {
                    if matches!(crate::plugins::termux_scan_state(), PluginState::Enabled(_)) {
                        println!("✔ Termux MediaScan enabled");
                    } else {
                        println!(
                            "✔ enabled but termux-media-scan not found — install termux:api app + pkg install termux-api, then test"
                        );
                    }
                    pause();
                    return;
                }
                Err(e) => eprintln!("✖ failed: {e}"),
            },
            "Disable" => match crate::plugins::set_termux_scan_disabled(true) {
                Ok(()) => {
                    println!("✔ disabled");
                    pause();
                    return;
                }
                Err(e) => eprintln!("✖ failed: {e}"),
            },
            c if c.starts_with("Test scan") => {
                let dir = crate::config::load()
                    .map(|c| crate::config::expand_output_dir(&c.general.output_dir))
                    .unwrap_or_else(|_| std::path::PathBuf::from("~/scrapmf"));
                let expanded = crate::config::expand_output_dir(&dir);
                println!("→ scanning {}", expanded.display());
                crate::application::media_scan::maybe_scan(&expanded);
                println!("✔ scan command sent (check Android Gallery)");
                pause();
            }
            c if c.starts_with("Help") => {
                println!("  Install on Termux:");
                println!("    1. Install Termux:API app from F-Droid");
                println!("    2. pkg install termux-api");
                println!("    3. termux-media-scan --help  (should print usage)");
                println!("    4. Enable this plugin, then next download will auto-scan");
                pause();
            }
            _ => {}
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Lifecycle state for a registry entry.
fn state_for(def: &PluginDef) -> PluginState {
    match def.id {
        "threads" => plugins::threads_state(),
        "termux-scan" => plugins::termux_scan_state(),
        _ => PluginState::NotInstalled,
    }
}

/// Short status for the plugin list line.
fn status_short(id: &str) -> String {
    match state_for_id(id) {
        PluginState::NotInstalled => "not installed".to_string(),
        PluginState::Disabled => "disabled".to_string(),
        PluginState::Enabled(v) => format!("enabled {v}"),
    }
}

fn state_for_id(id: &str) -> PluginState {
    match id {
        "threads" => plugins::threads_state(),
        "termux-scan" => plugins::termux_scan_state(),
        _ => PluginState::NotInstalled,
    }
}

/// "ThreadstractorMF by MFApplications"
fn plugin_line(def: &PluginDef) -> String {
    format!("{} by {}", def.title, def.vendor)
}

fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(1500));
}
