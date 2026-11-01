use std::path::{Path, PathBuf};

use inquire::Text;

use crate::application::scraper::{ScrapeRequest, validate_url};
use crate::config;

use super::content::{
    ContentKind, account_labels, build_tagged_urls, content_options, kinds_description,
    prompt_content_kinds, resolve_kinds, select_urls, shortcut_applicable, site_has_content_menu,
};
use super::menu::Step;
use super::scrape_flow::{preview_and_execute, site_options_with_fallbacks};
use super::{
    ask_nonempty, clear_screen,
    content::validate_kind_selection,
    edit_with_editor, select_menu,
    theme::{brand_account_label, render_config},
};
/// Outcome of the content-selection tree: the kinds per account, or a way out.
///
/// `Back` is separate from `Leave` so a caller can re-ask the account list
/// instead of dropping the run the way Ctrl+C should.
enum ContentPick {
    Kinds(Vec<Vec<ContentKind>>),
    Back,
    Leave,
}

/// Content selection — decision tree:
///   all non-menu sites (generic) → auto [Posts], no prompts
///   single account → direct content prompt, no shortcut confirm
///   2+ same menu site (instagram/tiktok) → shortcut confirm applies one choice to all
///   mixed sites → per-account prompts with each site's own menu
fn choose_content_kinds(selected: &[(String, String, crate::config::Account)]) -> ContentPick {
    let mut per_account_kinds: Vec<Vec<ContentKind>> = Vec::with_capacity(selected.len());

    if selected.iter().all(|(s, _, _)| !site_has_content_menu(s)) {
        // No account has a content menu — everything is just Posts
        for _ in 0..selected.len() {
            per_account_kinds.push(vec![ContentKind::Posts]);
        }
    } else if !shortcut_applicable(selected) {
        // Single menu-capable account or mixed sites: prompt per account.
        // Esc steps back to the account list, which is where a wrong choice
        // most often needs fixing.
        for (site, label, _acc) in selected {
            if site_has_content_menu(site) {
                match prompt_content_kinds(site, label) {
                    Step::Value(k) if k.is_empty() => {
                        println!("{}", crate::i18n::t("no_content_selected"));
                        return ContentPick::Leave;
                    }
                    Step::Value(k) => per_account_kinds.push(k),
                    // Esc steps back to the account list, which is where a
                    // wrong choice most often needs fixing.
                    Step::Back => return ContentPick::Back,
                    Step::Cancel => return ContentPick::Leave,
                }
            } else {
                per_account_kinds.push(vec![ContentKind::Posts]);
            }
        }
    } else {
        // 2+ accounts of the same menu-capable site — offer the shortcut
        let site = &selected[0].0.clone();
        let options = content_options(site);
        // Enter says yes: applying one choice to every account is the expected
        // action, and nothing is destroyed here.
        let same_for_all = match crate::cli::interactive::menu::confirm_back(
            &crate::i18n::t("profile_add_account"),
            &crate::i18n::t("apply_same_content_all_accounts"),
            true,
            false,
        ) {
            Step::Value(v) => v,
            // Esc on the shortcut goes back to the account list, consistent
            // with the per-account branch below.
            Step::Back => return ContentPick::Back,
            Step::Cancel => return ContentPick::Leave,
        };
        if same_for_all {
            let opts: Vec<(String, Vec<String>)> = options
                .iter()
                .map(|s| (s.to_string(), Vec::new()))
                .collect();
            let idxs = match crate::cli::interactive::menu::pick_multi_back(
                &crate::i18n::t("content_types_for_all_accounts"),
                opts.clone(),
                &[],
            ) {
                Step::Value(v) => v,
                // Esc steps back to the account list, consistent with the
                // per-account branch below.
                Step::Back => return ContentPick::Back,
                Step::Cancel => return ContentPick::Leave,
            };
            if idxs.is_empty() {
                return ContentPick::Leave;
            }
            let picked: Vec<String> = idxs
                .into_iter()
                .filter_map(|i| opts.get(i).map(|(l, _)| l.clone()))
                .collect();
            if let Err(msg) = validate_kind_selection(&picked) {
                eprintln!("{msg}");
                std::thread::sleep(std::time::Duration::from_millis(800));
                return ContentPick::Leave;
            }
            let kinds: Vec<ContentKind> = resolve_kinds(site, &picked);
            for _ in 0..selected.len() {
                per_account_kinds.push(kinds.clone());
            }
        } else {
            for (_site, label, _acc) in selected {
                match prompt_content_kinds(site, label) {
                    Step::Value(k) if k.is_empty() => {
                        println!("{}", crate::i18n::t("no_content_selected"));
                        return ContentPick::Leave;
                    }
                    Step::Value(k) => per_account_kinds.push(k),
                    Step::Back => return ContentPick::Back,
                    Step::Cancel => return ContentPick::Leave,
                }
            }
        }
    }
    ContentPick::Kinds(per_account_kinds)
}

pub(super) fn prompt_scrape_as_profile() {
    let cfg = config::load().unwrap_or_default();
    // Hide example templates from selection
    let mut profile_names: Vec<String> = cfg
        .profiles
        .keys()
        .filter(|k| *k != "example_person" && *k != "ejemplo_persona")
        .cloned()
        .collect();
    profile_names.sort();
    if profile_names.is_empty() {
        println!("{}", crate::i18n::t("no_profiles_yet"));
        return;
    }
    // First question of this flow: Esc leaves, there is nothing before it.
    // One loop for the whole wizard, so Esc walks back a question at a time
    // instead of abandoning the run: preview → content → accounts → profile.
    'wizard: loop {
        let idx = match crate::cli::interactive::menu::pick_single(
            crate::i18n::t("choose_profile").as_str(),
            profile_names
                .iter()
                .map(|n| {
                    (
                        n.clone(),
                        vec![crate::i18n::tf("profile_label_detail", &[("profile", n)])],
                    )
                })
                .collect(),
        ) {
            Step::Value(i) => i,
            // First question of this flow: Esc leaves.
            Step::Back | Step::Cancel => return,
        };
        let profile_choice = profile_names[idx].clone();
        let Some(profile) = cfg.profiles.get(&profile_choice).cloned() else {
            return;
        };
        // Collect sites for this profile
        let mut available_sites: Vec<String> = profile.accounts.keys().cloned().collect();
        available_sites.sort();
        if available_sites.is_empty() {
            println!(
                "{}",
                crate::i18n::tf("profile_no_accounts", &[("profile", &profile_choice)])
            );
            return;
        }

        // Filter to sites that have a site config? warn if missing but still allow
        let sites_dir = crate::config::sites_dir();
        let mut missing_sites = Vec::new();
        for s in &available_sites {
            if let Some(ref dir) = sites_dir {
                let path = dir.join(format!("{s}.toml"));
                if !path.exists() {
                    missing_sites.push(s.clone());
                }
            }
        }
        if !missing_sites.is_empty() {
            println!(
                "{}",
                crate::i18n::tf(
                    "sites_not_configured",
                    &[("sites", &missing_sites.join(", "))]
                )
            );
        }

        // Flatten all accounts into unique {site}:{username} labels (parallel index mapping)
        let mut flat: Vec<(String, crate::config::Account)> = Vec::new(); // (site, account)
        let mut account_options: Vec<String> = Vec::new(); // parallel labels
        let mut sites_sorted: Vec<&String> = profile.accounts.keys().collect();
        sites_sorted.sort();
        for site in sites_sorted {
            let list = &profile.accounts[site];
            let labels = account_labels(site, list);
            for (acc, label) in list.iter().zip(labels) {
                flat.push((site.clone(), acc.clone()));
                account_options.push(super::theme::brand_account_label(&label));
            }
        }
        if flat.is_empty() {
            println!(
                "{}",
                crate::i18n::tf("profile_no_accounts", &[("profile", &profile_choice)])
            );
            return;
        }

        // MultiSelect accounts — now via decorated Browser (box) so recuadro always present.
        let opts: Vec<(String, Vec<String>)> = account_options
            .iter()
            .map(|o| (o.clone(), vec![]))
            .collect();
        // Esc here goes back to the profile list rather than abandoning the run:
        // picking the wrong profile and stepping back is the common case.
        let idxs = match crate::cli::interactive::menu::pick_multi_back(
            &crate::i18n::t("select_accounts"),
            opts,
            &[],
        ) {
            Step::Value(v) if v.is_empty() => return,
            Step::Value(v) => v,
            Step::Back => continue 'wizard,
            Step::Cancel => return,
        };
        let picked_labels: Vec<String> = idxs
            .into_iter()
            .filter_map(|i| account_options.get(i).cloned())
            .collect();
        if picked_labels.is_empty() {
            return;
        }
        let selected: Vec<(String, String, crate::config::Account)> = picked_labels
            .into_iter()
            .filter_map(|label| {
                account_options
                    .iter()
                    .position(|o| *o == label)
                    .and_then(|idx| {
                        flat.get(idx)
                            .map(|(s, a)| (s.clone(), label.clone(), a.clone()))
                    })
            })
            .collect();
        if selected.is_empty() {
            println!("{}", crate::i18n::t("no_accounts_selected"));
            return;
        }

        // Content selection, request building and the preview form their own
        // loop: Esc on the preview returns here, to the question that decided
        // what the batch downloads, instead of restarting at the profile list.
        // The batch is rebuilt every pass because the preview consumes it.
        loop {
            let per_account_kinds = match choose_content_kinds(&selected) {
                ContentPick::Kinds(k) => k,
                // Esc steps back to the account list, which is where a wrong choice
                // most often needs fixing.
                ContentPick::Back => continue 'wizard,
                ContentPick::Leave => return,
            };

            // Build ScrapeRequests per account with only the chosen content URLs
            let mut requests: Vec<(ScrapeRequest, String, String, String)> = Vec::new(); // (req, site, username, kinds desc)
            for ((site_name, _label, account), kinds) in
                selected.iter().zip(per_account_kinds.iter())
            {
                let username = account
                    .username
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                let tagged = build_tagged_urls(site_name, &username);
                let Some((url, extra_urls)) = select_urls(&tagged, kinds) else {
                    continue;
                };
                if validate_url(&url).is_err() {
                    eprintln!(
                        "{}",
                        crate::i18n::tf("warn_skipping_invalid_url", &[("url", &url)])
                    );
                    continue;
                }
                let kinds_desc = kinds_description(site_name, kinds);
                let site_cfg = cfg.sites.get(site_name.as_str()).cloned();
                // Resolve fields: account > site > profile > general
                let cookies_from_browser: Option<String> = account
                    .cookies_from_browser
                    .clone()
                    .or_else(|| profile.cookies_from_browser.clone());
                let cookies_file: Option<PathBuf> =
                    account.cookies.clone().or_else(|| profile.cookies.clone());
                let mut archive: Option<PathBuf> = None;
                let mut rate_limit: Option<crate::config::RateLimit> = None;
                let mut extractor_options: std::collections::HashMap<String, toml::Value> =
                    std::collections::HashMap::new();
                let mut output_from_config: Option<PathBuf> = account
                    .output_dir
                    .clone()
                    .or_else(|| profile.output_dir.clone());

                if let Some(ref site) = site_cfg {
                    if archive.is_none() {
                        archive = site.archive.clone();
                    }
                    if rate_limit.is_none() {
                        rate_limit = site.rate_limit.clone();
                    }
                    if output_from_config.is_none() {
                        output_from_config = site.output_dir.clone();
                    }
                    extractor_options = site.extractor.clone();
                }
                // Cookie profiles (named Netscape files) outrank browser cookies:
                // a friend's stored session must not be silently replaced by ours.
                // Precedence: account > profile > site.
                let named = account
                    .cookie_profile
                    .as_deref()
                    .or(profile.cookie_profile.as_deref())
                    .or(site_cfg.as_ref().and_then(|s| s.cookie_profile.as_deref()));
                let (cookies_file, cookies_from_browser) = crate::config::cookies::resolve_session(
                    named,
                    cookies_file,
                    cookies_from_browser,
                    cfg.general.cookies_from_browser.clone(),
                    None,
                );
                // overrides per site from profile
                let mut filename_template: Option<String> =
                    site_cfg.as_ref().and_then(|s| s.filename_template.clone());
                let mut directory_template: Option<Vec<String>> =
                    site_cfg.as_ref().and_then(|s| s.directory_template.clone());
                if let Some(ov) = profile.overrides.get(site_name.as_str()) {
                    if let Some(ref rl) = ov.rate_limit {
                        rate_limit = Some(rl.clone());
                    }
                    if let Some(ref a) = ov.archive {
                        archive = Some(a.clone());
                    }
                    if let Some(ref ft) = ov.filename_template {
                        filename_template = Some(ft.clone());
                    }
                    if let Some(ref dt) = ov.directory_template {
                        directory_template = Some(dt.clone());
                    }
                    for (k, v) in &ov.extractor {
                        extractor_options.insert(k.clone(), v.clone());
                    }
                }

                // TikTok real filtering: selecting only Videos or only Photos sets the
                // extractor's native photos/videos options (verified in tiktok.py:
                // self.photo = config("photos", True); self.video = config("videos", True))
                if site_name == "tiktok" {
                    let wants_videos = kinds.contains(&ContentKind::Videos);
                    let wants_photos = kinds.contains(&ContentKind::Photos);
                    if wants_videos != wants_photos {
                        let posts = extractor_options
                            .entry("tiktok:posts".to_string())
                            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                        if let toml::Value::Table(map) = posts {
                            map.insert("photos".to_string(), toml::Value::Boolean(wants_photos));
                            map.insert("videos".to_string(), toml::Value::Boolean(wants_videos));
                        }
                    }
                }

                let mut extra_args: Vec<String> = Vec::new();
                if let Some(ref site) = site_cfg {
                    extra_args.extend(site.extra_args.clone());
                }
                extra_args.extend(account.extra_args.clone());

                let output = output_from_config
                    .clone()
                    .or_else(|| Some(crate::config::expand_output_dir(&cfg.general.output_dir)));

                // Twitter Media needs TWO passes (photos / videos): per-FILE conditional
                // directories don't work on twitter ({type} is only set after the
                // Directory message), so each pass pre-filters with file-filter and
                // pins a static directory. Profile URLs ride along the videos pass.
                if site_name == "twitter" && kinds.contains(&ContentKind::Media) {
                    for (pass, dir_name, filter) in [
                        ("photos", "photos", "type == 'photo'"),
                        ("videos", "videos", "type != 'photo'"),
                    ] {
                        let mut opts = extractor_options.clone();
                        let media = opts
                            .entry("twitter:media".to_string())
                            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                        if let toml::Value::Table(map) = media {
                            map.insert(
                                "directory".to_string(),
                                toml::Value::Array(vec![
                                    toml::Value::String("{scrapmf_root}".to_string()),
                                    toml::Value::String("{category}".to_string()),
                                    toml::Value::String("{user[name]}".to_string()),
                                    toml::Value::String(dir_name.to_string()),
                                ]),
                            );
                            map.insert(
                                "file-filter".to_string(),
                                toml::Value::String(filter.to_string()),
                            );
                        }
                        let pass_req = ScrapeRequest {
                            url: url.clone(),
                            output: output.clone(),
                            preset: Some(site_name.clone()),
                            extra_args: extra_args.clone(),
                            cookies_from_browser: cookies_from_browser.clone(),
                            cookies_file: cookies_file.clone(),
                            archive: archive.clone(),
                            rate_limit: rate_limit.clone(),
                            extractor_options: opts,
                            filename_template: filename_template.clone(),
                            directory_template: None,
                            // Profile URLs ride the videos pass; photos pass gets none.
                            extra_urls: if pass == "videos" {
                                extra_urls
                                    .iter()
                                    .filter(|u| !u.ends_with("/media"))
                                    .cloned()
                                    .collect()
                            } else {
                                Vec::new()
                            },
                            profile_name: Some(profile_choice.clone()),
                            extra_extractor_opts: Vec::new(),

                            ..Default::default()
                        };
                        requests.push((
                            pass_req,
                            site_name.clone(),
                            format!("{username} ({pass})"),
                            pass.to_string(),
                        ));
                    }
                    continue;
                }

                let req = ScrapeRequest {
                    url: url.clone(),
                    output,
                    preset: Some(site_name.clone()),
                    extra_args,
                    cookies_from_browser,
                    cookies_file,
                    archive,
                    rate_limit,
                    extractor_options,
                    filename_template: filename_template.clone(),
                    directory_template: directory_template.clone(),
                    extra_urls: extra_urls.clone(),
                    profile_name: Some(profile_choice.clone()),
                    extra_extractor_opts: Vec::new(),

                    ..Default::default()
                };
                requests.push((req, site_name.clone(), username, kinds_desc));
            }

            match preview_and_execute(requests, &cfg) {
                // Leaves the wizard so the "press enter" prompt below still runs.
                Step::Value(()) => break 'wizard,
                Step::Cancel => return,
                // Esc on the preview returns to the content question, which this
                // loop asks again.
                Step::Back => {}
            }
        }
    }
    let _ = Text::new(&crate::i18n::t("press_enter_to_continue"))
        .with_render_config(render_config())
        .prompt();
}

pub(super) fn prompt_new_profile_accounts(name: &str) -> crate::config::Profile {
    // Site options: configured sites first, then fallbacks, sorted
    let site_opts = site_options_with_fallbacks(&["instagram", "tiktok", "facebook"]);

    // NOTE: no cookie prompt here — cookies source lives in sites/*.toml
    // (cookies_from_browser) and is inherited via account > site precedence.

    let mut accounts: std::collections::HashMap<String, Vec<crate::config::Account>> =
        std::collections::HashMap::new();
    loop {
        clear_screen();
        let items: Vec<crate::cli::interactive::theme::SiteItem> = site_opts
            .iter()
            .cloned()
            .map(crate::cli::interactive::theme::SiteItem::new)
            .collect();
        let Ok(site) = select_menu(&crate::i18n::t("add_account_for_site"), items)
            .prompt()
            .map(|s| s.key())
        else {
            break;
        };
        let Some(username) = ask_nonempty(&crate::i18n::t("username")) else {
            break;
        };

        accounts
            .entry(site.clone())
            .or_default()
            .push(crate::config::Account {
                username: Some(username),
                display_name: None,
                cookies: None,
                cookies_from_browser: None,
                cookie_profile: None,
                output_dir: None,
                extra_args: Vec::new(),
            });

        let more = crate::cli::interactive::menu::confirm_back(
            &crate::i18n::t("profile_add_account"),
            crate::i18n::t("add_another_account").as_str(),
            false,
            false,
        )
        .value()
        .unwrap_or(false);
        if !more {
            break;
        }
    }

    crate::config::Profile {
        profile: Some(name.to_string()),
        display_name: Some(name.to_string()),
        sites: Vec::new(),
        accounts,
        output_dir: None,
        cookies: None,
        cookies_from_browser: None,
        cookie_profile: None,
        overrides: std::collections::HashMap::new(),
    }
}

/// Prompt for the account cookies source. "Default" inherits from sites/*.toml
/// (clears overrides); custom path sets `account.cookies`. Returns (cookies, browser).
pub(super) fn prompt_cookies_choice(
    current_file: Option<&Path>,
) -> (Option<PathBuf>, Option<String>) {
    let mut opts = vec![
        crate::i18n::t("default_from_site_config").to_string(),
        crate::i18n::t("profile_custom_file_path").to_string(),
    ];
    let current_desc = current_file.map(|p| p.display().to_string());
    if let Some(ref cur) = current_desc {
        opts.push(crate::i18n::tf("keep_current", &[("current", cur)]));
    }
    let choice = match select_menu(&crate::i18n::t("cookies_source"), opts).prompt() {
        Ok(c) => c,
        Err(_) => return (current_file.map(Path::to_path_buf), None),
    };
    if choice == crate::i18n::t("profile_custom_file_path") {
        if let Some(path_str) = ask_nonempty(&crate::i18n::t("cookies_file_path_netscape")) {
            return (Some(PathBuf::from(path_str)), None);
        }
        return (current_file.map(Path::to_path_buf), None);
    }
    // Default — clear overrides
    (None, None)
}

/// Interactive edit menu for a single profile. Every action persists
/// immediately via write_profile_file; canceling a prompt only skips that change.
pub(super) fn edit_profile_menu(name: &str, path: &Path) {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{}",
                crate::i18n::tf("error_read_profile", &[("e", &e.to_string())])
            );
            return;
        }
    };
    let mut profile: crate::config::Profile = match toml::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "{}",
                crate::i18n::tf("error_profile_toml_parse", &[("e", &e.to_string())])
            );
            eprintln!("{}", crate::i18n::t("help_advanced_editor"));
            // Only escape hatch available on corrupt data
            let _ = crate::cli::interactive::menu::confirm_back(
                crate::i18n::t("manage_profiles").as_str(),
                &crate::i18n::t("open_in_editor_now"),
                true,
                false,
            );
            if let Some(editor) = std::env::var("EDITOR")
                .ok()
                .or_else(|| std::env::var("VISUAL").ok())
                .or_else(|| Some("vi".to_string()))
            {
                let _ = std::process::Command::new(&editor).arg(path).status();
            }
            return;
        }
    };

    fn save(path: &Path, profile: &crate::config::Profile) -> bool {
        match crate::config::write_profile_file(path, profile) {
            Ok(()) => true,
            Err(e) => {
                eprintln!(
                    "{}",
                    crate::i18n::tf("error_save_profile", &[("e", &e.to_string())])
                );
                false
            }
        }
    }

    loop {
        clear_screen();
        // Rebuild account options each iteration (labels depend on contents)
        let mut flat: Vec<(String, usize)> = Vec::new(); // (site, idx within site vec)
        let mut account_options: Vec<String> = Vec::new();
        let mut sites_sorted: Vec<&String> = profile.accounts.keys().collect();
        sites_sorted.sort();
        for site in sites_sorted {
            let list = &profile.accounts[site];
            for (i, label) in account_labels(site, list).into_iter().enumerate() {
                flat.push((site.clone(), i));
                account_options.push(brand_account_label(&label));
            }
        }

        println!(
            "{}",
            crate::i18n::tf(
                "profile_header",
                &[
                    ("name", name),
                    ("count", &account_options.len().to_string())
                ]
            )
        );
        let delete_label = crate::i18n::t("delete_profile");
        let options = vec![
            crate::i18n::t("profile_add_account"),
            crate::i18n::t("profile_edit_account"),
            crate::i18n::t("profile_remove_account"),
            crate::i18n::t("profile_rename_display_name"),
            crate::i18n::t("profile_show_summary"),
            crate::i18n::t("advanced_open_editor").to_string(),
            delete_label.clone(),
            crate::i18n::t("back").to_string(),
        ];
        let choice = match select_menu(&crate::i18n::t("profile_menu"), options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            s if s == crate::i18n::t("back") => return,
            s if s == crate::i18n::t("delete_profile") => {
                // Destructive: Enter keeps the profile, `y` deletes it.
                if crate::cli::interactive::menu::confirm_back(
                    crate::i18n::t("manage_profiles").as_str(),
                    &crate::i18n::tf("delete_profile_confirm", &[("name", name)]),
                    false,
                    true,
                )
                .value()
                .unwrap_or(false)
                {
                    let _ = std::fs::remove_file(path);
                    println!("{}", crate::i18n::tf("profile_deleted", &[("name", name)]));
                    return;
                }
            }
            s if s == crate::i18n::t("profile_add_account") => {
                let site_opts = site_options_with_fallbacks(&["instagram", "tiktok", "facebook"]);
                let items: Vec<crate::cli::interactive::theme::SiteItem> = site_opts
                    .into_iter()
                    .map(crate::cli::interactive::theme::SiteItem::new)
                    .collect();
                let Ok(site) = select_menu(&crate::i18n::t("add_account_for_site"), items)
                    .prompt()
                    .map(|s| s.key())
                else {
                    continue;
                };
                let Some(username) = ask_nonempty(&crate::i18n::t("username")) else {
                    continue;
                };
                profile
                    .accounts
                    .entry(site)
                    .or_default()
                    .push(crate::config::Account {
                        username: Some(username),
                        ..Default::default()
                    });
                if save(path, &profile) {
                    println!("{}", crate::i18n::t("account_added"));
                }
            }
            s if s == crate::i18n::t("profile_edit_account") => {
                if account_options.is_empty() {
                    println!("{}", crate::i18n::t("no_accounts_to_edit"));
                    continue;
                }
                let picked = match select_menu(
                    &crate::i18n::t("edit_which_account"),
                    account_options.clone(),
                )
                .prompt()
                {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let Some(pos) = account_options.iter().position(|o| *o == picked) else {
                    continue;
                };
                let Some((site, idx)) = flat.get(pos) else {
                    continue;
                };
                let Some(acc) = profile
                    .accounts
                    .get_mut(site.as_str())
                    .and_then(|l| l.get_mut(*idx))
                else {
                    continue;
                };
                // Username with current as default
                let new_username = Text::new(&crate::i18n::t("username"))
                    .with_default(acc.username.as_deref().unwrap_or(""))
                    .prompt()
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                match new_username {
                    Some(u) => acc.username = Some(u),
                    None => continue,
                }
                // Cookies source
                let (cookies, browser) = prompt_cookies_choice(acc.cookies.as_deref());
                acc.cookies = cookies;
                acc.cookies_from_browser = browser;
                // Named cookie profile (outranks the above for this account)
                let profiles = crate::config::cookies::list_profiles();
                if !profiles.is_empty() {
                    let opts: Vec<String> =
                        std::iter::once(crate::i18n::t("none_paren").to_string())
                            .chain(profiles.iter().map(|p| {
                                format!(
                                    "{p}  — {}",
                                    crate::config::cookies::profile_summary(p).unwrap_or_default()
                                )
                            }))
                            .collect();
                    if let Ok(choice) =
                        select_menu(&crate::i18n::t("cookie_profile_for_account"), opts).prompt()
                    {
                        acc.cookie_profile = choice
                            .split("  — ")
                            .next()
                            .filter(|k: &&str| *k != crate::i18n::t("none_paren"))
                            .map(String::from);
                    }
                }
                if save(path, &profile) {
                    println!("{}", crate::i18n::t("account_updated"));
                }
            }
            s if s == crate::i18n::t("profile_remove_account") => {
                if account_options.is_empty() {
                    println!("{}", crate::i18n::t("no_accounts_to_remove"));
                    continue;
                }
                let picked = match select_menu(
                    &crate::i18n::t("remove_which_account"),
                    account_options.clone(),
                )
                .prompt()
                {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                // Destructive: Enter keeps the account, `y` removes it.
                if !crate::cli::interactive::menu::confirm_back(
                    crate::i18n::t("manage_profiles").as_str(),
                    &crate::i18n::tf("remove_account_confirm", &[("account", &picked)]),
                    false,
                    true,
                )
                .value()
                .unwrap_or(false)
                {
                    continue;
                }
                let Some(pos) = account_options.iter().position(|o| *o == picked) else {
                    continue;
                };
                let Some((site, idx)) = flat.get(pos) else {
                    continue;
                };
                if let Some(list) = profile.accounts.get_mut(site.as_str()) {
                    list.remove(*idx);
                    if list.is_empty() {
                        profile.accounts.remove(site.as_str());
                    }
                }
                if save(path, &profile) {
                    println!("{}", crate::i18n::t("account_removed"));
                }
            }
            s if s == crate::i18n::t("profile_rename_display_name") => {
                let current = profile.display_name.as_deref().unwrap_or("");
                match Text::new(&crate::i18n::t("display_name"))
                    .with_default(current)
                    .prompt()
                {
                    Ok(s) => {
                        let trimmed = s.trim().to_string();
                        profile.display_name = if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed)
                        };
                        if save(path, &profile) {
                            println!("{}", crate::i18n::t("display_name_updated"));
                        }
                    }
                    Err(_) => continue,
                }
            }
            s if s == crate::i18n::t("profile_show_summary") => {
                println!(
                    "{}",
                    crate::i18n::tf(
                        "profile_id_label",
                        &[("id", profile.profile.as_deref().unwrap_or("?"))]
                    )
                );
                if let Some(ref dn) = profile.display_name {
                    println!("{}", crate::i18n::tf("display_label", &[("name", dn)]));
                }
                let mut sites_now: Vec<&String> = profile.accounts.keys().collect();
                sites_now.sort();
                for site in sites_now {
                    if let Some(list) = profile.accounts.get(site.as_str()) {
                        for acc in list.iter() {
                            println!(
                                "{}",
                                crate::i18n::tf(
                                    "account_cookies_line",
                                    &[
                                        ("site", site),
                                        (
                                            "username",
                                            acc.username.as_deref().unwrap_or(&crate::i18n::t(
                                                "no_username_placeholder"
                                            ))
                                        ),
                                        (
                                            "cookies",
                                            &acc.cookies
                                                .as_ref()
                                                .map(|c| c.display().to_string())
                                                .or(acc.cookies_from_browser.clone())
                                                .unwrap_or_else(|| crate::i18n::t("default"))
                                        ),
                                    ]
                                )
                            );
                        }
                    }
                }
            }
            s if s == crate::i18n::t("advanced_open_editor") => {
                edit_with_editor(path);
                // Reload after possible external edits
                if let Ok(s) = std::fs::read_to_string(path) {
                    if let Ok(p) = toml::from_str::<crate::config::Profile>(&s) {
                        profile = p;
                    } else {
                        eprintln!("{}", crate::i18n::t("warn_toml_invalid_reload_skipped"));
                    }
                }
            }
            _ => return,
        }
    }
}
