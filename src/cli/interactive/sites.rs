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
    crate::cli::interactive::menu::notice("Cookie profiles", lines);
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
        let outcome = Browser::new("Configuration")
            .entry(
                "Cookie profiles",
                vec![
                    "Named cookie sets captured from your browser.".to_string(),
                    "Used by accounts to access private content.".to_string(),
                ],
            )
            .entry("Manage Sites", {
                let mut d = vec!["Site configs (editable with $EDITOR):".to_string()];
                if sites.is_empty() {
                    d.push("  (none found)".to_string());
                }
                d.extend(sites.iter().map(|s| format!("  · {s}")));
                d
            })
            .entry(
                "Manage Profiles",
                vec![format!("{profiles_count} saved scraping profile(s).")],
            )
            .entry(
                "General settings",
                vec![
                    format!(
                        "output dir : {}",
                        crate::config::expand_output_dir(&cfg.general.output_dir).display()
                    ),
                    format!(
                        "archive    : {}",
                        if cfg.general.archive { "on" } else { "off" }
                    ),
                ],
            )
            .entry("Back", vec!["Return to the main menu.".to_string()])
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
    let (ready, missing): (Vec<_>, Vec<_>) = detected.iter().partition(|b| b.available);

    const NONE: &str = "None — do not use browser cookies";
    let mut opts: Vec<(String, Vec<String>)> = Vec::new();
    let mut ids: Vec<Option<String>> = Vec::new();

    for b in &ready {
        let mut detail = vec![format!("id: {}", b.id)];
        if let Some(db) = &b.cookie_db {
            let size = std::fs::metadata(db).map(|m| m.len() / 1024).unwrap_or(0);
            let mtime = cookie_db_mtime(db);
            if mtime.is_empty() {
                detail.push(format!("cookie DB ({size} KB)"));
            } else {
                detail.push(format!("cookie DB ({size} KB), updated {mtime}"));
            }
        }
        if let Some(p) = &b.profile {
            detail.push(format!("profile: {p}"));
        }
        // Which account this actually is. For private content this is the only
        // detail that matters — two browsers logged into two accounts behave
        // differently and nothing said which one was being sent.
        match crate::config::cookies::instagram_account_for_channel(b.id) {
            Some(acct) => detail.push(format!("Instagram account: {acct}")),
            None => {
                detail.push("Instagram account: unknown — captured on the first scrape".to_string())
            }
        }
        if current == Some(b.id) {
            detail.push("← currently selected".to_string());
        }
        opts.push((b.display.clone(), detail));
        ids.push(Some(b.id.to_string()));
    }
    for b in &missing {
        opts.push((
            format!("{} (not usable)", b.display),
            vec![
                format!("id: {}", b.id),
                "browser binary or cookie database not found".to_string(),
            ],
        ));
        ids.push(Some(b.id.to_string()));
    }
    opts.push((
        NONE.to_string(),
        vec!["scrape anonymously; no session is sent".to_string()],
    ));
    ids.push(None);

    let idx =
        match crate::cli::interactive::menu::pick_single("Configuration ─ Browser cookies", opts)
        {
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
            let label = chosen.clone().unwrap_or_else(|| "none (anonymous)".into());
            notice_in(
                "Configuration ─ General settings",
                &[format!("✔ browser cookies: {label} → {cfg_path}")],
            );
        }
        Err(e) => notice_in(
            "Configuration ─ General settings",
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
            "enabled"
        } else {
            "disabled"
        };
        let browser_label = match cfg.general.cookies_from_browser.as_deref() {
            Some(id) => crate::browsers::resolve_channel(id)
                .map(|c| c.display.to_string())
                .unwrap_or_else(|| format!("{id} (not installed)")),
            None => "not set — anonymous".to_string(),
        };
        let options: Vec<String> = vec![
            format!("Output directory: {}", cfg.general.output_dir.display()),
            format!("Browser cookies: {browser_label}"),
            format!("Download archive: {archive_label}"),
            "Show config path".to_string(),
            "Edit raw config.toml ($EDITOR)".to_string(),
            "Back".to_string(),
        ];
        let opts: Vec<(String, Vec<String>)> = options
            .iter()
            .map(|o| {
                let details = if o.starts_with("Output directory:") {
                    vec![format!("current: {}", cfg.general.output_dir.display())]
                } else if o.starts_with("Browser cookies:") {
                    vec![match cfg.general.cookies_from_browser.as_deref() {
                        Some(id) => format!("currently: {id} — used for every site"),
                        None => "currently: none — every site scrapes anonymously".to_string(),
                    }]
                } else if o.starts_with("Download archive:") {
                    vec![format!("archive is {archive_label}")]
                } else {
                    Vec::new()
                };
                (o.clone(), details)
            })
            .collect();
        let idx = match crate::cli::interactive::menu::pick_single(
            "Configuration ─ General settings",
            opts,
        ) {
            crate::cli::interactive::menu::Step::Value(i) => i,
            _ => return,
        };
        let choice = options[idx].clone();
        if choice.starts_with("Output directory:") {
            let current = cfg.general.output_dir.display().to_string();
            let Ok(new_val) = Text::new("Output directory:")
                .with_default(&current)
                .with_help_message("e.g. ~/scrapmf, ~/Pictures/scrapmf — tilde ~/ is expanded")
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
                    "Configuration ─ General settings",
                    &[format!("✖ invalid path: {e}")],
                );
                continue;
            }
            let expanded = crate::config::expand_output_dir(&new_path);
            if let Some(home) = dirs::home_dir()
                && expanded == home
            {
                notice_in(
                    "Configuration ─ General settings",
                    &["✖ output directory cannot be $HOME itself — use a subdirectory like ~/scrapmf"
                        .to_string()],
                );
                continue;
            }
            let mut new_cfg = cfg.clone();
            new_cfg.general.output_dir = new_path;
            match crate::config::save(&new_cfg) {
                Ok(()) => notice_in(
                    "Configuration ─ General settings",
                    &[format!("✔ saved → {cfg_path}")],
                ),
                Err(e) => notice_in(
                    "Configuration ─ General settings",
                    &[format!("✖ save failed: {e}")],
                ),
            }
        } else if choice.starts_with("Browser cookies:") {
            browser_cookies_menu(cfg.general.cookies_from_browser.as_deref());
        } else if choice.starts_with("Download archive:") {
            let new_val = !cfg.general.archive;
            let label = if new_val { "enabled" } else { "disabled" };
            let prompt = format!("Turn download archive {label}?");
            let ok = crate::cli::interactive::menu::confirm_back(
                "Configuration ─ General settings",
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
                    "Configuration ─ General settings",
                    &[format!("✔ archive {label} → {cfg_path}")],
                ),
                Err(e) => notice_in(
                    "Configuration ─ General settings",
                    &[format!("✖ save failed: {e}")],
                ),
            }
        } else if choice == "Show config path" {
            notice_in(
                "Configuration ─ General settings",
                std::slice::from_ref(&cfg_path),
            );
        } else if choice == "Edit raw config.toml ($EDITOR)" {
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
    if present.is_empty() {
        return crate::browsers::CHANNELS
            .iter()
            .map(|c| c.display.to_string())
            .collect();
    }
    present.iter().map(|c| c.display.to_string()).collect()
}

/// Wizard: pick browser + networks, capture their session cookies into a
/// new named profile. Firefox uses direct SQLite reads; Chromium-family
/// browsers (Brave/Chrome/…) use keyring-based decryption.
fn create_profile_wizard() {
    use crate::config::cookies;
    let browsers = capture_browser_options();
    let b_idx = match crate::cli::interactive::menu::pick_single(
        "Configuration ─ Cookie capture ─ Browser",
        browsers
            .iter()
            .map(|b| (b.clone(), vec![format!("capture from {b}")]))
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
        let mut v = vec![("All networks".to_string(), vec!["select all".to_string()])];
        v.extend(networks.iter().map(|s| {
            (
                super::theme::brand_site_label(s),
                vec![format!("domain: {}", s)],
            )
        }));
        v
    };
    let picked_idxs = match crate::cli::interactive::menu::pick_multi(
        "Configuration ─ Cookie capture ─ Networks",
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
        notice(&["ℹ No networks selected".to_string()]);
        return;
    }
    // Labels carry ANSI; map back to clean keys via brand_site_label output.
    let mut sites: Vec<String> = Vec::new();
    for picked in &picked_raw {
        for net in &networks {
            if super::theme::brand_site_label(net) == *picked || picked == "All networks" {
                sites.push((*net).to_string());
                break;
            }
        }
    }
    sites.sort();
    sites.dedup();

    let suggested = format!("{}-{}", browser.to_lowercase(), sites.join("-"));
    let Some(name) = Text::new("Profile name:")
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
            format!("✔ {count} cookie(s) → {}", path.display()),
            "⚠ Cookie files grant account access — never share them.".to_string(),
        ]),
        Err(e) => notice(&[format!("✖ {e}")]),
    }
}

fn cookie_profiles_menu() {
    use crate::config::cookies;
    loop {
        clear_screen();
        let count = cookies::list_profiles().len();
        let browse_label = if count == 0 {
            "Browse profiles".to_string()
        } else {
            format!("Browse profiles ({count})")
        };
        let choices: Vec<String> = vec![
            browse_label,
            "Create profile — capture from browser".into(),
            "Import profile — from file or paste".into(),
            "Back".into(),
        ];
        let choice = match select_menu("Cookie profiles", choices).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        if choice.starts_with("Browse profiles") {
            browse_profiles_menu();
        } else if choice.starts_with("Create profile") {
            create_profile_wizard();
        } else if choice.starts_with("Import profile") {
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
            notice(&["ℹ No profiles yet — use Create or Import first".to_string()]);
            return;
        }
        let rows: Vec<String> = names
            .iter()
            .map(|name| match cookies::profile_summary(name) {
                Ok(summary) => format!("{name}  — {summary}"),
                Err(e) => format!("{name}  — ⚠ {e}"),
            })
            .collect();
        let Ok(picked_row) = select_menu("Browse profiles — select one", rows).prompt() else {
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
            "Back".into(),
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
                    "Cookie profiles",
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
            "Back".into(),
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
    let src_expanded = if let Some(stripped) = src.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(stripped)
        } else {
            src_path.to_path_buf()
        }
    } else {
        src_path.to_path_buf()
    };
    let content = match std::fs::read_to_string(&src_expanded) {
        Ok(c) => c,
        Err(e) => {
            notice_in(
                "Cookie profiles",
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
            "Back".to_string(),
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
                "Manage Sites",
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
        options.push(SiteItem::new("Back"));

        let choice = match select_menu("Manage Sites", options).prompt() {
            Ok(c) => c.key(),
            Err(_) => return,
        };
        if choice == "Back" {
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
        options.push("Back".to_string());

        // Strip the count suffix so a bare entry name can be matched
        let selected = match select_menu("Manage Profiles", options).prompt() {
            Ok(c) => c.split(" — ").next().unwrap_or(&c).to_string(),
            Err(_) => return,
        };
        if selected == "Back" {
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
