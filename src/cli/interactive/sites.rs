use super::profiles::{edit_profile_menu, prompt_new_profile_accounts};
use super::{
    ask_nonempty, clear_screen, edit_with_editor, select_menu, select_menu_with_details,
    theme::render_config,
};
use std::path::Path;

use inquire::Text;

/// Show a result inside the Browser chrome and wait for acknowledgement.
///
/// The menus here draw on the alternate screen, so a `println!` issued between
/// two of them was covered the moment the next box appeared — the action
/// looked like it had done nothing. This renders in the same chrome as every
/// other prompt.
fn notice(lines: &[String]) {
    crate::cli::interactive::menu::notice(&crate::i18n::t("cookie_profiles"), lines);
}

/// As [`notice`], for the settings and site menus.
fn notice_in(context: &str, lines: &[String]) {
    crate::cli::interactive::menu::notice(context, lines);
}

pub(super) fn configuration_submenu() {
    use crate::cli::interactive::browser::{Browser, Outcome};

    // Live data for the right pane
    let sites: Vec<String> = crate::config::sites_dir()
        .and_then(|dir| std::fs::read_dir(&dir).ok())
        .map(|rd| {
            let mut v: Vec<String> = rd
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .map(String::from)
                })
                .collect();
            // plugin gating mirrors the menus that consume these entries
            if !crate::plugins::threads_enabled() {
                v.retain(|n| n != "threads");
            }
            v.sort();
            v
        })
        .unwrap_or_default();
    let profiles_count = crate::config::profiles_dir()
        .and_then(|dir| std::fs::read_dir(&dir).ok())
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    let cfg = crate::config::load().unwrap_or_default();

    loop {
        let outcome = Browser::new(crate::i18n::t("configuration"))
            .entry(
                crate::i18n::t("cookie_profiles"),
                vec![
                    crate::i18n::t("named_cookie_sets_captured"),
                    crate::i18n::t("used_by_accounts_private_content"),
                ],
            )
            .entry(crate::i18n::t("manage_sites"), {
                let mut d = vec![crate::i18n::t("site_configs_editable_editor")];
                if sites.is_empty() {
                    d.push(crate::i18n::t("none_found_indented"));
                }
                d.extend(sites.iter().map(|s| format!("  · {s}")));
                d
            })
            .entry(
                crate::i18n::t("manage_profiles"),
                vec![crate::i18n::tf(
                    "saved_scraping_profiles_count",
                    &[("count", &profiles_count.to_string())],
                )],
            )
            .entry(
                crate::i18n::t("general_settings"),
                vec![
                    crate::i18n::tf(
                        "output_dir_colon",
                        &[(
                            "path",
                            &crate::config::expand_output_dir(&cfg.general.output_dir)
                                .display()
                                .to_string(),
                        )],
                    ),
                    {
                        let st = if cfg.general.archive {
                            crate::i18n::t("on")
                        } else {
                            crate::i18n::t("off")
                        };
                        crate::i18n::tf("archive_colon", &[("state", &st)])
                    },
                ],
            )
            .entry(
                crate::i18n::t("back"),
                vec![crate::i18n::t("return_to_main_menu").to_string()],
            )
            .run();

        let choice = match outcome {
            Outcome::Picked(i) => i,
            _ => {
                clear_screen();
                return;
            }
        };
        match choice {
            0 => {
                cookie_profiles_menu();
                clear_screen();
            }
            1 => {
                manage_sites();
                clear_screen();
            }
            2 => {
                manage_profiles();
                clear_screen();
            }
            3 => {
                general_settings_menu();
                clear_screen();
            }
            _ => {
                clear_screen();
                return;
            }
        }
    }
}

/// `YYYY-MM-DD` for a file's modification time, or an empty string.
///
/// The date matters here more than usual: Chrome-family channels each use their
/// own profile directory, so a machine with two installs of the same browser can
/// show an untouched-but-present database next to the live one. The size alone
/// does not reveal which is current.
fn cookie_db_mtime(path: &std::path::Path) -> String {
    let Ok(md) = std::fs::metadata(path) else {
        return String::new();
    };
    let Ok(mtime) = md.modified() else {
        return String::new();
    };
    let secs = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // civil_from_days, same algorithm the run-log stamp uses.
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Pick the one browser every site takes its session from.
///
/// Reuses the same channel list and menu shape as the cookie-capture wizard, so
/// it is the same gesture in the same place — but it also shows each cookie
/// database's size and modification date. That date is the whole point: two
/// installs of the same browser each keep their own profile directory, and a
/// stale one still looks perfectly valid on disk.
fn browser_cookies_menu(current: Option<&str>) {
    let detected = crate::providers::browser::detect_available_browsers();
    // Usable channels first; the rest stay listed but flagged, so a browser
    // that is installed in an unusual way is still selectable.
    let ready: Vec<_> = detected.iter().filter(|b| b.available).collect();

    let none_label = crate::i18n::t("none_no_browser_cookies");
    let mut opts: Vec<(String, Vec<String>)> = Vec::new();
    let mut ids: Vec<Option<String>> = Vec::new();

    for b in &ready {
        let mut detail = vec![crate::i18n::tf("id_colon", &[("id", b.id)])];
        if let Some(db) = &b.cookie_db {
            let size = std::fs::metadata(db).map(|m| m.len() / 1024).unwrap_or(0);
            let mtime = cookie_db_mtime(db);
            if mtime.is_empty() {
                detail.push(crate::i18n::tf(
                    "cookie_db_kb",
                    &[("size", &size.to_string())],
                ));
            } else {
                detail.push(crate::i18n::tf(
                    "cookie_db_kb_updated",
                    &[("size", &size.to_string()), ("mtime", &mtime)],
                ));
            }
        }
        if let Some(p) = &b.profile {
            detail.push(crate::i18n::tf("profile_colon", &[("profile", p)]));
        }
        // Instagram account hint intentionally omitted here: it only made
        // sense for Instagram runs and leaked the session's account id into
        // the browser-selection UI. The hint still surfaces on a failing
        // Instagram scrape via session_hint().
        if current == Some(b.id) {
            detail.push(crate::i18n::t("currently_selected_arrow"));
        }
        opts.push((b.display.clone(), detail));
        ids.push(Some(b.id.to_string()));
    }
    opts.push((
        none_label.clone(),
        vec![crate::i18n::t("scrape_anonymously_no_session")],
    ));
    ids.push(None);

    let idx = match crate::cli::interactive::menu::pick_single(
        &crate::i18n::t("config_browser_cookies"),
        opts,
    ) {
        crate::cli::interactive::menu::Step::Value(i) => i,
        _ => return,
    };
    let Some(chosen) = ids.get(idx).cloned() else {
        return;
    };

    let mut cfg = crate::config::load().unwrap_or_default();
    cfg.general.cookies_from_browser = chosen.clone();
    let cfg_path = crate::config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    match crate::config::save(&cfg) {
        Ok(()) => {
            let label = chosen
                .clone()
                .unwrap_or_else(|| crate::i18n::t("none_anonymous"));
            notice_in(
                &crate::i18n::t("config_general_settings"),
                &[crate::i18n::tf(
                    "browser_cookies_saved",
                    &[("label", &label), ("path", &cfg_path)],
                )],
            );
        }
        Err(e) => notice_in(
            &crate::i18n::t("config_general_settings"),
            &[format!("✖ save failed: {e}")],
        ),
    }
}

fn general_settings_menu() {
    loop {
        let cfg = crate::config::load().unwrap_or_default();
        let cfg_path = crate::config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let archive_label = if cfg.general.archive {
            crate::i18n::t("enabled")
        } else {
            crate::i18n::t("disabled")
        };
        let browser_label = match cfg.general.cookies_from_browser.as_deref() {
            Some(id) => crate::browsers::resolve_channel(id)
                .map(|c| c.display.to_string())
                .unwrap_or_else(|| crate::i18n::tf("not_installed_paren", &[("id", id)])),
            None => crate::i18n::t("not_set_anonymous"),
        };
        let options: Vec<String> = vec![
            format!(
                "{}: {}",
                crate::i18n::t("output_directory"),
                cfg.general.output_dir.display()
            ),
            format!("{}: {browser_label}", crate::i18n::t("browser_cookies")),
            format!("{}: {archive_label}", crate::i18n::t("download_archive")),
            format!("{}: {}", crate::i18n::t("language"), cfg.general.language),
            crate::i18n::t("show_config_path").to_string(),
            crate::i18n::t("edit_raw_config").to_string(),
            crate::i18n::t("back").to_string(),
        ];
        let opts: Vec<(String, Vec<String>)> = options
            .iter()
            .map(|o| {
                let details = if o.starts_with(&format!("{}:", crate::i18n::t("output_directory")))
                {
                    vec![crate::i18n::tf(
                        "current_colon",
                        &[("value", &cfg.general.output_dir.display().to_string())],
                    )]
                } else if o.starts_with(&format!("{}:", crate::i18n::t("browser_cookies"))) {
                    vec![match cfg.general.cookies_from_browser.as_deref() {
                        Some(id) => crate::i18n::tf("currently_used_every_site", &[("id", id)]),
                        None => crate::i18n::t("currently_none_anonymous"),
                    }]
                } else if o.starts_with(&format!("{}:", crate::i18n::t("download_archive"))) {
                    vec![crate::i18n::tf("archive_is", &[("label", &archive_label)])]
                } else if o.starts_with(&format!("{}:", crate::i18n::t("language"))) {
                    vec![crate::i18n::t("en_or_es")]
                } else {
                    Vec::new()
                };
                (o.clone(), details)
            })
            .collect();
        let idx = match crate::cli::interactive::menu::pick_single(
            &crate::i18n::t("config_general_settings"),
            opts,
        ) {
            crate::cli::interactive::menu::Step::Value(i) => i,
            _ => return,
        };
        let choice = options[idx].clone();
        if choice.starts_with(&format!("{}:", crate::i18n::t("output_directory"))) {
            let current = cfg.general.output_dir.display().to_string();
            let Ok(new_val) = Text::new(&crate::i18n::t("output_directory"))
                .with_default(&current)
                .with_help_message(&crate::i18n::t("help_output_dir_example"))
                .prompt()
            else {
                continue;
            };
            let new_val = new_val.trim().to_string();
            if new_val.is_empty() || new_val == current {
                continue;
            }
            let new_path = std::path::PathBuf::from(&new_val);
            if let Err(e) = crate::application::scraper::validate_output_path(&new_path) {
                notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::tf("invalid_path_x", &[("e", &e.to_string())])],
                );
                continue;
            }
            let expanded = crate::config::expand_output_dir(&new_path);
            if let Some(home) = dirs::home_dir()
                && expanded == home
            {
                notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::t("output_dir_cannot_be_home")],
                );
                continue;
            }
            let mut new_cfg = cfg.clone();
            new_cfg.general.output_dir = new_path;
            match crate::config::save(&new_cfg) {
                Ok(()) => notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::tf("saved_to_path", &[("path", &cfg_path)])],
                ),
                Err(e) => notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::tf("save_failed_x", &[("e", &e.to_string())])],
                ),
            }
        } else if choice.starts_with(&format!("{}:", crate::i18n::t("browser_cookies"))) {
            browser_cookies_menu(cfg.general.cookies_from_browser.as_deref());
        } else if choice.starts_with(&format!("{}:", crate::i18n::t("download_archive"))) {
            let new_val = !cfg.general.archive;
            let label = if new_val {
                crate::i18n::t("enabled")
            } else {
                crate::i18n::t("disabled")
            };
            let prompt = crate::i18n::tf("turn_download_archive", &[("label", &label)]);
            let ok = crate::cli::interactive::menu::confirm_back(
                &crate::i18n::t("config_general_settings"),
                &prompt,
                true,
                false,
            )
            .value()
            .unwrap_or(false);
            if !ok {
                continue;
            }
            let mut new_cfg = cfg.clone();
            new_cfg.general.archive = new_val;
            match crate::config::save(&new_cfg) {
                Ok(()) => notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::tf(
                        "archive_saved",
                        &[("label", &label), ("path", &cfg_path)],
                    )],
                ),
                Err(e) => notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::tf("save_failed_x", &[("e", &e.to_string())])],
                ),
            }
        } else if choice.starts_with(&format!("{}:", crate::i18n::t("language"))) {
            let langs = ["en".to_string(), "es".to_string()];
            let idx = match crate::cli::interactive::menu::pick_single(
                &crate::i18n::t("config_general_settings_language"),
                langs
                    .iter()
                    .map(|l| {
                        (
                            l.clone(),
                            vec![crate::i18n::tf("set_language_to", &[("lang", l)])],
                        )
                    })
                    .collect(),
            ) {
                crate::cli::interactive::menu::Step::Value(i) => i,
                _ => continue,
            };
            let new_lang = langs[idx].clone();
            let mut new_cfg = cfg.clone();
            new_cfg.general.language = new_lang.clone();
            match crate::config::save(&new_cfg) {
                Ok(()) => notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::tf(
                        "language_saved",
                        &[("lang", &new_lang), ("path", &cfg_path)],
                    )],
                ),
                Err(e) => notice_in(
                    &crate::i18n::t("config_general_settings"),
                    &[crate::i18n::tf("save_failed_x", &[("e", &e.to_string())])],
                ),
            }
        } else if choice == crate::i18n::t("show_config_path") {
            notice_in(
                &crate::i18n::t("config_general_settings"),
                std::slice::from_ref(&cfg_path),
            );
        } else if choice == crate::i18n::t("edit_raw_config") {
            if let Some(p) = crate::config::config_path() {
                // Ensure file exists before opening editor
                let _ = crate::config::ensure_default_config();
                edit_with_editor(&p);
            }
        } else {
            return;
        }
    }
}

/// Manage named cookie profiles: capture from Firefox, import pasted
/// Netscape text, list with live summaries, delete.
/// Browser labels offered by the capture wizard, most relevant first.
///
/// Only channels that are actually installed *or* have a cookie database on
/// this machine are listed, so a Brave Origin user is offered "Brave Origin"
/// instead of a list dominated by browsers they do not use. The previous
/// version hardcoded seven labels and omitted every Brave channel except
/// stable, which is why an Origin user could only ever be pointed at the
/// abandoned `Brave-Browser` profile.
fn capture_browser_options() -> Vec<String> {
    let present: Vec<&crate::browsers::BrowserChannel> = crate::browsers::CHANNELS
        .iter()
        .filter(|c| {
            crate::browsers::binary_for(c).is_some()
                || crate::browsers::find_cookie_db(c, "Cookies").is_some()
                || crate::browsers::find_cookie_db(c, "cookies.sqlite").is_some()
        })
        .collect();
    // Only offer browsers that are actually present on this machine; never
    // fall back to the full channel list, which made every browser look
    // selectable even when it was not installed.
    present.iter().map(|c| c.display.to_string()).collect()
}

/// Wizard: pick browser + networks, capture their session cookies into a
/// new named profile. Firefox uses direct SQLite reads; Chromium-family
/// browsers (Brave/Chrome/…) use keyring-based decryption.
fn create_profile_wizard() {
    use crate::config::cookies;
    let browsers = capture_browser_options();
    if browsers.is_empty() {
        println!("{}", crate::i18n::t("no_browsers_detected"));
        return;
    }
    let b_idx = match crate::cli::interactive::menu::pick_single(
        &crate::i18n::t("config_cookie_capture_browser"),
        browsers
            .iter()
            .map(|b| {
                (
                    b.clone(),
                    vec![crate::i18n::tf("capture_from", &[("browser", b)])],
                )
            })
            .collect(),
    ) {
        crate::cli::interactive::menu::Step::Value(i) => i,
        _ => return,
    };
    let browser = browsers[b_idx].clone();

    // Network multi-select with an "All" shortcut. Plugin-backed networks
    // (threads) only appear while their plugin is enabled.
    let mut networks: Vec<&str> = vec![
        "instagram",
        "tiktok",
        "twitter",
        "vsco",
        // Facebook was missing here even though `domains_for_site` supports it,
        // so its cookies were unreachable from the wizard.
        "facebook",
    ];
    if crate::plugins::threads_enabled() {
        networks.push("threads");
    }
    let net_opts: Vec<(String, Vec<String>)> = {
        let mut v = vec![(
            crate::i18n::t("all_networks"),
            vec![crate::i18n::t("select_all")],
        )];
        v.extend(networks.iter().map(|s| {
            (
                super::theme::brand_site_label(s),
                vec![crate::i18n::tf("domain_colon", &[("domain", s)])],
            )
        }));
        v
    };
    let picked_idxs = match crate::cli::interactive::menu::pick_multi(
        &crate::i18n::t("config_cookie_capture_networks"),
        net_opts.clone(),
        &[],
    ) {
        crate::cli::interactive::menu::Step::Value(v) => v,
        _ => return,
    };
    let picked_raw: Vec<String> = picked_idxs
        .into_iter()
        .filter_map(|i| net_opts.get(i).map(|(l, _)| l.clone()))
        .collect();
    if picked_raw.is_empty() {
        notice(&[crate::i18n::t("no_networks_selected")]);
        return;
    }
    // Labels carry ANSI; map back to clean keys via brand_site_label output.
    let mut sites: Vec<String> = Vec::new();
    for picked in &picked_raw {
        for net in &networks {
            if super::theme::brand_site_label(net) == *picked
                || *picked == crate::i18n::t("all_networks")
            {
                sites.push((*net).to_string());
                break;
            }
        }
    }
    sites.sort();
    sites.dedup();

    let suggested = format!("{}-{}", browser.to_lowercase(), sites.join("-"));
    let Some(name) = Text::new(&crate::i18n::t("profile_name"))
        .with_default(&suggested)
        .prompt()
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return;
    };

    let result = if browser.eq_ignore_ascii_case("firefox") {
        cookies::capture_firefox(&sites, &name)
    } else {
        cookies::capture_chromium(&browser, &sites, &name)
    };
    match result {
        Ok((path, count)) => notice(&[
            crate::i18n::tf(
                "cookies_captured_count",
                &[
                    ("count", &count.to_string()),
                    ("path", &path.display().to_string()),
                ],
            ),
            crate::i18n::t("cookie_files_grant_access_warning"),
        ]),
        Err(e) => notice(&[crate::i18n::tf("error_x", &[("e", &e.to_string())])]),
    }
}

fn cookie_profiles_menu() {
    use crate::config::cookies;
    loop {
        clear_screen();
        let count = cookies::list_profiles().len();
        let browse_label = if count == 0 {
            crate::i18n::t("browse_profiles")
        } else {
            crate::i18n::tf("browse_profiles_count", &[("count", &count.to_string())])
        };
        let choices: Vec<String> = vec![
            browse_label,
            crate::i18n::t("create_profile_capture_browser"),
            crate::i18n::t("import_profile_from_file_paste"),
            crate::i18n::t("back"),
        ];
        let choice = match select_menu(&crate::i18n::t("cookie_profiles"), choices).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        if choice.starts_with(&crate::i18n::t("browse_profiles")) {
            browse_profiles_menu();
        } else if choice.starts_with(&crate::i18n::t("create_profile_capture_browser")) {
            create_profile_wizard();
        } else if choice.starts_with(&crate::i18n::t("import_profile_from_file_paste")) {
            import_profile_submenu();
        } else {
            return;
        }
    }
}

fn browse_profiles_menu() {
    use crate::config::cookies;
    loop {
        let names = cookies::list_profiles();
        if names.is_empty() {
            notice(&[crate::i18n::t("no_profiles_yet_create_import")]);
            return;
        }
        let rows: Vec<String> = names
            .iter()
            .map(|name| match cookies::profile_summary(name) {
                Ok(summary) => format!("{name}  — {summary}"),
                Err(e) => format!("{name}  — ⚠ {e}"),
            })
            .collect();
        let Ok(picked_row) =
            select_menu(&crate::i18n::t("browse_profiles_select_one"), rows).prompt()
        else {
            return;
        };
        let name = picked_row.split("  — ").next().unwrap_or("").to_string();
        if !name.is_empty() {
            profile_detail_menu(&name);
        }
    }
}

fn profile_detail_menu(name: &str) {
    use crate::config::cookies;
    loop {
        // Header rendered inside the chrome: printed with `println!` it was
        // covered by the menu that opened next, so the summary and path of
        // the selected profile were never actually shown.
        let summary = match cookies::profile_summary(name) {
            Ok(s) => s,
            Err(e) => format!("⚠ {e}"),
        };
        let path = cookies::profile_path(name)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let choices: Vec<String> = vec![
            "View details".into(),
            "Refresh — re-capture from browser".into(),
            "Delete profile".into(),
            crate::i18n::t("back"),
        ];
        let choice = match select_menu_with_details(
            &format!("Profile: {name}"),
            vec![summary.clone(), path.clone()],
            choices,
        )
        .prompt()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "View details" => {
                let mut lines = vec![summary.to_string(), format!("File: {path}")];
                if let Some(p) = cookies::profile_path(name)
                    && let Ok(content) = std::fs::read_to_string(p)
                {
                    match cookies::parse_source_metadata(&content) {
                        Some(meta) => lines.push(format!(
                            "Source: {} — networks: {}",
                            meta.browser,
                            meta.networks.join(", ")
                        )),
                        None => {
                            lines.push("Source: manual import (no browser metadata)".to_string());
                        }
                    }
                }
                notice(&lines);
            }
            "Refresh — re-capture from browser" => {
                let result = cookies::refresh_profile(name);
                match result {
                    Ok(cookies::Refresh::Done { path, count }) => notice(&[
                        format!("✔ refreshed {count} cookie(s) → {}", path.display()),
                        "✔ accounts using this profile need no changes".to_string(),
                    ]),
                    Ok(cookies::Refresh::ManualImportRequired) => notice(&[
                        "ℹ This profile was imported manually — it has no browser to re-capture from."
                            .to_string(),
                        "  Re-export with 'Get cookies.txt LOCALLY' while logged in, then use"
                            .to_string(),
                        "  Import profile → From file / Paste.".to_string(),
                    ]),
                    Err(e) => notice(&[
                        format!("✖ refresh failed: {e}"),
                        "  tip: is the browser closed? Try again, or re-create with Create profile"
                            .to_string(),
                    ]),
                }
            }
            "Delete profile" => {
                // Destructive: Enter keeps the profile, `y` deletes it.
                let ok = crate::cli::interactive::menu::confirm_back(
                    &crate::i18n::t("cookie_profiles"),
                    &format!("Delete profile '{name}'?"),
                    false,
                    true,
                )
                .value()
                .unwrap_or(false);
                if !ok {
                    // Previously nothing was printed here and the menu simply
                    // redrew, so declining a destructive action looked exactly
                    // like the button being dead.
                    notice(&[format!("kept '{name}' — nothing was deleted")]);
                    continue;
                }
                match cookies::delete_profile(name) {
                    Ok(true) => {
                        notice(&[format!("✔ deleted '{name}'")]);
                        return;
                    }
                    Ok(false) => {
                        notice(&[
                            format!("✖ '{name}' no longer exists"),
                            "  it may have been deleted from another screen".to_string(),
                        ]);
                    }
                    Err(e) => {
                        notice(&[format!("✖ delete failed: {e}")]);
                    }
                }
            }
            _ => return,
        }
    }
}

fn import_profile_submenu() {
    import_profile_submenu_prefilled("")
}

fn import_profile_submenu_prefilled(default_name: &str) {
    loop {
        clear_screen();
        let choices: Vec<String> = vec![
            "Import from file — select cookies.txt".into(),
            "Import from paste — open $EDITOR".into(),
            crate::i18n::t("back"),
        ];
        let choice = match select_menu("Import profile", choices).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "Import from file — select cookies.txt" => {
                import_from_file_flow(default_name);
            }
            "Import from paste — open $EDITOR" => {
                import_from_paste_flow(default_name);
            }
            _ => return,
        }
    }
}

fn import_from_file_flow(default_name: &str) {
    use crate::config::cookies;
    let Some(name) = (if default_name.is_empty() {
        ask_nonempty("Profile name:")
    } else {
        let input = Text::new("Profile name:")
            .with_default(default_name)
            .prompt()
            .ok()
            .map(|s| s.trim().to_string());
        input.filter(|s| !s.is_empty())
    }) else {
        return;
    };
    let Some(src) = Text::new("Path to cookies.txt file:")
        .with_help_message("e.g. /tmp/cookies.txt or ~/Downloads/cookies.txt")
        .prompt()
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let src_path = Path::new(&src);
    let src_expanded = crate::util::expand_tilde(src_path);
    let content = match std::fs::read_to_string(&src_expanded) {
        Ok(c) => c,
        Err(e) => {
            notice_in(
                &crate::i18n::t("cookie_profiles"),
                &[format!("✖ cannot read {}: {e}", src_expanded.display())],
            );
            return;
        }
    };
    match cookies::save_profile(&name, &content) {
        Ok(path) => {
            let count = cookies::load_profile(&name).map(|v| v.len()).unwrap_or(0);
            notice(&[format!("✔ imported {count} cookie(s) → {}", path.display())]);
        }
        Err(e) => notice(&[
            format!("✖ invalid cookies file: {e}"),
            "  Expected Netscape format (7 tab-separated fields).".to_string(),
        ]),
    }
}

fn import_from_paste_flow(default_name: &str) {
    use crate::config::cookies;
    let Some(name) = (if default_name.is_empty() {
        ask_nonempty("Profile name:")
    } else {
        let input = Text::new("Profile name:")
            .with_default(default_name)
            .prompt()
            .ok()
            .map(|s| s.trim().to_string());
        input.filter(|s| !s.is_empty())
    }) else {
        return;
    };
    let path = match cookies::profile_path(&name) {
        Some(p) => p,
        None => return,
    };
    let template = "# Netscape HTTP Cookie File\n# Paste your exported cookies below.\n# Get them with the 'Get cookies.txt LOCALLY' extension while logged in.\n# 1. Install the extension in your browser\n# 2. Log in to the site (instagram.com, tiktok.com, etc.)\n# 3. Click the extension → Export → Copy\n# 4. Paste below this header, save and close the editor.\n\n";
    // Only write template if file doesn't exist, to avoid overwriting existing profile
    if !path.exists() {
        let _ = std::fs::write(&path, template);
    } else {
        // Ensure header exists for guidance
        if let Ok(existing) = std::fs::read_to_string(&path)
            && !existing.contains("Netscape HTTP Cookie File")
        {
            let _ = std::fs::write(&path, format!("{template}{existing}"));
        }
    }
    edit_with_editor(&path);
    match cookies::load_profile(&name) {
        Ok(list) => notice_in("Profiles", &[format!("✔ saved {} cookie(s)", list.len())]),
        Err(e) => notice_in(
            "Profiles",
            &[format!("✖ invalid profile ({e}) — delete or fix {path:?}")],
        ),
    }
}

pub(super) fn site_submenu(name: &str, dir: &Path) {
    let path = dir.join(format!("{name}.toml"));
    loop {
        clear_screen();
        let options: Vec<String> = vec![
            "Edit".to_string(),
            "Delete site".to_string(),
            crate::i18n::t("back").to_string(),
        ];
        let choice = match select_menu(&format!("Site: {name}"), options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        if choice == "Edit" {
            if path.exists() {
                edit_with_editor(&path);
            } else {
                eprintln!("error: {name}.toml no longer exists");
                return;
            }
        } else if choice == "Delete site" {
            // Destructive: Enter keeps the file, `y` deletes it.
            if crate::cli::interactive::menu::confirm_back(
                &crate::i18n::t("manage_sites"),
                &format!("Delete {name}.toml?"),
                false,
                true,
            )
            .value()
            .unwrap_or(false)
            {
                let _ = std::fs::remove_file(&path);
                notice_in("Sites", &[format!("✔ Deleted {name}")]);
                return;
            }
        } else {
            return;
        }
    }
}

pub(super) fn manage_sites() {
    let dir = match crate::config::sites_dir() {
        Some(d) => d,
        None => {
            eprintln!("error: cannot determine sites dir");
            return;
        }
    };
    let _ = std::fs::create_dir_all(&dir);
    loop {
        clear_screen();
        let mut entries: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if let Some(name) = e.path().file_stem().and_then(|n| n.to_str())
                    && e.path().extension().is_some_and(|ext| ext == "toml")
                {
                    entries.push(name.to_string());
                }
            }
        }
        entries.sort();
        // Plugin gating: plugin-backed sites only show when enabled.
        if !crate::plugins::threads_enabled() {
            entries.retain(|n| n != "threads");
        }
        use crate::cli::interactive::theme::SiteItem;
        let mut options: Vec<SiteItem> = entries.iter().map(|e| SiteItem::new(e.clone())).collect();
        options.push(SiteItem::new(crate::i18n::t("back")));

        let choice = match select_menu(&crate::i18n::t("manage_sites"), options).prompt() {
            Ok(c) => c.key(),
            Err(_) => return,
        };
        if choice == crate::i18n::t("back") {
            return;
        }
        if entries.contains(&choice) {
            // Selected an existing site — open its submenu
            site_submenu(&choice, &dir);
        }
    }
}

pub(super) fn manage_profiles() {
    let dir = match crate::config::profiles_dir() {
        Some(d) => d,
        None => {
            eprintln!("error: cannot determine profiles dir");
            return;
        }
    };
    let _ = std::fs::create_dir_all(&dir);
    loop {
        clear_screen();
        let mut entries: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if let Some(name) = e.path().file_stem().and_then(|n| n.to_str())
                    && e.path().extension().is_some_and(|ext| ext == "toml")
                {
                    entries.push(name.to_string());
                }
            }
        }
        entries.sort();
        let mut options: Vec<String> = Vec::new();
        options.push("Create new profile".to_string());
        // Entries show account counts inline (replaces the old "List all")
        for n in &entries {
            let p = dir.join(format!("{n}.toml"));
            let suffix = match std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| toml::from_str::<crate::config::Profile>(&s).ok())
            {
                Some(prof) => {
                    let total: usize = prof.accounts.values().map(|v| v.len()).sum();
                    format!(" — {} site(s), {} account(s)", prof.accounts.len(), total)
                }
                None => " — parse error".to_string(),
            };
            options.push(format!("{n}{suffix}"));
        }
        options.push(crate::i18n::t("back").to_string());

        // Strip the count suffix so a bare entry name can be matched
        let selected = match select_menu(&crate::i18n::t("manage_profiles"), options).prompt() {
            Ok(c) => c.split(" — ").next().unwrap_or(&c).to_string(),
            Err(_) => return,
        };
        if selected == crate::i18n::t("back") {
            return;
        }
        if selected == "Create new profile" {
            let name = match Text::new("Profile id (filename without .toml):")
                .with_render_config(render_config())
                .with_placeholder("example_person")
                .prompt()
            {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };
            if name.is_empty() || name.contains('/') || name.contains('.') {
                eprintln!("error: invalid name");
                continue;
            }
            let path = dir.join(format!("{name}.toml"));
            if path.exists() {
                eprintln!("error: {name}.toml already exists, choose Edit");
                continue;
            }
            // Guided account creation — TOML pre-filled with real values
            let profile = prompt_new_profile_accounts(&name);
            match crate::config::write_profile_file(&path, &profile) {
                Ok(()) => notice_in("Profiles", &[format!("✔ Created {}", path.display())]),
                Err(e) => {
                    notice_in("Profiles", &[format!("✖ write profile: {e}")]);
                    continue;
                }
            }
            // Continue with the menu-driven edit flow
            edit_profile_menu(&name, &path);
        } else if entries.contains(&selected) {
            // Selected an existing profile — open its submenu
            let path = dir.join(format!("{selected}.toml"));
            edit_profile_menu(&selected, &path);
        }
    }
}
