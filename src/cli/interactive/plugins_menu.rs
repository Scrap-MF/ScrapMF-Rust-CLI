//! crate::i18n::t("plugins") top-level submenu — pick a plugin, then manage it
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
        options.push(crate::i18n::t("back").to_string());
        let choice = match select_menu(&format!("{}:", crate::i18n::t("plugins")), options).prompt()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        if choice == crate::i18n::t("back") {
            return;
        }
        // Match back by title (labels are stable registry data).
        if let Some(def) = REGISTRY.iter().find(|p| choice.starts_with(p.title)) {
            plugin_submenu(def);
        }
    }
}

fn plugin_submenu(def: &PluginDef) {
    if def.id == crate::i18n::t("termux_scan") {
        termux_scan_submenu(def);
        return;
    }
    let state = state_for(def);
    loop {
        clear_screen();
        let status = match &state {
            PluginState::NotInstalled => crate::i18n::t("not_installed"),
            PluginState::Disabled => crate::i18n::t("disabled_files_kept"),
            PluginState::Enabled(v) => crate::i18n::tf("enabled_paren", &[("v", v)]),
        };
        println!(
            "{}",
            crate::i18n::tf(
                "plugin_header",
                &[("title", def.title), ("vendor", def.vendor)]
            )
        );
        println!("   {status}");
        println!();

        let mut options: Vec<String> = Vec::new();
        match state {
            PluginState::NotInstalled => options.push(crate::i18n::tf(
                "enable_install_pin",
                &[("pin", THREADSTRACTOR_PIN)],
            )),
            PluginState::Disabled => {
                options.push(crate::i18n::t("enable").to_string());
                options.push(crate::i18n::t("remove_deletes_all_files").to_string());
            }
            PluginState::Enabled(_) => {
                options.push(crate::i18n::tf(
                    "update_reinstall_pin",
                    &[("pin", THREADSTRACTOR_PIN)],
                ));
                options.push(crate::i18n::t("disable_hide_keep_files").to_string());
                options.push(crate::i18n::t("remove_deletes_all_files").to_string());
            }
        }
        options.push("Back".to_string());

        let choice = match select_menu(&crate::i18n::t("action_prompt"), options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            s if s == crate::i18n::t("back") => return,
            c if c.starts_with(&crate::i18n::t("enable"))
                || c.starts_with(&crate::i18n::t("update_reinstall_pin")) =>
            {
                let res = match def.id {
                    "threads" => plugins::install(),
                    "tiktok" => plugins::install_tiktok(),
                    _ => Ok(()),
                };
                if let Err(e) = res {
                    eprintln!(
                        "{}",
                        crate::i18n::tf("install_failed_x", &[("e", &e.to_string())])
                    );
                } else if matches!(state, PluginState::NotInstalled | PluginState::Disabled) {
                    println!(
                        "{}",
                        crate::i18n::tf(
                            "plugin_enabled_by",
                            &[("title", def.title), ("vendor", def.vendor)]
                        )
                    );
                }
                pause();
            }
            s if s == crate::i18n::t("disable_hide_keep_files") => {
                let res = match def.id {
                    "threads" => plugins::set_disabled(true),
                    "tiktok" => plugins::set_tiktok_disabled(true),
                    _ => Ok(()),
                };
                match res {
                    Ok(()) => println!("{}", crate::i18n::t("disabled_options_hidden_files_kept")),
                    Err(e) => {
                        eprintln!("{}", crate::i18n::tf("failed_x", &[("e", &e.to_string())]))
                    }
                }
            }
            s if s == crate::i18n::t("remove_deletes_all_files") => {
                let res = match def.id {
                    "threads" => plugins::remove(),
                    "tiktok" => plugins::remove_tiktok(),
                    _ => Ok(()),
                };
                match res {
                    Ok(()) => println!("{}", crate::i18n::t("removed_completely")),
                    Err(e) => eprintln!(
                        "{}",
                        crate::i18n::tf("remove_failed_x", &[("e", &e.to_string())])
                    ),
                }
            }
            _ => {}
        }
    }
}

fn termux_scan_submenu(def: &PluginDef) {
    loop {
        clear_screen();
        let state = crate::plugins::termux_scan_state();
        let status = match &state {
            PluginState::NotInstalled => {
                crate::i18n::t("not_installed_termux_media_scan_not_found")
            }
            PluginState::Disabled => crate::i18n::t("disabled_opt_in_enable_scan_gallery"),
            PluginState::Enabled(v) => crate::i18n::tf("enabled_paren", &[("v", v)]),
        };
        println!(
            "{}",
            crate::i18n::tf(
                "plugin_header",
                &[("title", def.title), ("vendor", def.vendor)]
            )
        );
        println!("   {status}");
        println!("{}", crate::i18n::t("runs_termux_media_scan_output_dir"));
        println!("{}", crate::i18n::t("requires_termux_api"));
        println!();

        let mut options: Vec<String> = Vec::new();
        match state {
            PluginState::NotInstalled => {
                options.push(crate::i18n::t("enable_requires_termux_media_scan_path").to_string());
                options.push(crate::i18n::t("help_install_termux_api").to_string());
            }
            PluginState::Disabled => {
                options.push(crate::i18n::t("enable").to_string());
                options.push(crate::i18n::t("help_install_termux_api").to_string());
            }
            PluginState::Enabled(_) => {
                options.push(crate::i18n::t("disable").to_string());
                options.push(crate::i18n::t("test_scan_now").to_string());
            }
        }
        options.push("Back".to_string());

        let choice = match select_menu("Action:", options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            s if s == crate::i18n::t("back") => return,
            c if c.starts_with(&crate::i18n::t("enable")) => {
                match crate::plugins::set_termux_scan_disabled(false) {
                    Ok(()) => {
                        if matches!(crate::plugins::termux_scan_state(), PluginState::Enabled(_)) {
                            println!("{}", crate::i18n::t("termux_mediascan_enabled"));
                        } else {
                            println!(
                                "{}",
                                crate::i18n::t("enabled_but_termux_media_scan_not_found")
                            );
                        }
                        pause();
                        return;
                    }
                    Err(e) => {
                        eprintln!("{}", crate::i18n::tf("failed_x", &[("e", &e.to_string())]))
                    }
                }
            }
            s if s == crate::i18n::t("disable") => {
                match crate::plugins::set_termux_scan_disabled(true) {
                    Ok(()) => {
                        println!("{}", crate::i18n::t("disabled_simple"));
                        pause();
                        return;
                    }
                    Err(e) => {
                        eprintln!("{}", crate::i18n::tf("failed_x", &[("e", &e.to_string())]))
                    }
                }
            }
            c if c.starts_with(&crate::i18n::t("test_scan_now")) => {
                // Scan the configured output dir — never a hardcoded fallback.
                let dir = match crate::config::load() {
                    Ok(c) => crate::config::expand_output_dir(&c.general.output_dir),
                    Err(e) => {
                        eprintln!(
                            "{}",
                            crate::i18n::tf("cannot_read_config_x", &[("e", &e.to_string())])
                        );
                        pause();
                        continue;
                    }
                };
                // `maybe_scan` reports the path it scanned and every failure
                // mode itself; only the toggle needs saying here, since being
                // disabled is deliberately silent there.
                match crate::application::media_scan::maybe_scan(&dir, None) {
                    crate::application::media_scan::ScanOutcome::Disabled => {
                        println!(
                            "{}",
                            crate::i18n::t("termux_mediascan_disabled_enable_first")
                        );
                    }
                    crate::application::media_scan::ScanOutcome::Done => {
                        println!("{}", crate::i18n::t("check_android_gallery"));
                    }
                    _ => {}
                }
                pause();
            }
            c if c.starts_with(&crate::i18n::t("help")) => {
                println!("{}", crate::i18n::t("install_on_termux"));
                println!("{}", crate::i18n::t("termux_help_step1"));
                println!("{}", crate::i18n::t("termux_help_step2"));
                println!("{}", crate::i18n::t("termux_help_step3"));
                println!("{}", crate::i18n::t("termux_help_step4"));
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
        "tiktok" => plugins::tiktok_state(),
        "termux-scan" => plugins::termux_scan_state(),
        _ => PluginState::NotInstalled,
    }
}

/// Short status for the plugin list line.
fn status_short(id: &str) -> String {
    match state_for_id(id) {
        PluginState::NotInstalled => crate::i18n::t("not_installed"),
        PluginState::Disabled => crate::i18n::t("disabled"),
        PluginState::Enabled(v) => crate::i18n::tf("enabled_value", &[("v", &v)]),
    }
}

fn state_for_id(id: &str) -> PluginState {
    match id {
        "threads" => plugins::threads_state(),
        "tiktok" => plugins::tiktok_state(),
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
