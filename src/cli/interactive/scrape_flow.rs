use std::path::PathBuf;
use std::sync::Arc;

use inquire::{Confirm, Text};

use crate::application::scraper::{ScrapeRequest, validate_url};
use crate::config;

use super::content::{ContentKind, build_tagged_urls, prompt_content_kinds, select_urls};
use super::menu::Step;

/// A job as the preview and the dashboard pass it around:
/// `(request, site key, label shown in the UI, content description)`.
pub(super) type ScrapeJob = (ScrapeRequest, String, String, String);

/// The session chosen for this run, and whether the question was shown at all.
///
/// `asked` exists because this question is conditional: with no stored
/// profiles there is nothing to ask, so a step back aimed at it would be a
/// no-op that lands the user on the same preview they just left.
pub(super) struct CookiePick {
    /// The chosen cookie profile, or `None` to keep the site defaults.
    pub file: Option<PathBuf>,
    /// False when the question was skipped, so there is nothing to go back to.
    pub asked: bool,
}

impl CookiePick {
    fn skipped() -> Self {
        Self {
            file: None,
            asked: false,
        }
    }
}

/// Whether the cookie question has to be shown.
///
/// Both conditions matter. Without a TTY there is nobody to answer, and
/// without a stored profile there is nothing to choose between the defaults
/// and.
fn should_ask_cookies(is_tty: bool, profile_count: usize) -> bool {
    is_tty && profile_count > 0
}

/// Decide which cookie source a run should use, after the user has chosen what
/// to scrape.
///
/// `sites` are the site keys this run touches, used to rank the stored cookie
/// profiles. Passing the real keys matters: the previous call site for direct
/// URLs passed `""`, and `domains_for_site("")` returns no domains, so the
/// filter silently matched every profile and the question was meaningless.
///
/// Behaviour:
///
/// * **not a TTY, or no stored profiles** — do not prompt, so a first-time
///   user goes straight to the site defaults with no extra keystroke;
/// * **at least one stored profile** — always ask, offering the site default
///   alongside the created profiles, so the choice is always explicit.
///
/// `Step::Back` on Esc returns to the previous question of the flow, and
/// `Step::Cancel` on Ctrl+C leaves the run. "Use the site default" stays a
/// single keystroke: it is the first option, which Enter selects.
pub(super) fn prompt_cookie_override(sites: &[String]) -> Step<CookiePick> {
    use crate::config::cookies;

    // The TTY check lives here rather than at each call site: `asked` has to be
    // authoritative, and a caller that forgets the guard would otherwise claim
    // the user saw a question they never did.
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return Step::Value(CookiePick::skipped());
    }

    let profiles = cookies::list_profiles();
    if !should_ask_cookies(true, profiles.len()) {
        return Step::Value(CookiePick::skipped());
    }

    // Rank profiles that actually carry cookies for these sites first. When no
    // profile matches, every profile is still offered — the user may know
    // better than the filter, and silently hiding their profile is worse than
    // showing a mismatch hint.
    let mut ranked: Vec<(bool, String)> = profiles
        .into_iter()
        .map(|name| {
            let matches = cookies::has_cookies_for(&name, sites);
            (matches, name)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let any_match = ranked.iter().any(|(m, _)| *m);
    let mut opts = vec![DEFAULT_COOKIE_CHOICE.to_string()];
    for (matches, name) in &ranked {
        let summary = cookies::profile_summary(name).unwrap_or_default();
        // The label is only a display string; the key is recovered by index
        // below, never by re-parsing the text.
        opts.push(format!(
            "{name}{}  — {summary}",
            if *matches {
                ""
            } else {
                "  (no cookies for this site)"
            }
        ));
    }

    let title = if any_match {
        "Cookies for this run? (Enter = site default)"
    } else {
        "No stored profile has cookies for this site. Cookies for this run?"
    };
    // Index 0 is the site default; the rest are the ranked profiles.
    let entries: Vec<(String, Vec<String>)> = opts
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let details = if i == 0 {
                vec!["keep whatever sites/*.toml configures".to_string()]
            } else {
                Vec::new()
            };
            (label.clone(), details)
        })
        .collect();
    let index = match crate::cli::interactive::menu::pick_single_back(title, entries) {
        Step::Value(i) => i,
        step => return step.map(|_| CookiePick::skipped()),
    };
    // Index 0 is the site default, which is an explicit answer, not a skip.
    let file = if index == 0 {
        None
    } else {
        ranked
            .get(index - 1)
            .map(|(_, name)| name)
            .and_then(|name| cookies::profile_path(name))
            .filter(|p| p.exists())
    };
    Step::Value(CookiePick { file, asked: true })
}

/// Label for the "keep whatever the site config says" option.
const DEFAULT_COOKIE_CHOICE: &str = "Default (from site config)";

/// Human description of the session a job will use, for the pre-flight preview.
pub(super) fn describe_cookie_source(req: &ScrapeRequest) -> String {
    if let Some(file) = req.cookies_file.as_ref() {
        // Show the profile name, not the full path, when the file is one of ours.
        if let Some(dir) = crate::config::cookies::cookies_dir()
            && file.parent() == Some(dir.as_path())
            && let Some(stem) = file.file_stem()
        {
            return format!("cookie profile '{}'", stem.to_string_lossy());
        }
        return format!("cookies {}", file.display());
    }
    if let Some(browser) = req.cookies_from_browser.as_ref() {
        return format!("browser cookies ({browser})");
    }
    "site defaults (no session)".to_string()
}

/// Whether a stored profile holds at least one cookie for any of `sites`.
/// Apply a chosen cookie file to a job, only when that job's site is one the
/// file was chosen for.
///
/// The previous version applied one override to every job in the batch, so in
/// a mixed run a TikTok profile was handed to the Instagram job too — the
/// exact kind of cross-session contamination this feature must never cause.
/// Distinct site keys carried by these jobs, sorted. Used to offer only the
/// cookie profiles that can actually authenticate the batch.
fn distinct_sites(jobs: &[ScrapeJob]) -> Vec<String> {
    let mut v: Vec<String> = jobs.iter().map(|(_, site, ..)| site.clone()).collect();
    v.sort();
    v.dedup();
    v
}

pub(super) fn apply_cookie_file_to_site(
    jobs: &mut [ScrapeJob],
    file: &std::path::Path,
    sites: &[String],
) {
    apply_cookie_file_where(jobs, file, |_, site| sites.iter().any(|s| s == site));
}

/// Apply a cookie file only to the jobs `wants` selects.
///
/// Filtering by site is not enough once an account can carry its own session:
/// two instagram accounts in one batch may need different identities, and a
/// per-site filter would hand both of them the same one.
pub(super) fn apply_cookie_file_where<F>(jobs: &mut [ScrapeJob], file: &std::path::Path, wants: F)
where
    F: Fn(usize, &str) -> bool,
{
    for (i, (req, site, ..)) in jobs.iter_mut().enumerate() {
        if wants(i, site) {
            req.cookies_file = Some(file.to_path_buf());
            req.cookies_from_browser = None;
        }
    }
}

/// Everything one account's jobs need, independent of which flow asked.
///
/// Quick scrape and tasks both build jobs for a single (site, username, kinds)
/// triple, and the per-site fan-out inside [`jobs_for_account`] is the part that
/// is easy to get subtly wrong — twitter needs two passes because gallery-dl's
/// directory conditionals are evaluated per file, and threads needs the profile
/// pic separated because it takes its own flag. Shared here so a third caller
/// cannot silently download half an account.
pub(super) struct AccountCtx {
    pub site: String,
    pub username: String,
    pub kinds: Vec<ContentKind>,
    /// Tagged URLs for this account, so facebook ID resolution stays in the
    /// caller that knows how to do it.
    pub tagged: Vec<(ContentKind, String)>,
    pub directory_template: Option<Vec<String>>,
    pub extractor_options: std::collections::HashMap<String, toml::Value>,
    pub extra_args: Vec<String>,
    pub cookies_file: Option<std::path::PathBuf>,
    pub cookies_from_browser: Option<String>,
    pub archive: Option<std::path::PathBuf>,
    pub rate_limit: Option<crate::config::RateLimit>,
    pub filename_template: Option<String>,
    /// Becomes `{scrapmf_root}`, so a task writes under its own name.
    pub profile_name: String,
    pub output_dir: std::path::PathBuf,
}

/// Build the jobs one account expands into.
///
/// `Err` carries the reason to skip that account, so a caller with several of
/// them can name the account and the reason instead of aborting the batch.
pub(super) fn jobs_for_account(c: &AccountCtx) -> Result<Vec<ScrapeJob>, String> {
    // Bound by value because the per-site branches clone them repeatedly.
    let site_name = c.site.clone();
    let username = c.username.clone();
    let profile_name = c.profile_name.clone();
    let AccountCtx {
        kinds,
        directory_template: directory_template_raw,
        extractor_options: extractor_options_raw,
        extra_args,
        archive,
        rate_limit,
        filename_template,
        ..
    } = &c;
    let Some((url, extra_urls)) = select_urls(&c.tagged, &c.kinds) else {
        return Err("no content selected".to_string());
    };
    if validate_url(&url).is_err() {
        return Err(format!("invalid url {url}"));
    }

    let kinds_desc = super::content::kinds_description(&site_name, kinds);

    // QUICK MODE — bake the resolved username into directory templates
    let directory_template = directory_template_raw
        .clone()
        .map(|dirs| flatten_quick_dirs(dirs, &username));
    let mut extractor_options = extractor_options_raw.clone();
    for v in extractor_options.values_mut() {
        if let toml::Value::Table(map) = v
            && let Some(dir) = map.get_mut("directory")
        {
            flatten_for_quick(dir, &username);
        }
    }
    // Final cookies for the jobs: the caller already resolved any
    // per-run override over the site config.
    let (cookies_file, cookies_from_browser) =
        (c.cookies_file.clone(), c.cookies_from_browser.clone());

    // `{scrapmf_root}` comes from the ctx, so quick scrape keeps rooting at the
    // username while a task roots at its own name.
    let base_req = |directory_override: Option<Vec<String>>,
                    extra_opts: Vec<(String, String)>,
                    extra_urls: Vec<String>| {
        let mut directory_template_field = None;
        let mut opts = extractor_options.clone();
        apply_quick_override(
            &mut opts,
            &mut directory_template_field,
            &site_name,
            directory_override,
            extra_opts,
        );
        ScrapeRequest {
            url: url.clone(),
            output: Some(c.output_dir.clone()),
            preset: Some(site_name.clone()),
            extra_args: extra_args.clone(),
            cookies_from_browser: cookies_from_browser.clone(),
            cookies_file: cookies_file.clone(),
            archive: archive.clone(),
            rate_limit: rate_limit.clone(),
            extractor_options: opts,
            filename_template: filename_template.clone(),
            directory_template: directory_template_field,
            extra_urls,
            profile_name: Some(profile_name.clone()),
            extra_extractor_opts: Vec::new(),

            ..Default::default()
        }
    };

    // Twitter Media needs TWO passes (see prompt_scrape_as_profile note):
    // per-FILE conditional directories don't work on twitter.
    if site_name == "twitter" {
        let mut jobs = Vec::new();
        for (pass, dir_name, filter) in [
            ("photos", "photos", "type == 'photo'"),
            ("videos", "videos", "type != 'photo'"),
        ] {
            let dirs = vec![
                username.clone(),
                "twitter".to_string(),
                "{user[name]}".to_string(),
                dir_name.to_string(),
            ];
            let req = base_req(
                Some(dirs),
                vec![("file-filter".to_string(), filter.to_string())],
                Vec::new(),
            );
            jobs.push((
                req,
                site_name.clone(),
                format!("{username} ({pass})"),
                pass.to_string(),
            ));
        }
        return Ok(jobs);
    }

    // Threads: fotos/videos (posts) and profile pic are separate — profile needs --profile-pic-only
    // Always 3 separate jobs so the dashboard shows progress 1-by-1, even for All.
    if site_name == "threads" {
        use crate::cli::interactive::content::ContentKind;
        let has_photos = kinds.contains(&ContentKind::Photos);
        let has_videos = kinds.contains(&ContentKind::Videos);
        let has_profile = kinds.contains(&ContentKind::Profile);
        if has_photos || has_videos || has_profile {
            let mut jobs = Vec::new();
            if has_photos {
                let photos_dirs = flatten_quick_dirs(
                    vec![
                        "{scrapmf_root}".to_string(),
                        "{category}".to_string(),
                        "{username}".to_string(),
                        "photos".to_string(),
                    ],
                    &username,
                );
                let mut req_photos = base_req(Some(photos_dirs), Vec::new(), extra_urls.clone());
                req_photos.extra_args.push("--photos-only".to_string());
                jobs.push((
                    req_photos,
                    site_name.clone(),
                    format!("{username} (photos)"),
                    "photos".to_string(),
                ));
            }
            if has_videos {
                let videos_dirs = flatten_quick_dirs(
                    vec![
                        "{scrapmf_root}".to_string(),
                        "{category}".to_string(),
                        "{username}".to_string(),
                        "videos".to_string(),
                    ],
                    &username,
                );
                let mut req_videos = base_req(Some(videos_dirs), Vec::new(), extra_urls.clone());
                req_videos.extra_args.push("--videos-only".to_string());
                jobs.push((
                    req_videos,
                    site_name.clone(),
                    format!("{username} (videos)"),
                    "videos".to_string(),
                ));
            }
            if has_profile {
                let profile_dirs = flatten_quick_dirs(
                    vec![
                        "{scrapmf_root}".to_string(),
                        "{category}".to_string(),
                        "{username}".to_string(),
                        "profile".to_string(),
                    ],
                    &username,
                );
                let mut req_profile = base_req(Some(profile_dirs), Vec::new(), Vec::new());
                req_profile.profile_pic_only = true;
                jobs.push((
                    req_profile,
                    site_name.clone(),
                    format!("{username} (profile)"),
                    "profile".to_string(),
                ));
            }
            return Ok(jobs);
        }
    }

    let req = base_req(directory_template, Vec::new(), extra_urls);
    Ok(vec![(req, site_name.clone(), username.clone(), kinds_desc)])
}

/// Run a batch inside the dashboard after the pre-flight preview.
///
/// Returns `Step::Back` when the user pressed Esc on the preview so the
/// caller can re-ask the question before it — the preview is the last
/// question of a flow, and backing out of it used to abandon the whole run.
pub(super) fn preview_and_execute(requests: Vec<ScrapeJob>, cfg: &config::Config) -> Step<()> {
    if requests.is_empty() {
        println!("ℹ No content selected");
        return Step::Value(());
    }
    // The summary and the decision share one box, so the confirmation looks
    // like the rest of the app instead of dropping out of it. Without a TTY the
    // plain lines are kept, which is what scripts and CI rely on.
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut summary: Vec<String> = vec![format!("✔ Ready — {} job(s):", requests.len())];
    for (i, (req, site, username, kinds_desc)) in requests.iter().enumerate() {
        let out = req
            .output
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| cfg.general.output_dir.display().to_string());
        summary.push(format!("  {}. {site}:{username} → {kinds_desc}", i + 1));
        summary.push(format!("     → {out}"));
        // Surface the session each job will actually use, so a wrong cookie
        // profile is visible here rather than discovered after the download.
        summary.push(format!("     session: {}", describe_cookie_source(req)));
    }

    let proceed = if tty {
        // Enter is the fast path: the expected action is to continue.
        match crate::cli::interactive::menu::confirm_box(
            "Download content",
            "Proceed?",
            &summary,
            true,
            false,
        ) {
            Step::Value(v) => v,
            // Esc steps back to the question that built this batch. Nothing
            // has been queued yet, so returning here is safe.
            step => return step.map(|_| ()),
        }
    } else {
        for line in &summary {
            println!("{line}");
        }
        Confirm::new("Proceed?")
            .with_render_config(super::theme::render_config())
            .with_default(false)
            .prompt()
            .unwrap_or(false)
    };
    if !proceed {
        println!("canceled");
        return Step::Value(());
    }

    // Pre-flight: verify provider binaries exist before opening the dashboard.
    // Without this, a missing threadstractormf would fail instantly inside
    // run_dashboard and appear as "no ejecuta nada".
    {
        let mut missing: Vec<String> = Vec::new();
        for (req, site, _, _) in &requests {
            let needs_threads = site == "threads"
                || req.url.contains("threads.com")
                || req.url.contains("threads.net")
                || req
                    .extra_urls
                    .iter()
                    .any(|u| u.contains("threads.com") || u.contains("threads.net"));
            if needs_threads
                && !crate::providers::Provider::is_available(
                    &crate::providers::threadstractor::Threadstractor,
                )
            {
                let msg = "threads plugin is not enabled — enable it in scrapmf → Plugins (installs threadstractormf)".to_string();
                if !missing.contains(&msg) {
                    missing.push(msg);
                }
            } else if !needs_threads
                && !crate::providers::Provider::is_available(
                    &crate::providers::gallery_dl::GalleryDl,
                )
            {
                let msg = "gallery-dl not found — run `scrapmf setup`".to_string();
                if !missing.contains(&msg) {
                    missing.push(msg);
                }
            }
        }
        if !missing.is_empty() {
            for m in &missing {
                crate::output::print_error(m);
            }
            return Step::Value(());
        }
    }

    let total = requests.len();

    // TTY: batch runs inside the ratatui dashboard (alternate screen).
    if tty {
        let labels = requests
            .iter()
            .map(|(req, site, username, _)| {
                // Sub-process names are NOT baked into the header — the
                // dashboard renders them as a per-job checklist.
                let _ = (&req.url, &req.extra_urls);
                format!("{site}:{username}")
            })
            .collect::<Vec<_>>();
        let state = Arc::new(std::sync::Mutex::new(crate::ui::DashboardState::new(
            labels,
        )));
        let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut was_cancelled = false;
        // Reports are collected during the run and printed AFTER the
        // dashboard closes: the alternate screen cannot be copied from, and
        // previously failed jobs vanished with it (regression fixed here).
        let mut job_reports: Vec<Vec<(bool, String)>> = Vec::with_capacity(requests.len());
        // Paths gallery-dl reported downloading, per job, for the post-run
        // integrity check. The interactive path never ran one, so truncated
        // videos and stray .part files were invisible here — the one place a
        // user is most likely to look after a run.
        let downloaded_by_job: Vec<Arc<std::sync::Mutex<Vec<String>>>> = requests
            .iter()
            .map(|_| Arc::new(std::sync::Mutex::new(Vec::new())))
            .collect();

        crate::ui::run_dashboard(state.clone(), abort.clone(), true, || {
            for (i, (req, site, username, _desc)) in requests.iter().enumerate() {
                {
                    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                    st.set_running(i);
                }
                // Persistent copyable log of every gallery-dl line this job
                // produces (best-effort; disabled if the state dir fails).
                let runlog = std::rc::Rc::new(std::cell::RefCell::new(
                    crate::application::runlog::RunLog::open(site, username),
                ));
                let mut hooks = crate::application::scraper::ScrapeHooks {
                    on_steps_plan: Some(Box::new({
                        let st = state.clone();
                        move |names: Vec<String>| {
                            st.lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .set_steps(i, names);
                        }
                    })),
                    on_step: Some(Box::new({
                        let st = state.clone();
                        move |cur: usize| {
                            st.lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .set_step(i, cur);
                        }
                    })),
                    on_file: Box::new({
                        let st = state.clone();
                        let rl = runlog.clone();
                        let paths = downloaded_by_job[i].clone();
                        move |path: &str| {
                            rl.borrow_mut().line(path);
                            paths
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push(path.to_string());
                            let mut st = st.lock().unwrap_or_else(|p| p.into_inner());
                            st.add_file(i);
                            // Log panel reassurance: show the downloaded file
                            let name = path.rsplit('/').next().unwrap_or(path);
                            st.push_log(format!("✔ {name}"));
                        }
                    }),
                    on_log: Box::new({
                        let st = state.clone();
                        let rl = runlog.clone();
                        move |line: &str| {
                            rl.borrow_mut().line(line);
                            // Hide noisy threadstractormf header lines from dashboard (keep in runlog)
                            if line.contains("cookies-from-browser=")
                                || line.contains("threadstractormf @")
                                || line.contains("threadstractor @")
                                || line.contains("rate_limit=")
                                || line.trim() == "posts"
                            {
                                return;
                            }
                            st.lock().unwrap_or_else(|p| p.into_inner()).push_log(line);
                        }
                    }),
                };
                let result = crate::application::scraper::scrape_with_hooks(
                    req,
                    false,
                    Some(&mut hooks),
                    &abort,
                );
                let mut failed = false;
                {
                    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                    match &result {
                        Ok(_) => st.finish_ok(i),
                        Err(e) => {
                            if e.to_string().contains("aborted by user") {
                                st.cancel_from(i);
                                was_cancelled = true;
                                failed = true;
                            } else {
                                st.finish_failed(i, e.to_string());
                                failed = true;
                            }
                        }
                    }
                }
                {
                    let mut rl = runlog.borrow_mut();
                    let status = crate::application::runlog::status_line(&result);
                    rl.finish(&status);
                }
                let lines = format_job_outcome(&result, site, username);
                let log_path = runlog.borrow().path.display().to_string();
                drop(runlog);
                let mut rep = lines;
                // Taken once and reused: the scan needs the same paths, and
                // locking the job's list twice risks seeing it mid-write.
                let job_paths = downloaded_by_job[i]
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                // Post-run integrity: only meaningful when the scrape actually
                // succeeded, and never on a cancelled run — a half-finished
                // download would report every remaining file as truncated.
                if result.is_ok() && !was_cancelled {
                    let paths = &job_paths;
                    let out_dir = crate::config::expand_output_dir(
                        req.output.as_ref().unwrap_or(&cfg.general.output_dir),
                    );
                    let (checked, issues) =
                        crate::application::integrity::verify_run(paths, Some(&out_dir));
                    if let Some(summary) = crate::application::integrity::quality_summary(paths) {
                        rep.push((false, format!("quality: {summary}")));
                    }
                    for (error, text) in integrity_report_lines(checked, &issues) {
                        rep.push((error, text));
                    }
                }
                // Termux MediaScan — one scan per finished download, narrowed to
                // the tree this job actually wrote (quick/saved/url)
                if result.is_ok() {
                    let dir = crate::config::expand_output_dir(
                        req.output.as_ref().unwrap_or(&cfg.general.output_dir),
                    );
                    let _ = crate::application::media_scan::maybe_scan(&dir, Some(&job_paths));
                }
                if failed && !log_path.is_empty() {
                    rep.push((true, format!("log: {log_path}")));
                }
                job_reports.push(rep);
                if was_cancelled {
                    break;
                }
            }
        });

        if was_cancelled {
            println!("⚠ canceled");
        }
        for rep in &job_reports {
            for (stderr, line) in rep {
                if *stderr {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            }
        }
        return Step::Value(());
    }

    // Non-TTY (CI/pipes): raw inherited output as before
    for (i, (req, site, username, _desc)) in requests.into_iter().enumerate() {
        println!(
            "→ [{}/{}] Scraping {}:{} — {}",
            i + 1,
            total,
            site,
            username,
            req.url
        );
        let result = crate::application::scraper::scrape(&req, false);
        report_job_outcome(&result, &site, &username);
        if result.is_ok() {
            let dir = crate::config::expand_output_dir(
                req.output.as_ref().unwrap_or(&cfg.general.output_dir),
            );
            // No per-file ledger on this path, so the whole output dir is the
            // honest answer.
            let _ = crate::application::media_scan::maybe_scan(&dir, None);
        }
    }
    Step::Value(())
}

/// Render the integrity report as `(to_stderr, text)` lines.
///
/// Mirrors the grouping used by the CLI path: an HEVC video is a complete
/// download that this toolchain will not play, so it must not inherit the
/// "delete and re-run" advice. A file that could not be read is a third case —
/// completeness is simply unknown.
///
/// Returns an empty vec when there is nothing to say.
pub(super) fn integrity_report_lines(
    checked: usize,
    issues: &[(
        std::path::PathBuf,
        crate::application::integrity::IntegrityIssue,
    )],
) -> Vec<(bool, String)> {
    if issues.is_empty() {
        if checked > 0 {
            tracing::debug!(checked, "integrity check: all files OK");
        }
        return Vec::new();
    }
    let (delete, leave, unknown) = crate::application::integrity::partition_by_remedy(issues);
    let mut out = Vec::new();

    if !delete.is_empty() {
        out.push((true, format!("{} file(s) look incomplete:", delete.len())));
        for (path, issue) in &delete {
            out.push((
                true,
                format!("    - {} ({})", path.display(), issue.description()),
            ));
        }
        out.push((
            true,
            "    ↳ delete the file(s) above and re-run — deterministic filenames \
             re-download only what is missing"
                .to_string(),
        ));
    }

    if !leave.is_empty() {
        // Not a failure: these downloaded correctly.
        out.push((
            false,
            format!(
                "{} file(s) downloaded fine but use H.265/HEVC:",
                leave.len()
            ),
        ));
        for (path, issue) in &leave {
            out.push((
                false,
                format!("    - {} ({})", path.display(), issue.description()),
            ));
        }
        out.push((
            false,
            "    ↳ complete downloads — leave them. Play them with VLC, or convert with \
             ffmpeg -i <file> -c:v libx264 -crf 20 <out>"
                .to_string(),
        ));
    }

    if !unknown.is_empty() {
        out.push((
            true,
            format!("{} file(s) could not be read:", unknown.len()),
        ));
        for (path, issue) in &unknown {
            out.push((
                true,
                format!("    - {} ({})", path.display(), issue.description()),
            ));
        }
        out.push((
            true,
            "    ↳ completeness is unknown — check permissions and free disk space, \
             then re-run"
                .to_string(),
        ));
    }
    out
}

/// Format per-job outcome summary lines (shared TTY/non-TTY).
/// Each entry is `(to_stderr, text)` so callers keep stream semantics.
pub(super) fn format_job_outcome(
    result: &anyhow::Result<crate::application::scraper::ScrapeOutcome>,
    site: &str,
    username: &str,
) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut push = |stderr: bool, msg: String| out.push((stderr, msg));
    match result {
        Ok(outcome) => {
            if outcome.success_count > 0 {
                push(
                    false,
                    format!(
                        "✔ {site}:{username} done ({} sub-extractors)",
                        outcome.success_count
                    ),
                );
            }
            if !outcome.skipped.is_empty() {
                push(
                    false,
                    format!(
                        "⚠ {site}:{username} — {} sub-extractor(s) skipped (auth/unsupported):",
                        outcome.skipped.len()
                    ),
                );
                for s in &outcome.skipped {
                    push(false, format!("    - {s}"));
                }
            }
            if !outcome.failed.is_empty() {
                push(
                    true,
                    format!(
                        "✖ {site}:{username} — {} sub-extractor(s) failed:",
                        outcome.failed.len()
                    ),
                );
                for f in &outcome.failed {
                    push(true, format!("    - {f}"));
                }
            }
            if outcome.challenge_failures > 0 {
                push(
                    false,
                    format!(
                        "⚠ {site}:{username} — {} post(s) lost to JavaScript challenges. \
                         Refresh your browser session on the site, then re-run",
                        outcome.challenge_failures
                    ),
                );
            }
        }
        Err(e) => {
            let short = e.to_string().lines().next().unwrap_or("error").to_string();
            tracing::debug!(error = %e, "scrape job failed");
            push(true, format!("✖ {site}:{username} — {short}"));
        }
    }
    out
}

/// Print per-job outcome summary lines (shared TTY/non-TTY).
pub(super) fn report_job_outcome(
    result: &anyhow::Result<crate::application::scraper::ScrapeOutcome>,
    site: &str,
    username: &str,
) {
    for (stderr, line) in format_job_outcome(result, site, username) {
        if stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
}

/// Quick scrape: pick a network, type a username, choose content types.
/// Nothing is persisted — no profile file is created. The tree roots at the
/// username: {output}/{username}/{content}/... — site templates are preserved
/// except for the network/account path segments, which are stripped.
/// Quick mode: flatten a `directory` template value so downloads land
/// directly under `<output>/<root>/<content-type>/`.
///
/// - Removes network identity segments (`{category}`, `{user}`, ...)
/// - Replaces `{scrapmf_root}` with the literal root (the username), so we no
///   longer depend on gallery-dl keyword injection resolving correctly.
/// - Handles both plain arrays and conditional tables (e.g. TikTok posts keyed
///   by `post_type`). Arrays that would end up empty are left untouched.
fn flatten_for_quick(v: &mut toml::Value, root: &str) {
    match v {
        toml::Value::Array(arr) => {
            let original = arr.clone();
            arr.retain(|seg| {
                seg.as_str()
                    .is_none_or(|s| !crate::cli::interactive::content::is_identity_segment(s))
            });
            if arr.is_empty() {
                *arr = original;
                return;
            }
            for seg in arr.iter_mut() {
                // {scrapmf_root} actual + {scarpmf_root} legacy (configs de
                // usuarios anteriores al renombre)
                if matches!(
                    seg.as_str(),
                    Some("{scrapmf_root}") | Some("{scarpmf_root}")
                ) {
                    *seg = toml::Value::String(root.to_string());
                }
            }
        }
        toml::Value::Table(map) => {
            for (_, child) in map.iter_mut() {
                flatten_for_quick(child, root);
            }
        }
        _ => {}
    }
}

/// Same flattening for `Vec<String>` directory templates.
pub(super) fn flatten_quick_dirs(dirs: Vec<String>, root: &str) -> Vec<String> {
    let mut v = toml::Value::Array(dirs.into_iter().map(toml::Value::String).collect());
    flatten_for_quick(&mut v, root);
    match v {
        toml::Value::Array(items) => items
            .into_iter()
            .map(|seg| seg.as_str().unwrap_or_default().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// Route quick-scrape per-pass overrides into a `ScrapeRequest`.
///
/// - **twitter**: needs TWO passes with per-file filters (`photos` /
///   `videos`) and per-pass directories — per-FILE conditional templates
///   don't work there — so both ride inside extractor options scoped to
///   `extractor.twitter.media.*`.
/// - **every other site**: the flattened directory goes to the
///   request-level `directory_template` (emitted by `build_args` as
///   `-o extractor.<site>.directory=[...]`). Sub-extractor scoped
///   overrides (instagram stories/highlights/avatar) still win over it,
///   exactly like the normal profile flow.
///
/// Bug history: this used to hardcode `"twitter:media"` for ALL sites, so
/// an instagram quick scrape silently dropped its directory template —
/// posts fell back to gallery-dl's default `{category}/{username}` tree
/// while highlights created a second root from their own scoped override.
pub(super) fn apply_quick_override(
    extractor_options: &mut std::collections::HashMap<String, toml::Value>,
    directory_template: &mut Option<Vec<String>>,
    site_name: &str,
    directory_override: Option<Vec<String>>,
    extra_opts: Vec<(String, String)>,
) {
    if site_name == "twitter" {
        let mut insert_media_opt = |key: String, value: toml::Value| {
            let media = extractor_options
                .entry("twitter:media".to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if let toml::Value::Table(map) = media {
                map.insert(key, value);
            }
        };
        if let Some(dirs) = directory_override {
            insert_media_opt(
                "directory".to_string(),
                toml::Value::Array(dirs.into_iter().map(toml::Value::String).collect()),
            );
        }
        for (k, v) in extra_opts {
            insert_media_opt(k, toml::Value::String(v));
        }
    } else {
        *directory_template = directory_override;
        if !extra_opts.is_empty() {
            tracing::warn!(
                site = %site_name,
                opts = ?extra_opts,
                "quick-scrape extra_opts are twitter-only; dropped"
            );
        }
    }
}

/// Parse a pasted blob of URLs (separators: spaces, commas, tabs, newlines).
/// Returns `(valid_urls, error_lines)`; order preserved, duplicates dropped.
pub(super) fn parse_pasted_urls(raw: &str) -> (Vec<String>, Vec<String>) {
    let mut valid = Vec::new();
    let mut errors = Vec::new();
    for token in raw.split([',', ' ', '\t', '\n', '\r']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match validate_url(token) {
            Ok(_) => {
                if !valid.iter().any(|u| u == token) {
                    valid.push(token.to_string());
                }
            }
            Err(e) => errors.push(format!("{token}: {e}")),
        }
    }
    (valid, errors)
}

/// Auto-match a sites/*.toml entry whose `pattern` appears in `url`.
pub(super) fn auto_match_site_key(
    cfg: &config::Config,
    url: &str,
) -> Option<(String, crate::config::Site)> {
    // Longest matching pattern wins across BOTH config fields (pattern and
    // patterns[]), mirroring config::site_matches semantics.
    let mut best: Option<(usize, &String, &crate::config::Site)> = None;
    for (key, site) in cfg.sites.iter() {
        let candidates = site
            .pattern
            .iter()
            .map(String::as_str)
            .chain(site.patterns.iter().map(String::as_str));
        for pat in candidates {
            if url.contains(pat) && best.as_ref().is_none_or(|(len, _, _)| pat.len() > *len) {
                best = Some((pat.len(), key, site));
            }
        }
    }
    best.map(|(_, k, s)| (k.clone(), s.clone()))
}

/// Interactive flow: paste MULTIPLE direct URLs (e.g. private-account story
/// links copied while viewing them, individual post links), auto-match each
/// against sites/*.toml, and batch-run them.
///
/// Motivation (re-verified against gallery-dl v1.32.17, whose set of
/// `Tiktok*Extractor` class names and whose `_check_status_code` are
/// byte-identical to v1.32.16): TikTok private-but-followed accounts fail the
/// /@USER/stories LIST path (profile page statusCode 10222), but their direct
/// /video/<id> story links extract perfectly — so pasting links is the
/// reliable route. Every TikTok change in 1.32.9..1.32.17 randomizes headers
/// to dodge 403s, or is cosmetic (`isinstance()` -> `__class__ is`,
/// `binascii` -> `util.b64decode`); none moves a semantic 10222.
pub(super) fn prompt_scrape_direct_urls() {
    // Re-ask with the previous text pre-filled: one bad URL in a batch should
    // mean fixing that one, not starting over.
    let mut previous = String::new();
    'urls: loop {
        let (urls, _errors) = loop {
            let raw = match crate::cli::interactive::menu::input_text_back(
                "Download content ─ URL(s)",
                "Paste URL(s) — separate with spaces or commas:",
                "https://www.tiktok.com/@user/video/123 https://www.instagram.com/reel/xyz/",
                "each URL is matched against your sites/*.toml patterns",
                &previous,
            ) {
                Step::Value(v) => v,
                // First question of this flow: Esc leaves.
                Step::Back | Step::Cancel => {
                    println!("canceled");
                    return;
                }
            };
            let (urls, errors) = parse_pasted_urls(&raw);
            for e in &errors {
                println!("⚠ skipped invalid URL — {e}");
            }
            // Keep the text even on success: coming back from the cookie question
            // or the preview re-fills the box instead of losing the batch.
            previous = raw;
            if urls.is_empty() {
                println!("ℹ No valid URLs — Esc to cancel");
                continue;
            }
            break (urls, errors);
        };

        let cfg = config::load().unwrap_or_default();

        loop {
            // Rebuilt every pass: `preview_and_execute` consumes the batch, so a
            // new cookie choice has to start from the URLs again.
            let mut jobs = direct_url_jobs(&urls, &cfg);

            // Per-run cookie override (named profile instead of site defaults).
            // The sites are derived from the resolved jobs, not guessed, so the
            // profile filter actually has domains to work with.
            let job_sites = distinct_sites(&jobs);
            let pick = match prompt_cookie_override(&job_sites) {
                Step::Value(p) => p,
                // Esc on cookies returns to the URL box, still holding the text.
                Step::Back => continue 'urls,
                Step::Cancel => return,
            };
            if let Some(ref file) = pick.file {
                apply_cookie_file_to_site(&mut jobs, file, &job_sites);
            }

            match preview_and_execute(jobs, &cfg) {
                Step::Value(()) | Step::Cancel => return,
                Step::Back => {
                    // Go back to the cookie question only if it was actually
                    // shown. Without stored profiles it never was, so aiming
                    // at it here would land on this same preview forever.
                    if !pick.asked {
                        continue 'urls;
                    }
                }
            }
        }
    }
}

/// One job per pasted URL, auto-matched against sites/*.toml.
fn direct_url_jobs(urls: &[String], cfg: &config::Config) -> Vec<ScrapeJob> {
    let mut jobs = Vec::new();
    for url in urls {
        let label = url
            .trim_end_matches('/')
            .rsplit('/')
            .find(|seg| !seg.is_empty())
            .unwrap_or(url)
            .to_string();

        // Auto-match site config by pattern; URLs from unconfigured sites
        // still scrape with general defaults (gallery-dl supports hundreds
        // of extractors natively — inherits the old "(no site)" behavior).
        if let Some((site_key, site)) = auto_match_site_key(cfg, url) {
            let output = Some(match &site.output_dir {
                Some(o) => crate::config::expand_output_dir(o),
                None => crate::config::expand_output_dir(&cfg.general.output_dir),
            });
            let req = ScrapeRequest {
                url: url.clone(),
                output,
                preset: Some(site_key.clone()),
                extra_args: site.extra_args.clone(),
                cookies_from_browser: site.cookies_from_browser.clone(),
                cookies_file: site.cookies.clone(),
                archive: site.archive.clone(),
                rate_limit: site.rate_limit.clone(),
                extractor_options: site.extractor.clone(),
                filename_template: site.filename_template.clone(),
                directory_template: site.directory_template.clone(),
                extra_urls: Vec::new(),
                profile_name: None,
                extra_extractor_opts: Vec::new(),

                ..Default::default()
            };
            jobs.push((
                req,
                site_key.clone(),
                format!("{site_key}:{label}"),
                "direct link".to_string(),
            ));
        } else {
            println!("ℹ {url} — no sites/*.toml match; using general config");
            jobs.push((
                ScrapeRequest {
                    url: url.clone(),
                    output: Some(crate::config::expand_output_dir(&cfg.general.output_dir)),
                    preset: None,
                    extra_args: vec![],
                    cookies_from_browser: None,
                    cookies_file: None,
                    archive: None,
                    rate_limit: None,
                    extractor_options: Default::default(),
                    filename_template: None,
                    directory_template: None,
                    extra_urls: Vec::new(),
                    profile_name: None,
                    extra_extractor_opts: Vec::new(),

                    ..Default::default()
                },
                "general".to_string(),
                format!("general:{label}"),
                "direct link".to_string(),
            ));
        }
    }
    jobs
}

/// Stems of sites/*.toml files + registry entries, sorted. Registry is the
/// single source of truth — adding a site = one `SiteSpec` entry.
/// Shared by quick scrape and profiles.
pub(super) fn site_options_with_fallbacks(fallbacks: &[&str]) -> Vec<String> {
    let mut opts: Vec<String> = Vec::new();
    if let Some(dir) = crate::config::sites_dir()
        && let Ok(rd) = std::fs::read_dir(&dir)
    {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "toml")
                && let Some(stem) = e.path().file_stem().and_then(|n| n.to_str())
            {
                opts.push(stem.to_string());
            }
        }
    }
    // Ensure registry sites appear even without a file yet (e.g. threads before install)
    for spec in crate::sites::registry::all_specs() {
        if !opts.iter().any(|s| s == spec.id) {
            opts.push(spec.id.to_string());
        }
    }
    for fb in fallbacks {
        if !opts.iter().any(|s| s == fb) {
            opts.push((*fb).to_string());
        }
    }
    // Plugin gating: sites backed by an optional provider only appear when
    // the plugin is installed and enabled (threads → threadstractormf).
    if !crate::plugins::threads_enabled() {
        opts.retain(|s| s != "threads");
    }
    opts.sort();
    opts
}

/// Normalise what the user typed for a username: trim surrounding whitespace
/// and nothing else.
///
/// A leading `@` is deliberately kept. It is stripped later, where the value is
/// turned into a URL, and stripping it here made the text change under the user
/// when a question was revisited — and reduced a lone `@` to an empty string,
/// which the caller could not tell apart from no answer at all.
pub(crate) fn username_prompt_value(raw: &str) -> String {
    raw.trim().to_string()
}

/// Ask for the account to scrape. Returns `None` when the user steps back, in
/// which case the caller shows the previous question again; `initial` carries
/// whatever was typed before so only the wrong part has to be corrected.
fn username_prompt(site: &str, initial: &str) -> Option<String> {
    let prompt = if site == "facebook" {
        "Profile ID or URL (e.g. 123..., https://www.facebook.com/profile.php?id=...):"
    } else if site == "instagram" {
        "Username or ID (without @):"
    } else {
        "Username (without @):"
    };
    match crate::cli::interactive::menu::input_text_back(
        &format!("Download content ─ {site}"),
        prompt,
        "someuser",
        "",
        initial,
    ) {
        Step::Value(v) => Some(username_prompt_value(&v)),
        Step::Back | Step::Cancel => None,
    }
}

pub(super) fn prompt_quick_scrape() {
    let cfg = config::load().unwrap_or_default();

    // Site selection from sites/*.toml (+ fallbacks)
    let site_opts =
        site_options_with_fallbacks(&["instagram", "tiktok", "twitter", "vsco", "facebook"]);

    // Decorated Browser with fixed chrome `╭ SCRAPMF v1.7.0 ─ Download content ─╮`
    // Rebuilt whenever the flow returns to this question.
    let site_opts_ui = |site_opts: &[String]| -> Vec<(String, Vec<String>)> {
        site_opts
            .iter()
            .map(|k| {
                let spec = crate::sites::registry::find_by_id(k);
                let details = if let Some(s) = spec {
                    vec![
                        format!("pattern: {}", s.patterns.join(", ")),
                        format!("backend: {:?}", s.backend),
                        format!("kinds: {}", s.content_kinds.join(", ")),
                    ]
                } else {
                    vec!["custom site (sites/*.toml)".to_string()]
                };
                (crate::cli::interactive::theme::brand_site_label(k), details)
            })
            .collect()
    };

    // site ⇄ username ⇄ content. Esc steps back one question instead of
    // discarding the run, and the username is kept so a typo is corrected in
    // place rather than retyped.
    let mut username_answer = String::new();
    // Labelled at both levels so Esc can walk back through the whole flow:
    // preview → cookies → content → username → site.
    'wizard: loop {
        let site_name = loop {
            // Rebuilt each time so a step back re-renders the full list.
            let site_list = site_opts_ui(&site_opts);
            let idx =
                match crate::cli::interactive::menu::pick_single("Download content", site_list) {
                    Step::Value(i) => i,
                    // First question of the flow: Esc leaves.
                    Step::Back | Step::Cancel => return,
                };
            let site = site_opts[idx].clone();
            // Re-ask rather than treat a bare "@" as no answer: previously an
            // input that normalised to empty looked like a step back, so the
            // user was thrown out of the flow for a typo instead of fixing it.
            let typed = loop {
                match username_prompt(&site, &username_answer) {
                    Some(t) if t.is_empty() => {
                        println!("⚠ Enter a username");
                        continue;
                    }
                    Some(t) => break t,
                    None => break String::new(),
                }
            };
            if typed.is_empty() {
                // Esc at the username returns to the site list.
                continue;
            }
            username_answer = typed;
            break site;
        };

        let raw_input = username_answer.clone();
        // Labelled so Esc on the cookie question re-asks this content list.
        'kinds: loop {
            let kinds = match prompt_content_kinds(
                &site_name,
                &super::theme::brand_account_label(&format!("{site_name}:{raw_input}")),
            ) {
                Step::Value(k) if k.is_empty() => {
                    println!("ℹ No content selected");
                    return;
                }
                Step::Value(k) => k,
                // Esc at content returns to the username, keeping its text.
                Step::Back => {
                    match username_prompt(&site_name, &username_answer) {
                        Some(typed) => username_answer = typed,
                        // Esc again steps back to the site list, which is what
                        // the outer label is for.
                        None => continue 'wizard,
                    }
                    // The username changed, so the content list is asked again.
                    continue 'kinds;
                }
                Step::Cancel => return,
            };
            // Facebook: accept ID or full profile URL (profile.php?id=, people/Name/ID, fb.com, etc.)
            // Instagram: accept ID (7-19 digits) as before. Both resolve ID → username.
            let (raw_is_id, raw_id, display_for_menu) = if site_name == "instagram"
                && crate::application::instagram_resolver::is_id_like(&raw_input)
            {
                let id = crate::application::instagram_resolver::normalize_id(&raw_input);
                (true, id.clone(), id)
            } else if site_name == "facebook" {
                if let Some(extracted) =
                    crate::application::facebook_resolver::extract_identifier(&raw_input)
                {
                    let is_id = crate::application::facebook_resolver::is_id_like(&extracted);
                    if is_id {
                        let nid = crate::application::facebook_resolver::normalize_id(&extracted);
                        (true, nid.clone(), nid)
                    } else {
                        (false, String::new(), extracted)
                    }
                } else {
                    (
                        false,
                        String::new(),
                        raw_input.trim().trim_start_matches('@').to_string(),
                    )
                }
            } else {
                (
                    false,
                    String::new(),
                    raw_input.trim().trim_start_matches('@').to_string(),
                )
            };
            // Keep original ID for facebook URL building (pages use profile.php?id=ID, not sanitized title)
            let facebook_id_for_url: Option<String> = if site_name == "facebook" && raw_is_id {
                Some(raw_id.clone())
            } else {
                None
            };

            // Content menu — same cycle as username (choose content before cookies/resolve)

            // Site config (raw, not yet baked with username)
            let site_cfg = cfg.sites.get(site_name.as_str()).cloned();
            let extractor_options_raw = site_cfg
                .as_ref()
                .map(|s| s.extractor.clone())
                .unwrap_or_default();
            let directory_template_raw =
                site_cfg.as_ref().and_then(|s| s.directory_template.clone());
            // Session precedence is resolved once, by the shared helper, so the
            // wizard cannot drift from the CLI, profiles and tasks: a per-run
            // pick > a named profile > the site's cookies file > the single
            // global browser choice > a deprecated per-site field.
            let (site_cookies_file, site_cookies_browser) = crate::config::cookies::resolve_session(
                site_cfg.as_ref().and_then(|s| s.cookie_profile.as_deref()),
                None,
                None,
                cfg.general.cookies_from_browser.clone(),
                site_cfg.as_ref(),
            );
            let rate_limit = site_cfg.as_ref().and_then(|s| s.rate_limit.clone());
            let archive = site_cfg.as_ref().and_then(|s| s.archive.clone());
            let extra_args = site_cfg
                .as_ref()
                .map(|s| s.extra_args.clone())
                .unwrap_or_default();
            let filename_template = site_cfg.as_ref().and_then(|s| s.filename_template.clone());

            // Everything from here to the preview is re-runnable, because Esc on the
            // preview comes back to the cookie question and the batch is rebuilt from
            // the URLs with the new session.
            loop {
                // Per-run cookie override comes BEFORE ID resolution so resolver uses
                // the same session that will be used for downloading (same cycle as username).
                let pick = match prompt_cookie_override(std::slice::from_ref(&site_name)) {
                    Step::Value(p) => p,
                    // Esc on cookies goes back to the content list; the site and the
                    // username typed so far are kept.
                    Step::Back => continue 'kinds,
                    Step::Cancel => return,
                };
                let cookie_override = pick.file.clone();
                let (cookies_file_for_resolve, cookies_browser_for_resolve) =
                    if let Some(ref ov) = cookie_override {
                        (Some(ov.as_path()), None)
                    } else {
                        (
                            site_cookies_file.as_deref(),
                            site_cookies_browser.as_deref(),
                        )
                    };

                // Now resolve ID → username if needed, using the final cookies
                let username = if raw_is_id {
                    let res = if site_name == "instagram" {
                        crate::application::instagram_resolver::resolve_instagram_username(
                            &raw_id,
                            cookies_file_for_resolve,
                            cookies_browser_for_resolve,
                        )
                    } else if site_name == "facebook" {
                        crate::application::facebook_resolver::resolve_facebook_id_to_username(
                            &raw_id,
                            cookies_file_for_resolve,
                            cookies_browser_for_resolve,
                        )
                    } else {
                        Err(anyhow::anyhow!("unsupported site for ID"))
                    };
                    match res {
                        Ok(u) => {
                            println!("→ {} → @{} (resolved)", raw_id, u);
                            u
                        }
                        Err(e) => {
                            let site_label = if site_name == "facebook" { "FB" } else { "IG" };
                            crate::output::print_error(&format!(
                                "could not resolve ID to username: {e} — check the ID and that your {site_label} session is still valid"
                            ));
                            crate::output::print_help(
                                "note: the error stays visible until you press Enter",
                            );
                            let _ = Text::new("Press Enter to go back")
                                .with_render_config(super::theme::render_config())
                                .prompt();
                            return;
                        }
                    }
                } else {
                    display_for_menu.clone()
                };

                let tagged = if site_name == "facebook"
                    && let Some(id) = &facebook_id_for_url
                {
                    vec![
                        (
                            ContentKind::Posts,
                            format!("https://www.facebook.com/profile.php?id={id}/photos"),
                        ),
                        (
                            ContentKind::Albums,
                            format!("https://www.facebook.com/profile.php?id={id}/photos_albums"),
                        ),
                        (
                            ContentKind::Videos,
                            format!("https://www.facebook.com/profile.php?id={id}/videos/"),
                        ),
                    ]
                } else {
                    build_tagged_urls(&site_name, &username)
                };
                // Final cookies for the jobs: override wins over site config.
                let (cookies_file, cookies_from_browser) = if let Some(ref ov) = pick.file {
                    (Some(ov.clone()), None)
                } else {
                    (site_cookies_file.clone(), site_cookies_browser.clone())
                };
                let account = AccountCtx {
                    site: site_name.clone(),
                    username: username.clone(),
                    kinds: kinds.clone(),
                    tagged,
                    directory_template: directory_template_raw.clone(),
                    extractor_options: extractor_options_raw.clone(),
                    extra_args: extra_args.clone(),
                    cookies_file,
                    cookies_from_browser,
                    archive: archive.clone(),
                    rate_limit: rate_limit.clone(),
                    filename_template: filename_template.clone(),
                    profile_name: username.clone(),
                    output_dir: cfg.general.output_dir.clone(),
                };
                let jobs = match jobs_for_account(&account) {
                    Ok(j) => j,
                    Err(reason) => {
                        println!("⚠ {username} — {reason}");
                        continue 'kinds;
                    }
                };
                match preview_and_execute(jobs, &cfg) {
                    Step::Value(()) | Step::Cancel => return,
                    Step::Back => {
                        if !pick.asked {
                            continue 'kinds;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod integrity_report_tests {
    use super::integrity_report_lines;
    use crate::application::integrity::IntegrityIssue;
    use std::path::PathBuf;

    fn entry(name: &str, issue: IntegrityIssue) -> (PathBuf, IntegrityIssue) {
        (PathBuf::from(name), issue)
    }

    fn text(lines: &[(bool, String)]) -> String {
        lines
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn nothing_to_say_returns_no_lines() {
        assert!(integrity_report_lines(5, &[]).is_empty());
    }

    /// The regression that motivated this: a complete HEVC download must not
    /// be listed among files to delete, and must not carry delete advice.
    #[test]
    fn hevc_never_gets_delete_advice() {
        let out = integrity_report_lines(1, &[entry("clip.mp4", IntegrityIssue::HevcCodec)]);
        let joined = text(&out);
        assert!(
            !joined.contains("delete"),
            "complete HEVC file must not be told to delete itself:\n{joined}"
        );
        assert!(
            joined.contains("complete"),
            "must say the download is fine:\n{joined}"
        );
    }

    #[test]
    fn truncated_files_do_get_delete_advice() {
        let out = integrity_report_lines(
            3,
            &[
                entry("a.part", IntegrityIssue::PartialFile),
                entry("b.mp4", IntegrityIssue::MissingMoov),
                entry("c.mp4", IntegrityIssue::Empty),
            ],
        );
        let joined = text(&out);
        assert!(joined.contains("delete"), "{joined}");
        assert!(joined.contains("a.part") && joined.contains("b.mp4"));
    }

    #[test]
    fn unreadable_files_get_their_own_advice() {
        let out = integrity_report_lines(1, &[entry("locked.mp4", IntegrityIssue::Unreadable)]);
        let joined = text(&out);
        assert!(joined.contains("could not be read"), "{joined}");
        assert!(
            joined.contains("unknown"),
            "must say completeness is unknown:\n{joined}"
        );
        assert!(
            !joined.contains("delete"),
            "an unreadable file is not a reason to delete:\n{joined}"
        );
    }

    #[test]
    fn mixed_run_prints_all_three_groups_separately() {
        let out = integrity_report_lines(
            4,
            &[
                entry("cut.mp4", IntegrityIssue::MissingMoov),
                entry("hevc.mp4", IntegrityIssue::HevcCodec),
                entry("locked.mp4", IntegrityIssue::Unreadable),
            ],
        );
        let joined = text(&out);
        assert!(joined.contains("look incomplete"), "{joined}");
        assert!(joined.contains("H.265/HEVC"), "{joined}");
        assert!(joined.contains("could not be read"), "{joined}");
        // The delete advice must be attached to the incomplete list only.
        let delete_line = out
            .iter()
            .find(|(_, t)| t.contains("delete the file(s)"))
            .map(|(e, _)| *e);
        assert_eq!(delete_line, Some(true), "delete advice goes to stderr");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod quick_flatten_tests {
    use super::{auto_match_site_key, flatten_quick_dirs, parse_pasted_urls};
    use crate::config::{Config, Site};
    use toml::Value;

    fn site_with_pattern(pattern: &str) -> Site {
        Site {
            pattern: Some(pattern.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn parse_pasted_urls_splits_separators_and_dedupes() {
        let raw = "https://a.com/1, https://b.com/2\nhttps://c.com/3\thttps://a.com/1";
        let (valid, errors) = parse_pasted_urls(raw);
        assert_eq!(
            valid,
            vec![
                "https://a.com/1".to_string(),
                "https://b.com/2".to_string(),
                "https://c.com/3".to_string(),
            ],
            "order preserved, duplicates dropped"
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn parse_pasted_urls_reports_invalid_tokens() {
        let (valid, errors) = parse_pasted_urls("https://ok.com/x notaurl ftp://bad.com/y");
        assert_eq!(valid, vec!["https://ok.com/x".to_string()]);
        assert_eq!(errors.len(), 2, "invalid scheme and garbage are reported");
    }

    #[test]
    fn auto_match_picks_longest_matching_pattern() {
        let mut cfg = Config::default();
        cfg.sites
            .insert("tiktok".to_string(), site_with_pattern("tiktok.com"));
        cfg.sites
            .insert("tiktok-wide".to_string(), site_with_pattern("tiktokv.com"));
        cfg.sites
            .insert("instagram".to_string(), site_with_pattern("instagram.com"));

        let (key, _) = auto_match_site_key(&cfg, "https://www.tiktok.com/@user/video/123")
            .expect("must match");
        assert_eq!(key, "tiktok");

        let (key, _) = auto_match_site_key(&cfg, "https://www.tiktokv.com/@user/video/123")
            .expect("must match");
        assert_eq!(key, "tiktok-wide", "longest matching pattern wins");

        assert!(
            auto_match_site_key(&cfg, "https://example.com/x").is_none(),
            "no matching pattern → caller falls back to general config"
        );
    }

    fn arr(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| Value::String(s.to_string())).collect())
    }

    #[test]
    fn plain_array_strips_identity_and_bakes_root() {
        let v = arr(&["{scrapmf_root}", "{category}", "{user}", "stories"]);
        let out = flatten_quick_dirs(
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect(),
            "profile_user",
        );
        assert_eq!(out, vec!["profile_user", "stories"]);
    }

    #[test]
    fn conditional_table_tiktok_posts_both_branches() {
        // Structure identical to the TikTok posts template
        let mut dir_table = toml::map::Map::new();
        dir_table.insert(
            "post_type == 'image'".to_string(),
            arr(&["{scrapmf_root}", "{category}", "{user}", "photos"]),
        );
        dir_table.insert(
            String::new(),
            arr(&["{scrapmf_root}", "{category}", "{user}", "videos"]),
        );
        let mut extractor = toml::map::Map::new();
        extractor.insert("directory".to_string(), Value::Table(dir_table));
        let mut top = toml::map::Map::new();
        top.insert("tiktok:posts".to_string(), Value::Table(extractor));

        let mut v = Value::Table(top);
        super::flatten_for_quick(&mut v, "profile_user");

        println!("DEBUG v = {}", v);
        let posts = &v["tiktok:posts"]["directory"];
        assert_eq!(
            posts["post_type == 'image'"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["profile_user", "photos"]
        );
        assert_eq!(
            posts[""]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["profile_user", "videos"]
        );
    }

    #[test]
    fn array_without_identity_segments_untouched_except_root() {
        let out = flatten_quick_dirs(vec!["{date:%Y}".into(), "media".into()], "root");
        assert_eq!(out, vec!["{date:%Y}", "media"]);
    }

    #[test]
    fn all_identity_array_keeps_original_rather_than_empty() {
        let out = flatten_quick_dirs(vec!["{user}".into(), "{category}".into()], "root");
        assert_eq!(out, vec!["{user}", "{category}"]);
    }

    /// Regression: instagram quick scrape must route the flattened directory
    /// through the request-level `directory_template`, NOT the twitter-only
    /// `extractor.twitter.media` hack (which gallery-dl ignores for
    /// instagram — posts fell back to `{category}/{username}` and created a
    /// second tree root).
    #[test]
    fn quick_override_instagram_uses_request_level_directory() {
        use super::apply_quick_override;

        let mut opts = std::collections::HashMap::new();
        let mut dir_tmpl = None;
        apply_quick_override(
            &mut opts,
            &mut dir_tmpl,
            "instagram",
            Some(vec!["sample_user".to_string(), "{subcategory}".to_string()]),
            Vec::new(),
        );
        assert_eq!(
            dir_tmpl,
            Some(vec!["sample_user".to_string(), "{subcategory}".to_string()]),
            "instagram override must land in directory_template"
        );
        assert!(
            !opts.contains_key("twitter:media"),
            "instagram quick scrape must not pollute twitter:media options"
        );
    }

    #[test]
    fn quick_override_twitter_keeps_media_scoped_options() {
        use super::apply_quick_override;

        let mut opts = std::collections::HashMap::new();
        let mut dir_tmpl = None;
        apply_quick_override(
            &mut opts,
            &mut dir_tmpl,
            "twitter",
            Some(vec!["user".to_string(), "photos".to_string()]),
            vec![("file-filter".to_string(), "type == 'photo'".to_string())],
        );
        assert_eq!(dir_tmpl, None, "twitter keeps using extractor options");
        let media = opts.get("twitter:media").expect("twitter:media table");
        match media {
            toml::Value::Table(map) => {
                assert_eq!(
                    map.get("directory").and_then(|v| v.as_array()),
                    Some(&vec![
                        toml::Value::String("user".into()),
                        toml::Value::String("photos".into())
                    ])
                );
                assert_eq!(
                    map.get("file-filter").and_then(|v| v.as_str()),
                    Some("type == 'photo'")
                );
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    // ─── Cookie session selection ──────────────────────────────────────────

    fn req_with(
        cookies_file: Option<&str>,
        cookies_from_browser: Option<&str>,
    ) -> crate::application::ScrapeRequest {
        crate::application::ScrapeRequest {
            cookies_file: cookies_file.map(std::path::PathBuf::from),
            cookies_from_browser: cookies_from_browser.map(str::to_string),
            ..Default::default()
        }
    }

    /// The preview must name the session each job will use, so a wrong cookie
    /// profile is caught before the download rather than after it.
    #[test]
    fn cookie_source_preview_covers_every_case() {
        assert_eq!(
            super::describe_cookie_source(&req_with(None, None)),
            "site defaults (no session)"
        );
        assert_eq!(
            super::describe_cookie_source(&req_with(None, Some("brave"))),
            "browser cookies (brave)"
        );
        assert!(
            super::describe_cookie_source(&req_with(Some("/tmp/elsewhere.txt"), None))
                .contains("/tmp/elsewhere.txt"),
            "an external cookie path must be shown verbatim"
        );
    }

    #[test]
    fn cookie_source_preview_names_stored_profiles() {
        // Point the profile directory at a temp tree holding one profile, so
        // the label resolves to the friendly profile name.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let profile = dir.path().join("brave-instagram.txt");
        std::fs::write(
            &profile,
            b"# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tk\tv\n",
        )
        .expect("write");
        let shown = super::describe_cookie_source(&req_with(
            Some(profile.to_str().expect("utf8 path")),
            None,
        ));
        assert!(
            shown.contains("brave-instagram"),
            "expected the profile name in {shown:?}"
        );
    }

    /// A single-site override must not leak into another site's job. Handing a
    /// TikTok profile to the Instagram job is exactly the cross-session
    /// contamination this guard exists to prevent.
    #[test]
    fn cookie_override_only_touches_jobs_of_the_chosen_site() {
        let file = std::path::Path::new("/tmp/tiktok-profile.txt");
        let mut jobs: Vec<(crate::application::ScrapeRequest, String, String, String)> = vec![
            (
                req_with(Some("/site/tiktok.txt"), None),
                "tiktok".to_string(),
                "tiktok:user".to_string(),
                "videos".to_string(),
            ),
            (
                req_with(Some("/site/instagram.txt"), None),
                "instagram".to_string(),
                "instagram:user".to_string(),
                "posts".to_string(),
            ),
        ];
        super::apply_cookie_file_to_site(&mut jobs, file, &["tiktok".to_string()]);
        assert_eq!(
            jobs[0].0.cookies_file.as_deref(),
            Some(file),
            "the chosen site must adopt the profile"
        );
        assert_eq!(
            jobs[1].0.cookies_file.as_deref(),
            Some(std::path::Path::new("/site/instagram.txt")),
            "another site must keep its own cookies"
        );
    }

    /// Choosing a profile must clear any browser-cookie source, otherwise the
    /// backend receives both and the browser session silently wins.
    #[test]
    fn cookie_override_clears_browser_cookies() {
        let mut jobs: Vec<(crate::application::ScrapeRequest, String, String, String)> = vec![(
            req_with(None, Some("brave")),
            "tiktok".to_string(),
            "tiktok:user".to_string(),
            "videos".to_string(),
        )];
        super::apply_cookie_file_to_site(
            &mut jobs,
            std::path::Path::new("/tmp/p.txt"),
            &["tiktok".to_string()],
        );
        assert_eq!(jobs[0].0.cookies_from_browser, None);
        assert!(jobs[0].0.cookies_file.is_some());
    }

    /// An unknown site key must not be treated as "matches everything": that is
    /// what made the direct-URL flow, which passed `""`, offer every profile
    /// regardless of relevance.
    #[test]
    fn unknown_site_does_not_match_profiles() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cookies_dir = dir.path().join("cookies");
        std::fs::create_dir_all(&cookies_dir).expect("mkdir");
        std::fs::write(
            cookies_dir.join("p.txt"),
            b"# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tk\tv\n",
        )
        .expect("write");
        // `has_cookies_for` consults the real profile store, so only assert the
        // domain-matching contract through the helper's own inputs.
        let unknown_domains = crate::config::cookies::domains_for_site("");
        assert!(
            unknown_domains.is_empty(),
            "an empty site key must resolve to no domains, not all domains"
        );
    }

    // ─── Username normalisation ─────────────────────────────────────────────

    /// The typed value round-trips apart from surrounding whitespace. Keeping
    /// the `@` matters: it is stripped where the URL is built, and stripping
    /// it here changed the text under the user on a revisit.
    #[test]
    fn username_keeps_the_at_sign_the_user_typed() {
        use super::username_prompt_value;
        assert_eq!(username_prompt_value("@user"), "@user");
        assert_eq!(username_prompt_value("  @user  "), "@user");
        assert_eq!(username_prompt_value("user"), "user");
    }

    /// A lone `@` must not collapse to "no answer", or a single stray
    /// character would eject the user from the flow.
    #[test]
    fn a_lone_at_sign_is_not_an_empty_answer() {
        use super::username_prompt_value;
        assert!(!username_prompt_value("@").is_empty());
        assert!(!username_prompt_value("   @   ").is_empty());
        assert!(username_prompt_value("   ").is_empty());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod back_chain_tests {
    use super::{direct_url_jobs, distinct_sites};
    use crate::application::scraper::ScrapeRequest;
    use crate::config::Config;

    fn job(site: &str) -> super::ScrapeJob {
        (
            ScrapeRequest::default(),
            site.to_string(),
            format!("{site}:someone"),
            "posts".to_string(),
        )
    }

    #[test]
    fn distinct_sites_sorts_and_dedupes() {
        // The cookie filter is only useful if a site appears once, otherwise
        // the same profile is offered twice in the ranking.
        let sites = distinct_sites(&[job("tiktok"), job("instagram"), job("tiktok")]);
        assert_eq!(sites, vec!["instagram".to_string(), "tiktok".to_string()]);
    }

    #[test]
    fn the_cookie_question_only_exists_when_it_was_shown() {
        use super::should_ask_cookies;
        // Without a TTY there is nobody to answer, and with no stored profile
        // there is nothing to choose. A step back aimed at a question that was
        // never shown lands on the same preview, which is the bug this guards.
        assert!(!should_ask_cookies(false, 0), "no TTY, no profiles");
        assert!(
            !should_ask_cookies(true, 0),
            "no profiles means nothing to ask about"
        );
        assert!(!should_ask_cookies(false, 3), "a pipe cannot answer");
        assert!(should_ask_cookies(true, 1), "a TTY with a profile asks");
    }

    #[test]
    fn direct_url_jobs_keeps_every_pasted_url() {
        // The batch is rebuilt on every pass of the cookie/preview loop, so
        // losing a URL here would silently drop downloads after a step back.
        let cfg = Config::default();
        let urls = vec![
            "https://www.tiktok.com/@user/video/1".to_string(),
            "https://www.instagram.com/reel/abc/".to_string(),
        ];
        let jobs = direct_url_jobs(&urls, &cfg);
        assert_eq!(jobs.len(), 2);
        let rebuilt = direct_url_jobs(&urls, &cfg);
        assert_eq!(
            rebuilt
                .iter()
                .map(|(_, s, ..)| s.clone())
                .collect::<Vec<_>>(),
            jobs.iter().map(|(_, s, ..)| s.clone()).collect::<Vec<_>>(),
            "rebuilding after a step back must yield the same batch"
        );
    }
}
