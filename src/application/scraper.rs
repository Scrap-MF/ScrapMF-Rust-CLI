use url::Url;

use crate::error::ScrapmfError;
use crate::providers::Provider;

/// Validate URL: trim, length ≤2048, Url::parse, allow only http/https.
pub fn validate_url(raw: &str) -> Result<Url, ScrapmfError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(ScrapmfError::InvalidUrl {
            url: "URL is empty".to_string(),
        });
    }
    if s.len() > 2048 {
        return Err(ScrapmfError::InvalidUrl {
            url: format!(
                "URL too long ({} > 2048): {}",
                s.len(),
                &s[..s.len().min(50)]
            ),
        });
    }
    let url = Url::parse(s).map_err(|_| ScrapmfError::InvalidUrl { url: s.to_string() })?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(ScrapmfError::InvalidUrl {
            url: format!("unsupported scheme '{}': {}", other, s),
        }),
    }
}

/// Core scraper service — orchestrates provider + executor.
/// Stub for Phase 4. Will contain business logic without CLI concerns.
#[derive(Clone, Debug, Default)]
pub struct ScrapeOutcome {
    pub success_count: usize,
    pub failed: Vec<String>,
    pub skipped: Vec<String>,
    /// Posts lost to anti-bot JavaScript challenges (rehydration attempts
    /// exhausted). Usually means the browser session is stale — the user
    /// should refresh it before re-running.
    pub challenge_failures: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ScrapeRequest {
    pub url: String,
    pub output: Option<std::path::PathBuf>,
    pub preset: Option<String>,
    pub extra_args: Vec<String>,
    pub cookies_from_browser: Option<String>,
    pub cookies_file: Option<std::path::PathBuf>,
    /// Explicit archive override. When `None`, the automatic per-account
    /// archive (see `crate::application::archive`) kicks in unless
    /// [`Self::no_archive`] is set.
    pub archive: Option<std::path::PathBuf>,
    /// Disable dedup entirely for this request (CLI `--no-archive`).
    pub no_archive: bool,
    /// Threads: download only profile picture (avatar)
    pub profile_pic_only: bool,
    pub rate_limit: Option<crate::config::RateLimit>,
    pub extractor_options: std::collections::HashMap<String, toml::Value>,
    pub filename_template: Option<String>,
    pub directory_template: Option<Vec<String>>,
    pub extra_urls: Vec<String>,
    /// Profile name (person) — injected as `scrapmf_root` gallery-dl keyword so
    /// directory templates can start with {scrapmf_root} (profile/network/account/content).
    pub profile_name: Option<String>,
    /// Additional raw `-o key=value` pairs appended AFTER site extractor
    /// options (so they win). Used e.g. for the twitter two-pass photos/videos
    /// split (static directory override + file-filter per pass).
    pub extra_extractor_opts: Vec<(String, String)>,
}

/// System directory trees that must never be used as scrape output.
/// Matched as component-prefixes, so `/etc` also rejects `/etc/anything`.
/// `/opt` is deliberately NOT protected (legitimate content location).
const PROTECTED_ROOTS: &[&str] = &[
    "/", "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/proc", "/root", "/run",
    "/sbin", "/sys", "/usr", "/var",
];

/// Validate output path: reject empty, protected system trees (including any
/// path inside them), and ParentDir `..`.
pub fn validate_output_path(path: &std::path::Path) -> anyhow::Result<()> {
    use std::path::{Component, Path};
    if path.as_os_str().is_empty() {
        anyhow::bail!("output path is empty");
    }
    // Expanded through the shared helper, not an inline copy: validating the
    // raw string would let `$HOME/etc` pass the `/etc` protection below, since
    // the literal string never starts with a protected root.
    let expanded = crate::util::expand_tilde(path);
    // Prefix match on components: `/etc` protects `/etc/passwd_backup` too.
    // The bare filesystem root `/` is special-cased: it only rejects the root
    // itself, since every absolute path technically starts with it.
    for root in PROTECTED_ROOTS {
        let protected = if *root == "/" {
            expanded == Path::new("/")
        } else {
            expanded.starts_with(root)
        };
        if protected {
            anyhow::bail!(
                "output path '{}' is inside protected system directory '{root}'",
                path.display()
            );
        }
    }
    if expanded
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        anyhow::bail!(
            "output path '{}' contains '..' (directory traversal not allowed)",
            path.display()
        );
    }
    Ok(())
}

const ALLOW_LIST: &[&str] = &[
    "--proxy",
    "--user-agent",
    "--sleep",
    "--sleep-request",
    "--sleep-429",
    "--limit-rate",
    "--cookies",
    "--cookies-from-browser",
    "--restrict-filenames",
    "--destination",
    "--get-urls",
    "--photos-only",
    "--videos-only",
    // Download robustness (flaky CDNs, large videos): retry/timeout tuning
    "--retries",
    "--http-timeout",
    "--force-ipv4",
];

pub fn validate_extra_args(args: &[String]) -> anyhow::Result<()> {
    for arg in args {
        if arg.contains([';', '|', '&', '$', '>', '<', '`']) {
            anyhow::bail!("extra_args contains forbidden chars in '{arg}'");
        }
        if arg == "--exec" || arg.starts_with("--exec=") {
            anyhow::bail!("extra_args forbidden flag '--exec'");
        }
        if arg.starts_with("--") {
            let allowed = ALLOW_LIST
                .iter()
                .any(|allow| arg == *allow || arg.starts_with(&format!("{allow}=")));
            if !allowed {
                anyhow::bail!(
                    "extra_args forbidden flag '{arg}' (allowed: {})",
                    ALLOW_LIST.join(", ")
                );
            }
        }
    }
    Ok(())
}

pub fn validate_cookies_file(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.is_file() {
        anyhow::bail!("cookies file '{}' not found or not a file", path.display());
    }
    // Check readable and not world-readable is optional warn, but check exists
    let md = std::fs::metadata(path)?;
    if md.len() == 0 {
        anyhow::bail!("cookies file '{}' is empty", path.display());
    }
    Ok(())
}

pub fn validate_cookies_browser(browser: &str) -> anyhow::Result<()> {
    let id = browser.split(':').next().unwrap_or(browser);
    // Derived from CHANNELS so a channel that exists can be requested. This
    // used to be a second hand-written list, which silently omitted every
    // Brave variant (`brave-origin`, `brave-beta`, …) — they were in
    // CHANNELS and readable by the capture path, but no request could name
    // them, so a user whose session lives in Brave Origin could only ask for
    // Brave-Browser and got the wrong profile. `whale` was listed here with
    // no channel behind it and no reader for it; it was dead weight.
    let allowed: Vec<&str> = crate::browsers::CHANNELS.iter().map(|c| c.id).collect();
    if !allowed.contains(&id) {
        anyhow::bail!(
            "unsupported browser '{id}' for --cookies-from-browser (allowed: {})",
            allowed.join(", ")
        );
    }
    Ok(())
}

/// True when a gallery-dl stderr line reports a post whose rehydration data
/// could NOT be retrieved after exhausting all attempts — e.g.
/// `@user: Failed to retrieve rehydration data (10/10)`.
/// That post is lost for this run; the usual cause is an expired/flagged
/// browser session being JavaScript-challenged on every request.
pub fn is_challenge_exhausted_line(line: &str) -> bool {
    let Some((_, tail)) = line.split_once("Failed to retrieve rehydration data") else {
        return false;
    };
    // Look for "(N/M)" anywhere after the message; exhausted iff N == M > 0.
    let bytes = tail.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'('
            && let Some(close) = tail[i..].find(')')
        {
            let inner = &tail[i + 1..i + close];
            if let Some((cur, max)) = inner.split_once('/')
                && let (Ok(c), Ok(m)) = (cur.trim().parse::<u32>(), max.trim().parse::<u32>())
            {
                return c == m && c > 0;
            }
        }
    }
    false
}

/// Optional UI hooks so the presentation layer (indicatif/log panel) can
/// observe progress without application knowing about any UI crate.
type PlanFn<'a> = dyn FnMut(Vec<String>) + 'a;

pub struct ScrapeHooks<'a> {
    /// Called once per downloaded/skipped media file with its stdout line
    /// (the file path printed by gallery-dl).
    pub on_file: Box<dyn FnMut(&str) + 'a>,
    /// Called per gallery-dl stderr line (warnings, errors, notices).
    pub on_log: Box<dyn FnMut(&str) + 'a>,
    /// Called once per job with the full sub-extractor plan (labels in run
    /// order) — drives the dashboard's checklist.
    pub on_steps_plan: Option<Box<PlanFn<'a>>>,
    /// Called before each sub-extractor URL runs (1-based index).
    pub on_step: Option<Box<dyn FnMut(usize) + 'a>>,
}

/// The value of the `{scrapmf_root}` keyword for this request.
///
/// Shared with [`crate::providers::gallery_dl`]: anything that has to land
/// beside the media gallery-dl writes has to resolve the root exactly the way
/// gallery-dl does, and two copies of this rule would eventually disagree
/// about where a file belongs.
pub fn scrapmf_root(req: &ScrapeRequest) -> String {
    req.profile_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| {
            crate::application::archive::site_account_from_url(&req.url).map(|(_, account)| account)
        })
        .unwrap_or_else(|| "default".to_string())
}

/// Short process name for a sub-extractor URL — dashboard progress label.
pub fn sub_url_label(url: &str) -> &'static str {
    // Checked first: `/stories/USER/` contains no other marker, but the id
    // view is `/USER/info/` and must not fall through to "posts".
    if crate::application::instagram_id::is_info_url(url) {
        return "Instagram_id";
    }
    if url.contains("/stories/") {
        return "stories";
    }
    if url.contains("/highlights/") {
        return "highlights";
    }
    if url.contains("/reels/") {
        return "reels";
    }
    if url.contains("/photos_albums") {
        return "albums";
    }
    if url.contains("/photos") {
        return "photos";
    }
    if url.contains("/videos") {
        return "videos";
    }
    if url.contains("/avatar") {
        return "profile";
    }
    // Profile root passes and threads base URLs download the media feed.
    "posts"
}

pub fn scrape(req: &ScrapeRequest, dry_run: bool) -> anyhow::Result<ScrapeOutcome> {
    let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    scrape_with_hooks(req, dry_run, None, &never)
}

pub fn scrape_with_hooks(
    req: &ScrapeRequest,
    dry_run: bool,
    mut hooks: Option<&mut ScrapeHooks>,
    abort: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<ScrapeOutcome> {
    validate_extra_args(&req.extra_args)?;
    if let Some(ref f) = req.cookies_file {
        validate_cookies_file(f)?;
    }
    if let Some(ref b) = req.cookies_from_browser {
        validate_cookies_browser(b)?;
    }
    if let Some(ref out) = req.output {
        validate_output_path(out)?;
        if !dry_run {
            let expanded = crate::config::expand_output_dir(out);
            std::fs::create_dir_all(&expanded)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perm = std::fs::Permissions::from_mode(0o755);
                let _ = std::fs::set_permissions(&expanded, perm);
            }
        }
    }

    // ── Download archive (dedup) ────────────────────────────────────────────
    // Canonical store is ours (JSONL per site/account). gallery-dl reads a
    // disposable sqlite cache seeded from it and drained back afterwards;
    // threads keeps its own ledger beside the media, so ours is bridged into
    // it directly instead. Both end up in the same JSONL.
    let mut req = req.clone();
    let mut archive_keys: std::collections::HashSet<String> = Default::default();
    // (our entries JSONL, the store this provider was actually given)
    let mut archive_files: Option<(std::path::PathBuf, std::path::PathBuf)> = None;
    let mut threads_ledger: Option<std::path::PathBuf> = None;
    if !req.no_archive && req.archive.is_none() && !dry_run {
        use crate::application::archive;
        let enabled = crate::config::load()
            .map(|c| c.general.archive)
            .unwrap_or(true);
        if enabled
            && let Some((site, account)) = archive::site_account_from_url(&req.url)
            && let Some(entries) = archive::entries_path(&site, &account)
        {
            archive_keys = archive::load_keys(&entries).unwrap_or_default();
            if is_threads_url(&req.url) {
                // The plugin resolves its ledger from `--dest`, so the dest has
                // to be derived exactly as the provider derives it. Without an
                // explicit output the plugin falls back to `dl` in the cwd,
                // which is not a path worth guessing at.
                if let Some(out) = &req.output {
                    let dest = crate::config::expand_output_dir(out);
                    let ledger = crate::application::threads_archive::ledger_path(&dest);
                    match crate::application::threads_archive::inject(&ledger, &archive_keys) {
                        Ok(n) => tracing::debug!(
                            site = %site, account = %account,
                            keys = archive_keys.len(), injected = n,
                            "threads ledger seeded from archive"
                        ),
                        Err(e) => tracing::warn!(
                            error = %e,
                            "could not seed the threads ledger — proceeding without dedup"
                        ),
                    }
                    // Its presence is what makes the provider pass `--archive`.
                    req.archive = Some(ledger.clone());
                    archive_files = Some((entries, ledger.clone()));
                    threads_ledger = Some(ledger);
                }
            } else if let Some(cache) = archive::cache_path(&site, &account) {
                match archive::seed_cache(&cache, &archive_keys) {
                    Ok(()) => {
                        tracing::debug!(
                            site = %site, account = %account,
                            keys = archive_keys.len(),
                            "archive cache seeded"
                        );
                        req.archive = Some(cache.clone());
                        archive_files = Some((entries, cache));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "could not seed download archive — proceeding without dedup");
                    }
                }
            }
        }
    }

    let mut outcome = ScrapeOutcome::default();
    let urls: Vec<&String> = std::iter::once(&req.url)
        .chain(req.extra_urls.iter())
        .collect();
    if let Some(hooks) = hooks.as_deref_mut()
        && let Some(f) = hooks.on_steps_plan.as_mut()
    {
        f(urls.iter().map(|u| sub_url_label(u).to_string()).collect());
    }
    if let Some(hooks) = hooks.as_deref_mut() {
        let banner = session_banner(&req);
        let f = &mut hooks.on_log;
        f(&banner);
    }
    for (i, sub_url) in urls.into_iter().enumerate() {
        if let Some(hooks) = hooks.as_deref_mut()
            && let Some(f) = hooks.on_step.as_mut()
        {
            f(i + 1);
        }
        // `/info/` is not a media extractor — gallery-dl resolves the account
        // and emits a directory message, downloading nothing while still
        // reporting success. Handled here so the id lands in a file instead.
        if crate::application::instagram_id::is_info_url(sub_url) {
            match resolve_instagram_id(&req, sub_url, dry_run, hooks.as_deref_mut()) {
                Ok(()) => {
                    outcome.success_count += 1;
                }
                Err(e) => {
                    tracing::warn!(url = %sub_url, error = %e, "could not resolve the account id");
                    if let Some(hooks) = hooks.as_deref_mut() {
                        let f = &mut hooks.on_log;
                        f(&format!("Instagram_id: {e}"));
                    }
                    outcome.failed.push(format!("{sub_url}: {e}"));
                }
            }
            continue;
        }
        // A throttled sub-extractor is retried rather than accepted: the site
        // asked us to slow down, which is a wait-and-continue, not a failure.
        // gallery-dl does not retry a 429 itself (its `retry-codes` is empty
        // for Instagram), so without this a single throttle silently costs the
        // whole run and reports itself as a privacy problem.
        let mut attempt = 0u32;
        loop {
            match run_one_sub_scrape(&req, sub_url, dry_run, hooks.as_deref_mut(), abort) {
                Ok(challenge_failures) => {
                    outcome.success_count += 1;
                    outcome.challenge_failures += challenge_failures;
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    let kind = classify_failure(&msg);
                    // "Not found" with no explanation is the least useful
                    // message scrapmf can print: Instagram returns it for a
                    // profile that does not exist, one that is private and not
                    // followed by the session's account, and one whose session
                    // the site refused outright. It cannot be told apart from
                    // the wording, so say which account was sent instead — that
                    // is what the reader can act on.
                    let msg = if kind == FailureKind::Unknown && is_not_found(&msg) {
                        match session_hint(&req, sub_url, outcome.success_count > 0) {
                            Some(h) => format!("{msg}\n  note: {h}"),
                            None => msg,
                        }
                    } else {
                        msg
                    };
                    if kind == FailureKind::RateLimited
                        && attempt < MAX_RATE_LIMIT_RETRIES
                        && !abort.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        let wait = rate_limit_backoff_secs(attempt);
                        tracing::warn!(
                            url = %sub_url,
                            attempt = attempt + 1,
                            wait_secs = wait,
                            "rate limited; backing off before retry"
                        );
                        if let Some(hooks) = hooks.as_deref_mut() {
                            let f = &mut hooks.on_log;
                            f(&format!(
                                "[rate limited] {sub_url}: site throttled this request \
                                 (HTTP 429); retrying in {wait}s (attempt {}/{})",
                                attempt + 1,
                                MAX_RATE_LIMIT_RETRIES + 1
                            ));
                        }
                        // Sleep in slices so Ctrl+C stays responsive; a single
                        // multi-minute sleep would make the run look hung and
                        // would ignore the abort flag until it finished.
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(wait);
                        loop {
                            if abort.load(std::sync::atomic::Ordering::Relaxed) {
                                break;
                            }
                            let left =
                                deadline.saturating_duration_since(std::time::Instant::now());
                            if left.is_zero() {
                                break;
                            }
                            std::thread::sleep(left.min(std::time::Duration::from_secs(1)));
                        }
                        if abort.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        attempt += 1;
                        continue;
                    }
                    match kind {
                        FailureKind::RateLimited => {
                            // Out of retries: report it as the throttle it is,
                            // not as the privacy verdict gallery-dl guessed.
                            outcome
                                .failed
                                .push(format!("[rate limited] {sub_url}: {msg}"));
                            tracing::warn!(
                                url = %sub_url,
                                attempts = attempt + 1,
                                "rate limited after retries; giving up"
                            );
                        }
                        FailureKind::AuthRequired => {
                            outcome.skipped.push(format!("{sub_url}: {msg}"));
                            tracing::warn!(
                                url = %sub_url,
                                error = %e,
                                "sub-extractor skipped (auth required)"
                            );
                        }
                        FailureKind::Unsupported => {
                            outcome
                                .skipped
                                .push(format!("[unsupported] {sub_url}: {msg}"));
                            tracing::warn!(
                                url = %sub_url,
                                error = %e,
                                "sub-extractor skipped (unsupported URL)"
                            );
                        }
                        FailureKind::Unknown => {
                            outcome.failed.push(format!("{sub_url}: {msg}"));
                            tracing::debug!(url = %sub_url, error = %e, "sub-extractor failed");
                        }
                    }
                    break;
                }
            }
        }
    }
    if outcome.success_count == 0 && outcome.failed.is_empty() && outcome.skipped.is_empty() {
        // No sub-url succeeded and none reported — treat as overall failure
        anyhow::bail!("all sub-extractors failed or were skipped");
    }
    // If at least one succeeded, return outcome even if some failed/skipped
    if outcome.success_count == 0 && !outcome.failed.is_empty() {
        // All failed (no skipped) → propagate error
        let details = outcome.failed.join("; ");
        anyhow::bail!("scrape failed: {details}");
    }

    // Drain the store the provider actually wrote: keys gallery-dl inserted
    // this run become permanent JSONL entries, and for threads the plugin's
    // ledger is read back the same way. Best-effort — a failure here must not
    // fail the scrape.
    if let Some((entries, store)) = archive_files {
        let drained = if threads_ledger.is_some() {
            crate::application::threads_archive::drain(&store).map_err(anyhow::Error::from)
        } else {
            crate::application::archive::drain_cache(&store)
        };
        match drained {
            Ok(all) => {
                match crate::application::archive::append_entries(&entries, &archive_keys, all) {
                    Ok(n) if n > 0 => {
                        tracing::info!(new_entries = n, "download archive updated");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "could not append archive entries"),
                }
            }
            Err(e) => {
                if threads_ledger.is_some() {
                    tracing::warn!(error = %e, "could not read the threads ledger");
                } else {
                    tracing::warn!(error = %e, "could not read download archive cache");
                }
            }
        }
    }
    Ok(outcome)
}

/// Resolve the account id behind an `/info/` URL and write `Instagram_id.txt`.
///
/// On a dry run the lookup still happens — it is a read, not a download — and
/// the destination is printed instead of the file being created, so `--dry-run`
/// shows where the id would land.
fn resolve_instagram_id(
    req: &ScrapeRequest,
    url: &str,
    dry_run: bool,
    hooks: Option<&mut ScrapeHooks>,
) -> anyhow::Result<()> {
    let account = crate::application::instagram_id::account_from_info_url(url).unwrap_or_default();
    let path = crate::application::instagram_id::target_path(req, &account);
    let id = crate::application::instagram_id::resolve(req, url)?;

    if dry_run {
        tracing::info!(url = %url, id = %id, path = %path.display(), "would write the account id");
        if let Some(hooks) = hooks {
            let f = &mut hooks.on_log;
            f(&format!(
                "Instagram_id: {id} -> would write {}",
                path.display()
            ));
        }
        return Ok(());
    }

    crate::application::instagram_id::write_id(&path, &id)?;
    tracing::info!(url = %url, id = %id, path = %path.display(), "account id written");
    if let Some(hooks) = hooks {
        let f = &mut hooks.on_log;
        f(&format!("Instagram_id: {id} → {}", path.display()));
    }
    Ok(())
}

/// Why a sub-extractor run did not produce downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// gallery-dl needs a valid session (cookies expired / private content).
    AuthRequired,
    /// No extractor can handle the URL — retrying with cookies won't help.
    Unsupported,
    /// The site throttled the request (HTTP 429). Distinct from auth: the
    /// session is fine and the content may be reachable later, so this is the
    /// one kind worth waiting out and retrying.
    RateLimited,
    /// Anything else: network errors, extractor bugs, unexpected output.
    Unknown,
}

/// True when gallery-dl reported that the requested resource does not exist.
fn is_not_found(msg: &str) -> bool {
    msg.contains("NotFoundError") || msg.contains("could not be found")
}

/// What session this run puts on the wire, and which Instagram account it
/// belongs to.
///
/// `(origin, account)`. `origin` is empty for an anonymous run. `account` is
/// `None` when the session carries no `ds_user_id` — genuinely unknown, which
/// is different from "no session" and must not be rendered as the same thing.
fn session_identity(req: &ScrapeRequest) -> (String, Option<String>) {
    match (&req.cookies_file, &req.cookies_from_browser) {
        (Some(file), _) => (
            format!("cookie file {}", file.display()),
            crate::config::cookies::instagram_account_hint_file(file),
        ),
        (None, Some(browser)) => (
            format!("browser {browser}"),
            crate::config::cookies::instagram_account_for_channel(browser),
        ),
        (None, None) => (String::new(), None),
    }
}

/// One line naming the session, printed before any sub-extractor runs.
///
/// Until v1.11.2 the only place scrapmf ever named the Instagram account was
/// inside an error message, so choosing the wrong profile was something you
/// could only discover by failing. Switching accounts in the browser also used
/// to be invisible: the capture was replayed until it covered the target, and
/// the previous account kept answering for the new one. Naming it up front
/// costs one line and removes both blind spots.
fn session_banner(req: &ScrapeRequest) -> String {
    let (origin, account) = session_identity(req);
    if origin.is_empty() {
        return "session: none — only public content is reachable".to_string();
    }
    match account {
        Some(a) => format!("session: Instagram account {a} via {origin}"),
        None => format!("session: {origin} (no Instagram account id in it)"),
    }
}

/// Report what is *known* about an unclassifiable "not found", and nothing more.
///
/// gallery-dl prints one sentence for at least four unrelated situations and
/// names none of them:
///
/// * the account does not exist
/// * it exists, is private, and this session does not follow it
/// * the site refused this session outright
/// * gallery-dl's own user lookup failed — before v1.32.17 the Instagram stories
///   path raised `NotFoundError` without ever asking Instagram
///
/// A previous version of this note asserted the third was the only possibility.
/// That was wrong, and it pointed a whole debugging session at the session
/// while the real cause was the pinned backend being a release behind. So the
/// rule here is: state the session, the account, the sub-extractor and the
/// backend version, list the causes without ranking them, and let the reader
/// decide.
///
/// `ran_before` says an earlier URL in this same run already succeeded on this
/// same session, which is the one piece of evidence that can actually clear the
/// session of suspicion.
fn session_hint(req: &ScrapeRequest, sub_url: &str, ran_before: bool) -> Option<String> {
    let kind = sub_url_label(sub_url);
    let (origin, account) = session_identity(req);

    // Anonymous: the one case where the cause really is knowable, because
    // without a session nothing private or follower-gated is reachable at all.
    if origin.is_empty() {
        return Some(format!(
            "no session was sent, so only public content is reachable — \
             the {kind} pass on this URL needs a logged-in Instagram account"
        ));
    }

    let head = match &account {
        Some(a) => format!("sent Instagram account {a} via {origin}"),
        None => format!("sent {origin} (no Instagram account id in it)"),
    };
    let mut lines = vec![match crate::application::backend::installed_version() {
        Some(v) => format!("{head}, gallery-dl {v}"),
        None => head,
    }];

    if ran_before {
        lines.push(format!(
            "an earlier URL in this run succeeded on this same session, so the site is \
             accepting it — this is the {kind} extractor failing, not the session"
        ));
    }

    lines.push(format!(
        "\"{kind}\" reported not-found, which means one of: account missing / private \
         and not followed by this session / session refused / gallery-dl's own user \
         lookup failed. Same sentence for all four, so it points at none of them"
    ));

    if !ran_before {
        lines.push(
            "next: retry this URL on its own, then with another session \
             (Configuration → Cookie profiles, or --cookies <file>)"
                .to_string(),
        );
    }

    if let Some(drift) = crate::application::backend::pin_drift_notice() {
        lines.insert(0, drift);
    }

    Some(lines.join("\n  "))
}

/// True when a gallery-dl message shows the site throttled the request.
///
/// This exists because gallery-dl misreports a 429 as a privacy problem. Its
/// Instagram lookup asks for `followed_by_viewer` and `friendship_status`; when
/// the API answers 429 those fields never arrive, and gallery-dl reads their
/// absence as "you do not follow this account" and logs `posts are private`:
///
/// ```python
/// if check_private and user.get("is_private") and (
///         not user.get("followed_by_viewer", True) or ...)
/// ```
///
/// The two situations print the same line and need opposite responses — wait,
/// versus go follow someone — so the distinction has to be made before the
/// message reaches the user. The 429 itself is what separates them.
pub fn is_rate_limited(msg: &str) -> bool {
    // Match the status code only where it reads as an HTTP status: "429" on its
    // own also occurs inside signed CDN URLs (an `oe=`/`_nc_` parameter), so a
    // bare substring test would fire on a perfectly healthy download.
    msg.contains("429 Too Many Requests")
        || msg.contains("HTTP Error 429")
        || msg.contains("HTTP error 429")
        || msg.contains("Error 429")
        || msg.contains("Too Many Requests")
}

/// True when gallery-dl logged the Instagram "posts are private" warning.
pub fn is_private_warning(msg: &str) -> bool {
    msg.contains("posts are private")
}

/// Retry budget per sub-extractor after a 429.
///
/// Two retries at 60s and 120s. Beyond that a run would spend minutes asleep
/// and the throttle is more likely to be an account-level block that waiting
/// out does not clear, so it is better to stop and say so.
const MAX_RATE_LIMIT_RETRIES: u32 = 2;

/// Backoff before retrying a throttled sub-extractor, in seconds.
///
/// Instagram's limit is per-account and recovers on the order of minutes, so
/// a retry that fires in seconds would just earn a second 429 and deepen the
/// throttle. Doubling from 60s gives two chances inside a couple of minutes
/// without hammering.
fn rate_limit_backoff_secs(attempt: u32) -> u64 {
    const BASE: u64 = 60;
    BASE.saturating_mul(1u64 << attempt.min(4)).min(15 * 60)
}

/// Classify a sub-extractor error message using gallery-dl's real failure
/// signatures. Conservative: only well-known patterns map to the benign
/// categories; everything else is treated as a hard failure.
pub fn classify_failure(msg: &str) -> FailureKind {
    // Checked before auth: a throttled run also tends to contain 403s and
    // "private" wording, and telling the user to fix their session when the
    // site simply asked them to slow down sends them to the wrong remedy.
    if is_rate_limited(msg) {
        return FailureKind::RateLimited;
    }
    // gallery-dl raises its own AuthRequired exception for these cases.
    if msg.contains("AuthRequired")
        || msg.contains("401 Unauthorized")
        || msg.contains("403 Forbidden")
        || msg.contains("login required")
        // TikTok private profiles: webapp.user-detail returns statusCode
        // 10222 and gallery-dl logs this exact sentence (the profile-page
        // fetch fails even for accounts your session follows — the story
        // list extractor needs the authorId from that page). Direct
        // /video/<id> URLs still work; surface as auth-skip, not a crash.
        || msg.contains("Login required to access this profile")
        || msg.contains("private account")
        || is_private_warning(msg)
        || msg.contains("authentication required")
    {
        return FailureKind::AuthRequired;
    }
    if msg.contains("Unsupported URL") || msg.contains("no suitable extractor") {
        return FailureKind::Unsupported;
    }
    FailureKind::Unknown
}

/// True when `url` is the Instagram profile root — the "posts" pass.
///
/// gallery-dl maps this URL to the dispatching user extractor, whose primary
/// sub-extractor (`instagram.posts`) reads the feed endpoint. Instagram mixes
/// reels into that feed, so without a filter every reel downloads twice:
/// once into `posts/` here and again into `reels/` in the dedicated pass
/// (different directories defeat gallery-dl's existing-file skip).
pub fn is_instagram_posts_pass(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let Some(path) = rest.strip_prefix("instagram.com/") else {
        return false;
    };
    // Drop query string / fragment, then require exactly one path segment
    // (the username), optional trailing slash. Sub-pages like USER/reels/,
    // USER/avatar or stories/USER/ contain '/'.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let seg = path.trim_end_matches('/');
    !seg.is_empty() && !seg.contains('/')
}

/// Per-sub-url overrides applied right before building argv.
///
/// Instagram dedup (always on): scope a post-filter to the
/// `instagram.posts` sub-extractor only. Posts carrying metadata
/// `type == "reel"` are skipped here; they download exactly once, from the
/// dedicated `/USER/reels/` pass. The filter is scoped per-subcategory so a
/// queued dispatch inside the same run cannot suppress the other extractors,
/// and missing `type` (e.g. GraphQL path) evaluates to keep — fail-open: worst
/// case is a duplicate download, never lost content.
fn apply_sub_url_overrides(req: &mut ScrapeRequest, url: &str) {
    if is_instagram_posts_pass(url) {
        req.extra_extractor_opts.push((
            "extractor.instagram.posts.post-filter".to_string(),
            "type != 'reel'".to_string(),
        ));
    }
}

/// Turn a `cookies_from_browser` request into an explicit Netscape profile so
/// gallery-dl authenticates with the session this run actually intended.
///
/// gallery-dl resolves the browser name to a profile directory itself, and its
/// Brave mapping is a fallback *tuple*:
///
/// ```python
/// "brave": (join(config, "BraveSoftware/Brave-Browser"),
///           join(config, "BraveSoftware/Brave-Origin"))
/// ```
///
/// It uses `Brave-Browser` whenever that directory exists and only falls back
/// to `Brave-Origin` otherwise. When both are installed, a logged-out or
/// server-side-revoked session in the first shadows a healthy one in the
/// second, and Instagram answers `HTTP redirect to home page` while scrapmf's
/// own cookie count looks correct — the two processes read different profiles.
/// Capturing through scrapmf removes that ordering dependency.
///
/// Any failure here (locked OS keyring, undecryptable database, unresolvable
/// site) leaves the request untouched, so `--cookies-from-browser` still runs
/// exactly as before. This can only narrow the session, never widen the blast
/// radius of a failure.
fn resolve_cookie_capture(req: &mut ScrapeRequest) {
    if req.cookies_file.is_some() {
        return;
    }
    let Some(browser) = req.cookies_from_browser.clone() else {
        return;
    };
    // A profile suffix (`brave:Profile 2`) only means something to the browser;
    // scrapmf's capture reads the default profile, so leave those alone rather
    // than silently scraping the wrong one.
    if browser.contains(':') {
        return;
    }
    let Some(site) = crate::sites::registry::find_by_url(&req.url) else {
        return;
    };
    let sites = vec![site.id.to_string()];
    let profile = format!("{}{browser}", crate::config::cookies::AUTO_PROFILE_PREFIX);
    // Reuse the cache when it already covers this site. Re-reading the
    // browser hits the OS keyring over D-Bus for every sub-extractor, and the
    // session does not change mid-run, so the extra reads bought nothing. A
    // cache that lacks this site is recaptured rather than used blind.
    let cached = crate::config::cookies::profile_path(&profile)
        .filter(|p| p.is_file())
        .filter(|_| {
            crate::config::cookies::load_profile(&profile)
                .map(|cs| {
                    cs.iter()
                        .any(|c| sites.iter().any(|s| c.domain.contains(s.as_str())))
                })
                .unwrap_or(false)
        });
    // A capture that still covers the site can still be out of date. Browsers
    // rotate the session cookie on their own — Instagram refreshed one within
    // the minute while this check was being written — and the site then refuses
    // the stale value immediately, which looks exactly like a missing account.
    //
    // Comparing against the browser's own cookie database is exact, where a
    // fixed age threshold would only guess: if the database has been written
    // since the capture was taken, the capture predates it and is stale.
    let stale = cached
        .as_ref()
        .is_some_and(|p| crate::config::cookies::capture_predates_browser(p, &browser));
    if let Some(path) = cached.filter(|_| !stale) {
        tracing::debug!("reusing captured cookie profile {}", path.display());
        req.cookies_file = Some(path);
        req.cookies_from_browser = None;
        return;
    }
    if stale {
        tracing::info!(
            "{browser}: captured session is older than the browser's cookie database — \
             re-capturing"
        );
    }
    let captured = if browser.eq_ignore_ascii_case("firefox") {
        crate::config::cookies::capture_firefox(&sites, &profile)
    } else {
        crate::config::cookies::capture_chromium(&browser, &sites, &profile)
    };
    match captured {
        Ok((path, count)) => {
            tracing::debug!(
                "captured {count} cookie(s) from {browser} into {}",
                path.display()
            );
            req.cookies_file = Some(path);
            req.cookies_from_browser = None;
        }
        Err(e) => {
            tracing::debug!(
                "cookie capture from {browser} unavailable ({e}); \
                 keeping --cookies-from-browser"
            );
            // A locked keyring must not cost the user their download. If a stale
            // capture exists, use it: a session from yesterday usually still
            // beats no session at all, and the site decides whether it works.
            if let Some(path) =
                crate::config::cookies::profile_path(&profile).filter(|p| p.is_file())
            {
                tracing::warn!(
                    "{browser}: could not refresh the captured session ({e}); \
                     falling back to the stored copy"
                );
                req.cookies_file = Some(path);
                req.cookies_from_browser = None;
            }
        }
    }
}

pub fn is_threads_url(url: &str) -> bool {
    url.contains("threads.com") || url.contains("threads.net")
}

fn run_one_sub_scrape(
    req: &ScrapeRequest,
    url: &str,
    dry_run: bool,
    hooks: Option<&mut ScrapeHooks>,
    abort: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<usize> {
    // Pick provider like gallery-dl: threads URLs go to the threads plugin
    let is_threads = is_threads_url(url);
    if is_threads && !crate::plugins::threads_enabled() {
        anyhow::bail!(
            "threads plugin is not enabled — this URL needs it. \
             Enable via scrapmf → Plugins (installs threadstractormf {})",
            crate::plugins::THREADSTRACTOR_PIN
        );
    }
    let (binary, mut args) = if is_threads {
        let p = crate::providers::threadstractor::Threadstractor;
        let bin = crate::providers::threadstractor::Threadstractor::binary()?
            .to_string_lossy()
            .into_owned();
        let mut sub_req = req.clone();
        sub_req.url = url.to_string();
        apply_sub_url_overrides(&mut sub_req, url);
        let a = p.build_args(&sub_req)?;
        (bin, a)
    } else {
        let p = crate::providers::gallery_dl::GalleryDl;
        let bin = crate::providers::gallery_dl::GalleryDl::binary()?
            .to_string_lossy()
            .into_owned();
        let mut sub_req = req.clone();
        sub_req.url = url.to_string();
        apply_sub_url_overrides(&mut sub_req, url);
        resolve_cookie_capture(&mut sub_req);
        let a = p.build_args(&sub_req)?;
        (bin, a)
    };
    if dry_run {
        args.push(std::ffi::OsString::from("--get-urls"));
        crate::process::Executor::run_inherited_checked(&binary, &args)?;
        return Ok(0);
    }

    // Count posts lost to anti-bot JS challenges (rehydration exhausted).
    let challenge_failures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    match hooks {
        Some(hooks) => {
            let on_file = &mut hooks.on_file;
            let on_log = &mut hooks.on_log;
            let counter = challenge_failures.clone();
            crate::process::Executor::run_streaming(
                &binary,
                &args,
                abort,
                // one stdout line per downloaded/skipped file (the path)
                &mut |line| on_file(line),
                &mut |line| {
                    tracing::debug!(target = "gallery-dl", "{line}");
                    if is_challenge_exhausted_line(line) {
                        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    on_log(line);
                },
            )?;
        }
        None => {
            // No UI attached — inherit stdio (raw output, CI-friendly).
            // stderr lines are not captured here, so challenge counting is
            // unavailable in this mode.
            crate::process::Executor::run_inherited_checked(&binary, &args)?;
        }
    }
    Ok(challenge_failures.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        FailureKind, ScrapeRequest, apply_sub_url_overrides, classify_failure,
        is_instagram_posts_pass, sub_url_label, validate_cookies_browser, validate_cookies_file,
        validate_extra_args, validate_output_path, validate_url,
    };
    use std::io::Write;

    /// A request carrying a real Instagram session on disk, so `ds_user_id`
    /// can actually be read the way it is in production.
    fn req_with_instagram_session() -> (ScrapeRequest, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let p = dir.path().join("session.txt");
        std::fs::write(
            &p,
            "# Netscape HTTP Cookie File\n\
             .instagram.com\tTRUE\t/\tTRUE\t1800000000\tds_user_id\t76493942756\n\
             .instagram.com\tTRUE\t/\tTRUE\t1800000000\tsessionid\tabc\n",
        )
        .expect("seed");
        let mut req = base_req();
        req.cookies_file = Some(p);
        (req, dir)
    }

    /// The regression this rewrite exists for. The note used to assert that a
    /// not-found meant the site had refused the session — but gallery-dl
    /// v1.32.16 raised that same `NotFoundError` from its own user lookup
    /// without asking Instagram at all. The note pointed at the session, the
    /// bug was in the pinned backend, and a day of debugging went with it.
    #[test]
    fn not_found_never_blames_the_session() {
        let (req, _dir) = req_with_instagram_session();
        let hint = super::session_hint(
            &req,
            "https://www.instagram.com/stories/demo_account/",
            false,
        )
        .expect("a known session always yields a hint");
        assert!(!hint.contains("refused that session"), "{hint}");
        assert!(!hint.contains("not that the account is missing"), "{hint}");
        assert!(hint.contains("76493942756"), "{hint}");
        assert!(hint.contains("stories"), "{hint}");
    }

    /// When an earlier URL in the same run already worked on this session, the
    /// site is demonstrably accepting it. That is the one fact that can clear
    /// the session, and it is what pointed at the backend in this case.
    #[test]
    fn an_earlier_success_clears_the_session() {
        let (req, _dir) = req_with_instagram_session();
        let hint = super::session_hint(&req, "https://www.instagram.com/demo_account/", true)
            .expect("hint");
        assert!(
            hint.contains("an earlier URL in this run succeeded"),
            "{hint}"
        );
        assert!(hint.contains("not the session"), "{hint}");
        assert!(
            !hint.contains("next: retry this URL on its own"),
            "with the session already cleared, 'try another session' is wrong: {hint}"
        );
    }

    #[test]
    fn the_first_url_of_a_run_makes_no_such_claim() {
        let (req, _dir) = req_with_instagram_session();
        let hint = super::session_hint(&req, "https://www.instagram.com/demo_account/", false)
            .expect("hint");
        assert!(!hint.contains("an earlier URL in this run"), "{hint}");
        assert!(hint.contains("next: retry this URL on its own"), "{hint}");
    }

    /// Every run names the account it is about to send. Switching profiles in
    /// the browser was invisible until v1.11.2, and the only place the account
    /// was ever named was inside this error.
    #[test]
    fn the_banner_names_the_account_before_anything_runs() {
        let (req, _dir) = req_with_instagram_session();
        let banner = super::session_banner(&req);
        assert!(banner.contains("76493942756"), "{banner}");
    }

    #[test]
    fn an_anonymous_run_says_so() {
        let req = base_req();
        let hint = super::session_hint(&req, "https://www.instagram.com/demo_account/", false)
            .expect("an anonymous run gets a note too");
        assert!(hint.contains("no session was sent"), "{hint}");
        assert!(super::session_banner(&req).contains("none"));
    }

    /// The pin and the installed binary drift apart silently, because only
    /// `scrapmf setup` reinstalls. That drift is precisely why a bug already
    /// fixed upstream kept reproducing here.
    #[test]
    fn backend_drift_is_never_silent_in_the_hint() {
        let (req, _dir) = req_with_instagram_session();
        let hint = super::session_hint(&req, "https://www.instagram.com/demo_account/", false)
            .expect("hint");
        assert_eq!(
            crate::application::backend::pin_drift_notice().is_some(),
            hint.contains("scrapmf setup"),
            "the hint and the drift helper must agree; hint was: {hint}"
        );
    }

    #[test]
    fn sub_url_labels_by_path() {
        assert_eq!(
            sub_url_label("https://www.instagram.com/stories/example_user/"),
            "stories"
        );
        assert_eq!(
            sub_url_label("https://www.instagram.com/example_user/highlights/"),
            "highlights"
        );
        assert_eq!(
            sub_url_label("https://www.instagram.com/reels/example_user/"),
            "reels"
        );
        // profile root pass downloads the feed
        assert_eq!(
            sub_url_label("https://www.instagram.com/example_user/"),
            "posts"
        );
        // `/USER/info/` must not be mistaken for the posts pass, or the
        // dashboard would report a media step that writes no media.
        assert_eq!(
            sub_url_label("https://www.instagram.com/example_user/info/"),
            "Instagram_id"
        );
        assert_eq!(
            sub_url_label("https://www.threads.com/@example_user"),
            "posts"
        );
    }

    /// TikTok private-but-followed profiles: the story list extractor dies
    /// fetching webapp.user-detail (statusCode 10222) even though the session
    /// is valid — direct /video/<id> URLs work. Must surface as an auth skip,
    /// not a hard failure.
    #[test]
    fn classifies_tiktok_private_profile_login_wall_as_auth() {
        let msg = "scrape failed: https://www.tiktok.com/@demo_user/stories: \
    backend '/home/tester/.local/share/scrapmf/bin/gallery-dl.bin' failed with exit code Some(4)\n  \
stderr: [tiktok][error] https://www.tiktok.com/@demo_user: Login required to access this \
profile, or this profile has no videos posted";
        assert!(matches!(classify_failure(msg), FailureKind::AuthRequired));
    }

    #[test]
    fn instagram_posts_pass_matches_profile_root() {
        assert!(is_instagram_posts_pass("https://www.instagram.com/user/"));
        assert!(is_instagram_posts_pass("https://instagram.com/user/"));
        assert!(is_instagram_posts_pass("https://www.instagram.com/user"));
        assert!(is_instagram_posts_pass("http://www.instagram.com/user/"));
        // query string still identifies the profile-root pass
        assert!(is_instagram_posts_pass(
            "https://www.instagram.com/user/?hl=en"
        ));
    }

    #[test]
    fn instagram_posts_pass_rejects_subpages_and_other_sites() {
        // dedicated passes must NOT get the posts filter
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/user/reels/"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/user/highlights/"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/user/avatar"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/stories/user/"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/p/Cxyz123/"
        ));
        // other sites / schemes
        assert!(!is_instagram_posts_pass(
            "https://www.tiktok.com/@user/posts"
        ));
        assert!(!is_instagram_posts_pass("ftp://instagram.com/user/"));
        assert!(!is_instagram_posts_pass("not a url"));
    }

    fn base_req() -> ScrapeRequest {
        ScrapeRequest {
            no_archive: false,
            profile_pic_only: false,
            url: String::new(),
            output: None,
            preset: Some("instagram".to_string()),
            extra_args: vec![],
            cookies_from_browser: None,
            cookies_file: None,
            archive: None,
            rate_limit: None,
            extractor_options: Default::default(),
            filename_template: None,
            directory_template: None,
            extra_urls: vec![],
            profile_name: None,
            extra_extractor_opts: vec![],
        }
    }

    #[test]
    fn sub_url_overrides_inject_dedup_filter_only_for_posts_pass() {
        let mut req = base_req();

        apply_sub_url_overrides(&mut req, "https://www.instagram.com/user/");
        assert_eq!(
            req.extra_extractor_opts,
            vec![(
                "extractor.instagram.posts.post-filter".to_string(),
                "type != 'reel'".to_string()
            )],
            "posts pass must carry the dedup post-filter"
        );

        for url in [
            "https://www.instagram.com/user/reels/",
            "https://www.instagram.com/user/highlights/",
            "https://www.instagram.com/stories/user/",
        ] {
            let mut r = base_req();
            apply_sub_url_overrides(&mut r, url);
            assert!(
                r.extra_extractor_opts.is_empty(),
                "{url} must not get the posts dedup filter"
            );
        }
    }

    #[test]
    fn dedup_filter_lands_in_built_args() {
        use crate::providers::Provider;
        use crate::providers::gallery_dl::GalleryDl;

        let mut req = base_req();
        req.url = "https://www.instagram.com/user/".to_string();
        let url = req.url.clone();
        apply_sub_url_overrides(&mut req, &url);

        let args = GalleryDl.build_args(&req).unwrap();
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            strs.contains(&"-o".to_string()),
            "-o flag missing in {strs:?}"
        );
        assert!(
            strs.iter()
                .any(|a| a == "extractor.instagram.posts.post-filter=type != 'reel'"),
            "scoped post-filter missing in {strs:?}"
        );
    }

    #[test]
    fn validate_url_accepts_https() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("https://example.com/gallery/123").is_ok());
        assert!(validate_url("  https://example.com  ").is_ok());
    }

    #[test]
    fn validate_url_accepts_http() {
        assert!(validate_url("http://example.com").is_ok());
    }

    #[test]
    fn validate_url_rejects_javascript() {
        assert!(validate_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn validate_url_rejects_file_scheme() {
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn validate_url_rejects_empty() {
        assert!(validate_url("").is_err());
        assert!(validate_url("   ").is_err());
    }

    #[test]
    fn validate_url_rejects_too_long() {
        let long = format!("https://example.com/{}", "a".repeat(2048));
        assert!(validate_url(&long).is_err());
    }

    #[test]
    fn validate_url_rejects_invalid() {
        assert!(validate_url("not a url").is_err());
    }

    #[test]
    fn validate_extra_args_allows_valid() {
        assert!(validate_extra_args(&["--proxy".to_string(), "http://a.com".to_string()]).is_ok());
        assert!(validate_extra_args(&["--sleep".to_string(), "1".to_string()]).is_ok());
        assert!(validate_extra_args(&["--cookies".to_string()]).is_ok());
        // Download robustness flags (large videos, flaky CDNs)
        assert!(
            validate_extra_args(&["--retries".to_string(), "10".to_string()]).is_ok(),
            "--retries must be allowed"
        );
        assert!(
            validate_extra_args(&["--http-timeout".to_string(), "120".to_string()]).is_ok(),
            "--http-timeout must be allowed"
        );
        assert!(validate_extra_args(&["--force-ipv4".to_string()]).is_ok());
        assert!(validate_extra_args(&[]).is_ok());
    }

    #[test]
    fn validate_extra_args_rejects_exec() {
        assert!(validate_extra_args(&["--exec".to_string()]).is_err());
        assert!(validate_extra_args(&["--exec=rm -rf".to_string()]).is_err());
    }

    #[test]
    fn validate_extra_args_rejects_forbidden_flag() {
        assert!(validate_extra_args(&["--unknown-flag".to_string()]).is_err());
        assert!(validate_extra_args(&["--proxy; rm".to_string()]).is_err());
    }

    #[test]
    fn validate_extra_args_rejects_forbidden_chars() {
        assert!(validate_extra_args(&["--proxy; rm -rf".to_string()]).is_err());
        assert!(validate_extra_args(&["--user-agent | cat".to_string()]).is_err());
    }

    #[test]
    fn validate_cookies_browser_accepts() {
        assert!(validate_cookies_browser("firefox").is_ok());
        assert!(validate_cookies_browser("firefox:profile").is_ok());
        assert!(validate_cookies_browser("brave").is_ok());
        assert!(validate_cookies_browser("chrome:Default").is_ok());
    }

    #[test]
    fn validate_cookies_browser_rejects() {
        assert!(validate_cookies_browser("invalidbrowser").is_err());
        assert!(validate_cookies_browser("firefox; rm").is_err());
    }

    /// Every channel scrapmf can read must be nameable in a request.
    ///
    /// The allowlist used to be a separate hand-written array, so the Brave
    /// variants were readable but unreachable: a user whose Instagram session
    /// lived in Brave Origin had no way to ask for it and silently got
    /// Brave-Browser instead.
    #[test]
    fn validate_cookies_browser_accepts_every_readable_channel() {
        for ch in crate::browsers::CHANNELS {
            assert!(
                validate_cookies_browser(ch.id).is_ok(),
                "channel '{}' can be read but cannot be requested",
                ch.id
            );
        }
    }

    /// A channel scrapmf cannot resolve must stay rejected, otherwise the
    /// derived allowlist would drift the other way and accept a browser that
    /// silently yields no cookies.
    #[test]
    fn validate_cookies_browser_rejects_unknown_channels() {
        assert!(validate_cookies_browser("definitely-not-a-browser").is_err());
        assert!(validate_cookies_browser("").is_err());
    }

    // ─── Rate limiting (HTTP 429) ───────────────────────────────────────────

    #[test]
    fn rate_limit_is_recognised_across_gallery_dl_phrasings() {
        for msg in [
            "HttpError: '429 Too Many Requests' for 'https://www.instagram.com/api/v1/feed/'",
            "[instagram][error] HTTP Error 429",
            "HTTP error 429: Too Many Requests",
            "Error 429",
        ] {
            assert!(super::is_rate_limited(msg), "missed a 429 phrasing: {msg}");
        }
    }

    /// A signed CDN URL carries a hex `oe=` / `_nc_` parameter that can hold
    /// the digits 429. Matching the bare number would classify a perfectly
    /// healthy download as throttled and send the run into a minutes-long
    /// retry loop over a failure that needs no waiting at all.
    #[test]
    fn digits_inside_a_cdn_url_are_not_a_rate_limit() {
        let msg = "[downloader.http][warning] '403 Forbidden' for \
'https://instagram.fna.fbcdn.net/v/t51.1/763379249_n.jpg?efg=eyJm&oe=6A916429\
&_nc_sid=4291d3b'";
        assert!(
            !super::is_rate_limited(msg),
            "a 429 inside a CDN URL must not read as throttling"
        );
    }

    /// The whole point: gallery-dl reports a throttled request with the same
    /// "posts are private" line it uses for a genuinely private account.
    /// Telling the user to check their follow when the site asked them to slow
    /// down sends them to the wrong remedy, and retrying nothing loses the run.
    #[test]
    fn a_throttled_private_warning_is_not_classified_as_auth() {
        let msg = "backend failed with exit code Some(4)\n  stderr: \
[instagram][warning] demo_account's posts are private\n\
[instagram][error] HttpError: '429 Too Many Requests' for \
'https://www.instagram.com/api/v1/feed/user/45710848586/'";
        assert_eq!(
            super::classify_failure(msg),
            super::FailureKind::RateLimited,
            "a 429 must win over the private-account wording it drags along"
        );
    }

    /// Without the 429 the same warning means what it says. The TikTok and
    /// Instagram private-profile paths depend on this and must not regress
    /// into "wait and retry".
    #[test]
    fn private_warning_alone_stays_auth_required() {
        let msg = "[instagram][warning] someone's posts are private";
        assert_eq!(
            super::classify_failure(msg),
            super::FailureKind::AuthRequired
        );
        assert!(super::is_private_warning(msg));
    }

    #[test]
    fn rate_limit_backoff_grows_and_is_bounded() {
        let waits: Vec<u64> = (0..5).map(super::rate_limit_backoff_secs).collect();
        assert_eq!(waits, vec![60, 120, 240, 480, 900]);
        assert!(
            waits.windows(2).all(|w| w[0] < w[1]),
            "backoff must increase: {waits:?}"
        );
        assert!(
            waits.iter().all(|w| *w <= 15 * 60),
            "no single wait may exceed the cap: {waits:?}"
        );
    }

    /// A request that already carries an explicit cookie file must not be
    /// rewritten: the file is the more specific instruction.
    #[test]
    fn capture_never_overrides_an_explicit_cookie_file() {
        let mut req = base_req();
        req.cookies_file = Some(std::path::PathBuf::from("/tmp/explicit.txt"));
        req.cookies_from_browser = Some("brave".to_string());
        super::resolve_cookie_capture(&mut req);
        assert_eq!(
            req.cookies_file.as_deref(),
            Some(std::path::Path::new("/tmp/explicit.txt")),
            "an explicit cookie file must win over any browser capture"
        );
    }

    /// The regression this exists for: capture is wired in where argv is built,
    /// which is the single point both the interactive flow and `scrapmf scrape`
    /// pass through. A wrong site must leave the browser flag intact so the
    /// original behaviour survives.
    #[test]
    fn capture_leaves_browser_flag_when_the_site_is_unknown() {
        let mut req = base_req();
        req.url = "https://example.invalid/some/path".to_string();
        req.cookies_from_browser = Some("brave".to_string());
        super::resolve_cookie_capture(&mut req);
        assert_eq!(
            req.cookies_from_browser.as_deref(),
            Some("brave"),
            "an unresolvable site must keep --cookies-from-browser"
        );
        assert!(
            req.cookies_file.is_none(),
            "no site means no capture; a file must not appear out of nowhere"
        );
    }

    /// A `browser:profile` suffix is meaningful only to the browser's own reader.
    /// scrapmf captures the default profile, so honouring the suffix here would
    /// silently authenticate as the wrong profile — leave those requests alone.
    #[test]
    fn capture_skips_requests_naming_an_explicit_browser_profile() {
        let mut req = base_req();
        req.url = "https://www.instagram.com/user/".to_string();
        req.cookies_from_browser = Some("brave:Profile 2".to_string());
        super::resolve_cookie_capture(&mut req);
        assert_eq!(
            req.cookies_from_browser.as_deref(),
            Some("brave:Profile 2"),
            "a profile-qualified browser must not be captured from the default profile"
        );
        assert!(req.cookies_file.is_none());
    }

    /// Whatever the capture decides, argv must still carry a session — either the
    /// captured file or the browser flag. This is the invariant that keeps the
    /// change strictly an improvement.
    #[test]
    fn capture_never_leaves_the_request_without_a_session() {
        let mut req = base_req();
        req.url = "https://www.instagram.com/user/".to_string();
        req.cookies_from_browser = Some("definitely-not-a-browser".to_string());
        super::resolve_cookie_capture(&mut req);
        assert!(
            req.cookies_file.is_some() || req.cookies_from_browser.is_some(),
            "capture must never strip the session from the request"
        );
        if let Some(p) = req.cookies_file.as_ref() {
            assert!(p.is_file(), "a captured profile must exist on disk: {p:?}");
        }
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn validate_cookies_file_accepts() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let _ = writeln!(tmp, "# Netscape HTTP Cookie File");
        assert!(validate_cookies_file(tmp.path()).is_ok());
    }

    #[test]
    fn validate_cookies_file_rejects_missing() {
        assert!(
            validate_cookies_file(std::path::Path::new("/tmp/fake_cookies_12345.txt")).is_err()
        );
    }

    #[test]
    fn failure_classification() {
        use super::{FailureKind, classify_failure};
        // AuthRequired: real gallery-dl signatures
        assert_eq!(
            classify_failure(
                "scrape failed: backend 'gallery-dl' failed ... AuthRequired exception"
            ),
            FailureKind::AuthRequired
        );
        assert_eq!(
            classify_failure("HTTP Error 401 Unauthorized"),
            FailureKind::AuthRequired
        );
        assert_eq!(
            classify_failure("403 Forbidden for url https://x.com/i/1"),
            FailureKind::AuthRequired
        );
        assert_eq!(
            classify_failure("login required to view this profile"),
            FailureKind::AuthRequired
        );
        // Unsupported
        assert_eq!(
            classify_failure("'https://example.com/x': Unsupported URL"),
            FailureKind::Unsupported
        );
        assert_eq!(
            classify_failure("no suitable extractor found"),
            FailureKind::Unsupported
        );
        // Unknown — including the old false positives:
        // a bare "403" inside unrelated text must NOT be auth...
        assert_ne!(
            classify_failure("connection to port 403 failed"),
            FailureKind::AuthRequired
        );
        // ...and 'cookies' mentions in generic network errors are hard failures now.
        assert_eq!(
            classify_failure("error reading cookies jar file: no such file"),
            FailureKind::Unknown
        );
        assert_eq!(
            classify_failure("Connection reset by peer"),
            FailureKind::Unknown
        );
        assert_eq!(classify_failure(""), FailureKind::Unknown);
    }

    #[test]
    fn challenge_exhausted_detection() {
        // Exhausted attempts → true
        assert!(super::is_challenge_exhausted_line(
            "[tiktok][warning] @user: Failed to retrieve rehydration data (10/10)"
        ));
        assert!(super::is_challenge_exhausted_line(
            "123: Failed to retrieve rehydration data (4/4)"
        ));
        // Still retrying → false
        assert!(!super::is_challenge_exhausted_line(
            "@user: Failed to retrieve rehydration data (1/10)"
        ));
        assert!(!super::is_challenge_exhausted_line(
            "@user: Failed to retrieve rehydration data (3/4)"
        ));
        // Unrelated lines → false
        assert!(!super::is_challenge_exhausted_line(
            "Failed to solve JavaScript challenge"
        ));
        assert!(!super::is_challenge_exhausted_line(
            "[download] file.mp4 done"
        ));
        assert!(!super::is_challenge_exhausted_line(
            "Failed to retrieve rehydration data (0/0)"
        ));
    }

    #[test]
    fn validate_output_path_rejects_forbidden() {
        assert!(validate_output_path(std::path::Path::new("/")).is_err());
        assert!(validate_output_path(std::path::Path::new("/etc")).is_err());
        assert!(validate_output_path(std::path::Path::new("../out")).is_err());
    }

    #[test]
    fn validate_output_path_protects_system_trees_not_just_exact_matches() {
        // Anything INSIDE a protected tree is rejected too
        for p in [
            "/etc/passwd_backup",
            "/var/lib/scrapmf",
            "/usr/local/share/out",
            "/boot/vmlinuz.d",
            "/root/downloads",
            "/proc/self/out",
            "/sys/x",
            "/dev/null-dir",
            "/run/media",
            "/bin/sh-out",
            "/lib64/x",
        ] {
            assert!(
                validate_output_path(std::path::Path::new(p)).is_err(),
                "{p} must be rejected"
            );
        }
    }

    #[test]
    fn validate_output_path_allows_legitimate_locations() {
        for p in [
            "/tmp/scrapmf-out",
            "/opt/data/archives",
            "/home/someone/pics",
            "./downloads",
            "~/Pictures/scrapmf",
            "relative/out",
        ] {
            assert!(
                validate_output_path(std::path::Path::new(p)).is_ok(),
                "{p} must be allowed"
            );
        }
    }
}
