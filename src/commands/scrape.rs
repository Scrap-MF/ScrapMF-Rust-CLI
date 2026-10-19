use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::application::scraper::{ScrapeRequest, validate_output_path, validate_url};
use crate::config;
use crate::providers::gallery_dl::GalleryDl;

/// Handle `scrapmf scrape` command — Phase 2-4: validate, resolve preset, execute.
// CLI flags map 1:1 to parameters; grouping them would only obscure the call
// site in main.rs.
#[allow(clippy::too_many_arguments)]
pub fn run(
    url: String,
    output: Option<PathBuf>,
    preset: Option<String>,
    cookies: Option<PathBuf>,
    cookies_from_browser: Option<String>,
    dry_run: bool,
    no_archive: bool,
    profile_pic_only: bool,
    verbose: u8,
) -> Result<()> {
    let _ = verbose;
    // 1. Validate URL (trim, 2048, http/https)
    let validated = validate_url(&url)?;

    tracing::debug!(
        url = validated.as_str(),
        output = ?output,
        preset = ?preset,
        cookies = ?cookies,
        cookies_from_browser = ?cookies_from_browser,
        dry_run = dry_run,
        "scrape request"
    );

    // 2. Validate output path if provided
    if let Some(ref out) = output {
        validate_output_path(out).context("invalid output path")?;
    }
    // Validate cookies if provided
    if let Some(ref c) = cookies {
        crate::application::scraper::validate_cookies_file(c).context("invalid cookies file")?;
    }
    if let Some(ref b) = cookies_from_browser {
        crate::application::scraper::validate_cookies_browser(b)
            .context("invalid cookies browser")?;
    }

    // 3. Load config and resolve preset
    let cfg = config::load().unwrap_or_default();
    let resolved_preset = config::resolve_preset(validated.as_str(), preset.as_deref(), &cfg);
    if let Some(ref p) = resolved_preset {
        tracing::debug!(pattern = ?p.pattern, "resolved preset");
    }

    // 4. Build effective output (CLI > preset > general)
    let effective_output = output
        .clone()
        .or_else(|| resolved_preset.as_ref().and_then(|p| p.output_dir.clone()))
        .or_else(|| Some(config::expand_output_dir(&cfg.general.output_dir)));

    // Validate effective output if present
    if let Some(ref out) = effective_output {
        validate_output_path(out).context("invalid output path")?;
    }

    // 4b. Resolve site (for instagram structure) to get archive/rate_limit/templates
    let resolved_site = config::resolve_site(validated.as_str(), preset.as_deref(), &cfg);
    // 4c. Session precedence lives in one place, shared with the interactive
    // paths: explicit flag > named profile > cookies file > the global browser
    // choice > a site file that still carries the deprecated field.
    let (cookies_file, cookies_from_browser) = crate::config::cookies::resolve_session(
        resolved_site
            .as_ref()
            .and_then(|(_, s)| s.cookie_profile.as_deref()),
        cookies.clone(),
        cookies_from_browser.clone(),
        cfg.general.cookies_from_browser.clone(),
        resolved_site.as_ref().map(|(_, s)| &**s),
    );

    // 4d. Build ScrapeRequest DTO (site > preset > CLI)
    let req = ScrapeRequest {
        url: validated.as_str().to_string(),
        output: effective_output.clone(),
        preset: preset
            .clone()
            .or_else(|| resolved_site.as_ref().map(|(k, _)| (*k).clone())),
        extra_args: resolved_site
            .as_ref()
            .map(|(_, s)| s.extra_args.clone())
            .or_else(|| resolved_preset.as_ref().map(|p| p.extra_args.clone()))
            .unwrap_or_default(),
        cookies_from_browser: cookies_from_browser.clone(),
        cookies_file,
        archive: resolved_site.as_ref().and_then(|(_, s)| s.archive.clone()),
        rate_limit: resolved_site
            .as_ref()
            .and_then(|(_, s)| s.rate_limit.clone()),
        extractor_options: resolved_site
            .as_ref()
            .map(|(_, s)| s.extractor.clone())
            .unwrap_or_default(),
        filename_template: resolved_site
            .as_ref()
            .and_then(|(_, s)| s.filename_template.clone()),
        directory_template: resolved_site
            .as_ref()
            .and_then(|(_, s)| s.directory_template.clone()),
        extra_urls: Vec::new(),
        profile_name: None,
        extra_extractor_opts: Vec::new(),
        no_archive,
        profile_pic_only,
    };

    // 5. Provider args (gallery-dl, or the threads plugin for threads URLs)
    let provider: Box<dyn crate::providers::Provider> =
        if crate::application::scraper::is_threads_url(&req.url) {
            Box::new(crate::providers::threadstractor::Threadstractor)
        } else {
            Box::new(GalleryDl)
        };
    let args = provider
        .build_args(&req)
        .context("failed to build provider args")?;

    // Request summary only when useful: dry-run or verbose.
    // Cookie file paths are never printed without explicit verbosity.
    if dry_run || verbose > 0 {
        crate::output::print_info(&format!("Would scrape: {}", req.url));
        if let Some(out) = effective_output.clone() {
            crate::output::print_info(&format!("Output: {}", out.display()));
        }
        if let Some(p) = preset.clone() {
            crate::output::print_info(&format!("Preset: {p}"));
        }
        if verbose > 0 {
            if let Some(ref c) = cookies {
                crate::output::print_info(&format!("Cookies: {}", c.display()));
            }
            if let Some(ref b) = cookies_from_browser {
                crate::output::print_info(&format!("Cookies from browser: {b}"));
            }
        }
        tracing::debug!(args = ?args, "provider args");
        if verbose > 1 {
            crate::output::print_info(&format!("Args preview: {:?}", args));
        }
        if dry_run {
            crate::output::print_note("dry-run, no download");
        }
    }

    crate::output::print_info(&format!(
        "Provider: {} ({}available)",
        provider.name(),
        if provider.is_available() { "" } else { "not " }
    ));

    // 6. Execute real (Phase 4)
    let site_label = format!(
        "{}:{}",
        req.preset.as_deref().unwrap_or("gallery-dl"),
        validated.host_str().unwrap_or("site")
    );
    if dry_run {
        // For dry-run, add --get-urls and execute to list URLs
        println!("→ Executing dry-run (gallery-dl --get-urls)...");
    } else {
        println!("→ Executing gallery-dl...");
    }

    let downloaded: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let outcome = match run_scrape(&req, dry_run, &site_label, &downloaded) {
        Ok(outcome) => outcome,
        Err(e) => {
            // User abort (Ctrl+C in dashboard): clean message + exit code 130
            // (128+SIGINT convention) so scripts can distinguish it from errors.
            if e.chain().any(|c| c.to_string().contains("aborted by user")) {
                crate::output::print_note("cancelled by user");
                std::process::exit(130);
            }
            return Err(e).context("scrape failed");
        }
    };

    if !dry_run {
        // Post-scrape integrity check: truncated videos / .part leftovers.
        let paths = downloaded.lock().unwrap_or_else(|p| p.into_inner());
        let (checked, issues) =
            crate::application::integrity::verify_run(&paths, effective_output.as_deref());
        // Quality audit: resolution + codec of every verified media file
        if let Some(summary) = crate::application::integrity::quality_summary(&paths) {
            crate::output::print_info(&summary);
        }
        drop(paths);
        report_integrity(checked, &issues);
        // Termux MediaScan — single dir scan per finished download (best-effort)
        if let Some(dir) = effective_output.as_deref() {
            let _ = crate::application::media_scan::maybe_scan(dir);
        } else if let Some(dir) = crate::config::load()
            .ok()
            .map(|c| crate::config::expand_output_dir(&c.general.output_dir))
        {
            let _ = crate::application::media_scan::maybe_scan(&dir);
        }
    }

    if !outcome.skipped.is_empty() {
        crate::output::print_note(&format!(
            "{} sub-extractor(s) skipped (auth/unsupported):",
            outcome.skipped.len()
        ));
        for s in &outcome.skipped {
            println!("  - {s}");
        }
    }
    if !outcome.failed.is_empty() {
        crate::output::print_error(&format!(
            "{} sub-extractor(s) failed:",
            outcome.failed.len()
        ));
        for f in &outcome.failed {
            println!("  - {f}");
        }
    }
    crate::output::print_success(&format!("Done ({} sub-extractors)", outcome.success_count));

    // Actionable hint when posts were lost to anti-bot challenges
    if outcome.challenge_failures > 0 {
        crate::output::print_note(&format!(
            "{} post(s) failed due to TikTok JavaScript challenges — your browser session \
             is likely stale. Open tiktok.com in your configured browser, browse a bit, \
             then re-run to fetch the missing posts",
            outcome.challenge_failures
        ));
    }

    Ok(())
}

/// Run the scrape, attaching the dashboard + integrity path collection when
/// running on a TTY (dashboard mode), or plain inherited output otherwise.
fn run_scrape(
    req: &ScrapeRequest,
    dry_run: bool,
    site_label: &str,
    downloaded: &Arc<std::sync::Mutex<Vec<String>>>,
) -> anyhow::Result<crate::application::scraper::ScrapeOutcome> {
    if !dry_run && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        // TTY: dashboard, single job, NO log panel per design. gallery-dl
        // lines are mirrored into a persistent run log instead — the
        // alternate screen cannot be copied from.
        let state = Arc::new(std::sync::Mutex::new(crate::ui::DashboardState::new(vec![
            site_label.to_string(),
        ])));
        let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state2 = state.clone();
        let downloaded2 = downloaded.clone();
        let (user, site) = match site_label.split_once(':') {
            Some((s, u)) => (u.to_string(), s.to_string()),
            None => (site_label.to_string(), String::from("gallery-dl")),
        };
        let runlog = std::rc::Rc::new(std::cell::RefCell::new(
            crate::application::runlog::RunLog::open(&site, &user),
        ));
        let rl_out = runlog.clone();
        let mut hooks = crate::application::scraper::ScrapeHooks {
            on_steps_plan: None,
            on_step: None,
            on_file: Box::new(move |path: &str| {
                rl_out.borrow_mut().line(path);
                state2.lock().unwrap_or_else(|p| p.into_inner()).add_file(0);
                // Collect reported paths for the post-run integrity check
                downloaded2
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(path.to_string());
            }),
            on_log: Box::new({
                let rl = runlog.clone();
                move |line: &str| {
                    // No dashboard log feed by design; keep the persistent file
                    rl.borrow_mut().line(line);
                }
            }),
        };
        let result =
            crate::application::scraper::scrape_with_hooks(req, dry_run, Some(&mut hooks), &abort);
        let failed = result.is_err();
        let status = crate::application::runlog::status_line(&result);
        runlog.borrow_mut().finish(&status);
        if failed {
            eprintln!("log: {}", runlog.borrow().path.display());
        }
        result
    } else {
        crate::application::scraper::scrape(req, dry_run)
    }
}

/// Print the post-scrape integrity summary (truncated media, .part leftovers).
///
/// Three separate lists, because the remedy differs. An HEVC video is a
/// complete download that this toolchain will not play — telling the user to
/// delete it and re-run only destroys a good file and re-downloads identical
/// bytes. A file we could not read is a third case again: we know nothing
/// about it, so neither "delete" nor "you're fine" is true.
fn report_integrity(
    checked: usize,
    issues: &[(
        std::path::PathBuf,
        crate::application::integrity::IntegrityIssue,
    )],
) {
    if issues.is_empty() {
        if checked > 0 {
            tracing::debug!(checked = checked, "integrity check: all files OK");
        }
        return;
    }
    let (delete, leave, unknown) = crate::application::integrity::partition_by_remedy(issues);

    if !delete.is_empty() {
        crate::output::print_error(&format!(
            "{} downloaded file(s) look incomplete:",
            delete.len()
        ));
        for (path, issue) in &delete {
            println!("  - {} ({})", path.display(), issue.description());
        }
        crate::output::print_note(
            "delete the file(s) above and re-run — deterministic filenames re-download only what is missing",
        );
    }

    if !leave.is_empty() {
        // Info, not error: nothing is wrong with these files.
        crate::output::print_info(&format!(
            "{} file(s) downloaded fine but use an uncommon codec:",
            leave.len()
        ));
        for (path, issue) in &leave {
            println!("  - {} ({})", path.display(), issue.description());
        }
        crate::output::print_note(
            "these are complete downloads — leave them as they are. To play H.265/HEVC, use VLC, or re-encode with: ffmpeg -i <file> -c:v libx264 -crf 20 <out>",
        );
    }

    if !unknown.is_empty() {
        crate::output::print_error(&format!("{} file(s) could not be read:", unknown.len()));
        for (path, issue) in &unknown {
            println!("  - {} ({})", path.display(), issue.description());
        }
        crate::output::print_note(
            "these were reported as downloaded but could not be inspected, so completeness is unknown. Check the file permissions and free disk space, then re-run to confirm.",
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::application::scraper::validate_output_path;
    use std::path::PathBuf;

    #[test]
    fn rejects_root() {
        assert!(validate_output_path(&PathBuf::from("/")).is_err());
        assert!(validate_output_path(&PathBuf::from("/etc")).is_err());
    }

    #[test]
    fn rejects_parent_dir() {
        assert!(validate_output_path(&PathBuf::from("../out")).is_err());
        assert!(validate_output_path(&PathBuf::from("a/../b")).is_err());
    }

    #[test]
    fn accepts_normal() {
        assert!(validate_output_path(&PathBuf::from("./downloads")).is_ok());
        assert!(validate_output_path(&PathBuf::from("/tmp/out")).is_ok());
        assert!(validate_output_path(&PathBuf::from("~/Pictures/scrapmf")).is_ok());
    }
}
