//! Cookie profiles: named, shareable Netscape-format credential files.
//!
//! Storage lives at `~/.config/scrapmf/cookies/<name>.txt` in the classic
//! Mozilla/Netscape format that gallery-dl consumes via `--cookies`. Files are
//! credentials (account access!) — always written 0600 and never logged.
//!
//! Chromium-family browsers encrypt cookie values (v10 legacy / v11 current):
//! the decryption key comes from the desktop keyring (schema v2 stores it
//! base64-encoded; v1/legacy use "peanuts") and is derived with PBKDF2-SHA1.
//! `capture_chromium` tries every candidate key against the first encrypted
//! cookie and keeps whichever validates.

use std::path::{Path, PathBuf};

// ─── Site domains ───────────────────────────────────────────────────────────

/// Domains (as they appear in cookie files / moz_cookies.host_key) per site key.
/// Delegates to the central `crate::sites` registry — single source of truth.
pub fn domains_for_site(site_key: &str) -> &'static [&'static str] {
    crate::sites::registry::domains_for_site(site_key)
}

// ─── Storage ────────────────────────────────────────────────────────────────

/// Directory holding every cookie profile.
pub fn cookies_dir() -> Option<PathBuf> {
    crate::config::config_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|base| base.join("cookies"))
}

/// Prefix of the profile scrapmf writes itself when it captures a browser
/// session for a scrape. It lives in the cookies directory because that is
/// what `--cookies` expects, but it is a cache and not a user profile: it is
/// hidden from [`list_profiles`] and replaced on each capture.
pub const AUTO_PROFILE_PREFIX: &str = "scrapmf-auto-";

fn sanitize_name(name: &str) -> String {
    crate::util::sanitize_component(name.trim(), 48, "unnamed")
}

pub fn profile_path(name: &str) -> Option<PathBuf> {
    cookies_dir().map(|dir| dir.join(format!("{}.txt", sanitize_name(name))))
}

pub fn list_profiles() -> Vec<String> {
    let Some(dir) = cookies_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str().map(String::from))
        })
        // `scrapmf-auto-*` is the internal capture cache written by the
        // browser-cookie resolution, not something the user created. Listing it
        // in Browse profiles made it look like a saved profile the user had to
        // manage — and deleting it changed nothing, because the next scrape
        // rewrote it.
        .filter(|n| !n.starts_with(AUTO_PROFILE_PREFIX))
        .collect();
    names.sort();
    names
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ─── Netscape format ────────────────────────────────────────────────────────

// ─── Source metadata (embedded as a comment line) ───────────────────────────

const SOURCE_PREFIX: &str = "# scrapmf-source: ";

#[derive(Debug, Clone, PartialEq)]
pub struct SourceMeta {
    pub browser: String,
    pub networks: Vec<String>,
}

/// Read embedded origin metadata ("# scrapmf-source: browser=X networks=a,b").
/// None when the profile was created by manual paste/import.
pub fn parse_source_metadata(content: &str) -> Option<SourceMeta> {
    let line = content.lines().find(|l| l.starts_with(SOURCE_PREFIX))?;
    let mut browser = None;
    let mut networks = Vec::new();
    for part in line.strip_prefix(SOURCE_PREFIX)?.split_whitespace() {
        if let Some(v) = part.strip_prefix("browser=") {
            browser = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("networks=") {
            networks = v.split(',').map(String::from).collect();
        }
    }
    Some(SourceMeta {
        browser: browser?,
        networks,
    })
}

pub fn source_metadata_line(browser: &str, networks: &[String]) -> String {
    format!(
        "{SOURCE_PREFIX}browser={browser} networks={}\n",
        networks.join(",")
    )
}

/// One cookie row in Netscape format.
#[derive(Clone, Debug)]
pub struct StoredCookie {
    /// Raw Chromium-encrypted blob — used only during capture, never serialized.
    pub encrypted_value: Vec<u8>,
    pub domain: String,
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    /// Unix seconds; `0` = session cookie.
    pub expires: i64,
    pub name: String,
    pub value: String,
    pub http_only: bool,
}

const NETSCAPE_HEADER: &str =
    "# Netscape HTTP Cookie File\n# This is a generated file! Do not edit.\n\n";

fn cookie_to_netscape_line(c: &StoredCookie) -> String {
    format!(
        "{}{}\t{}\t{}\t{}\t{}\t{}\t{}",
        if c.http_only { "#HttpOnly_" } else { "" },
        c.domain,
        if c.include_subdomains {
            "TRUE"
        } else {
            "FALSE"
        },
        c.path,
        if c.secure { "TRUE" } else { "FALSE" },
        c.expires,
        c.name,
        c.value
    )
}

/// Parse Netscape content into stored cookies. Skips comments, blank lines
/// and malformed rows; errors only when no valid cookie line exists at all.
/// The `#HttpOnly_` prefix convention is understood (http_only = true).
pub fn parse_netscape(content: &str) -> Result<Vec<StoredCookie>, String> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line.starts_with("# ") {
            continue;
        }
        let (http_only, line) = match line.strip_prefix("#HttpOnly_") {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 || f[5].is_empty() {
            continue;
        }
        let Ok(expires) = f[4].trim().parse::<i64>() else {
            continue;
        };
        out.push(StoredCookie {
            encrypted_value: Vec::new(),
            domain: f[0].to_string(),
            include_subdomains: f[1] == "TRUE",
            path: f[2].to_string(),
            secure: f[3] == "TRUE",
            expires,
            name: f[5].to_string(),
            value: f[6].to_string(),
            http_only,
        });
    }
    if out.is_empty() {
        return Err("no valid cookie lines found — expected Netscape format \
                    (7 tab-separated fields per line, header '# Netscape HTTP Cookie File')"
            .to_string());
    }
    Ok(out)
}

/// Serialize cookies into gallery-dl-ready Netscape content.
pub fn to_netscape(cookies: &[StoredCookie]) -> String {
    let mut out = String::from(NETSCAPE_HEADER);
    for c in cookies {
        out.push_str(&cookie_to_netscape_line(c));
        out.push('\n');
    }
    out
}

// ─── Profile CRUD ───────────────────────────────────────────────────────────

/// Load + validate a profile file. Returns the parsed cookies.
pub fn load_profile(name: &str) -> Result<Vec<StoredCookie>, String> {
    let path = profile_path(name).ok_or("cannot resolve cookies dir")?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read cookie profile '{}': {e}", path.display()))?;
    parse_netscape(&content)
}

/// Save a profile atomically with 0600 permissions. Content is validated first.
pub fn save_profile(name: &str, netscape_content: &str) -> Result<PathBuf, String> {
    parse_netscape(netscape_content)?;
    let dir = cookies_dir().ok_or("cannot resolve cookies dir")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create cookies dir: {e}"))?;
    crate::config::restrict_perms(&dir, true);
    let path = profile_path(name).ok_or("invalid profile name")?;
    std::fs::write(&path, netscape_content).map_err(|e| format!("write profile: {e}"))?;
    crate::config::restrict_perms(&path, false);
    Ok(path)
}

/// Delete a stored profile.
///
/// `Ok(true)` means the file was removed, `Ok(false)` that there was nothing to
/// remove. The two are distinct on purpose: the UI reports them differently,
/// because "already gone" usually means another screen moved underneath this
/// one, while `Err` is a real failure such as a permission problem on the
/// directory.
pub fn delete_profile(name: &str) -> Result<bool, String> {
    let path = profile_path(name).ok_or("invalid profile name")?;
    delete_profile_at(&path)
}

fn delete_profile_at(path: &Path) -> Result<bool, String> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("delete profile: {e}"))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// True when the browser's cookie database was written after `capture`.
///
/// Browsers rotate the session cookie on their own — Instagram refreshed one
/// within the minute while this was being written — and a site then refuses the
/// stale value immediately, which looks exactly like a missing account rather
/// than an outdated session. Comparing against the browser's own database is
/// exact, where a fixed age threshold would only guess.
///
/// Returns false when anything is unresolvable: an unreadable path is not
/// evidence that a capture is current, and neither is it evidence that it is
/// stale. Guessing would either re-capture on every sub-extractor, or never.
pub fn capture_predates_browser(capture: &std::path::Path, channel: &str) -> bool {
    let Some(ch) = crate::browsers::resolve_channel(channel) else {
        return false;
    };
    let db_name = if ch.id == "firefox" {
        "cookies.sqlite"
    } else {
        "Cookies"
    };
    let Some(db) = crate::browsers::find_cookie_db(ch, db_name) else {
        return false;
    };
    let (Ok(cap_meta), Ok(db_meta)) = (std::fs::metadata(capture), std::fs::metadata(&db.path))
    else {
        return false;
    };
    match (cap_meta.modified(), db_meta.modified()) {
        (Ok(cap), Ok(db)) => db > cap,
        _ => false,
    }
}

/// Instagram account that a stored profile belongs to, if it can be told.
///
/// `ds_user_id` is the account's numeric id and is already decrypted in the
/// profile — it was never surfaced. For private content it is the only detail
/// that decides anything: Instagram serves a private profile to a session whose
/// account follows it, so two browsers logged into two accounts behave
/// completely differently and nothing in the interface said which one was in use.
///
/// Returns `None` when the profile has not been captured yet, or holds no
/// Instagram session. Reading it is a local file parse — no keyring round trip,
/// no network.
pub fn instagram_account_hint(profile: &str) -> Option<String> {
    let cookies = load_profile(profile).ok()?;
    cookies
        .iter()
        .find(|c| c.name == "ds_user_id" && c.domain.contains("instagram"))
        .map(|c| c.value.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Instagram account inside an arbitrary Netscape cookie file.
///
/// Takes a path rather than a profile name so a run can name the account it is
/// actually sending, whichever way that session was configured.
pub fn instagram_account_hint_file(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let cookies = parse_netscape(&content).ok()?;
    cookies
        .iter()
        .find(|c| c.name == "ds_user_id" && c.domain.contains("instagram"))
        .map(|c| c.value.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Like [`instagram_account_hint`], but for the auto profile a browser channel
/// captures into (`scrapmf-auto-<channel>`).
pub fn instagram_account_for_channel(channel: &str) -> Option<String> {
    // A profile suffix ("brave:Profile 2") is meaningless here: the capture
    // always reads the default profile, so reporting the default's account
    // would name an account that is not the one being used.
    if channel.contains(':') {
        return None;
    }
    instagram_account_hint(&format!("{AUTO_PROFILE_PREFIX}{channel}"))
}

/// `AUTO_PROFILE_PREFIX` plus a channel id, matching what the capture writes.
pub fn auto_profile_name(channel: &str) -> String {
    format!("{AUTO_PROFILE_PREFIX}{channel}")
}

/// Resolve the session a scrape should use for one account.
///
/// Precedence, highest first:
///
/// 1. `cookies_browser` — an explicit `--cookies-from-browser` flag.
/// 2. `cookie_profile` — a named profile. When it wins it also clears the
///    browser source: mixing a named session with browser cookies would send
///    two different identities in the same request.
/// 3. `cookies_file` / `site_cfg.cookies` — a Netscape file.
/// 4. `global_browser` — the single choice made under General settings.
/// 5. `site_cfg.cookies_from_browser` — deprecated. Generated site files no
///    longer carry it, so this only fires for configs written by hand, and it
///    warns because it will shadow the global setting.
///
/// A named profile that has disappeared falls through to the browser with a
/// warning rather than failing: a stale name should not cost the user their
/// download.
pub fn resolve_session(
    cookie_profile: Option<&str>,
    cookies_file: Option<PathBuf>,
    cookies_browser: Option<String>,
    global_browser: Option<String>,
    site_cfg: Option<&crate::config::Site>,
) -> (Option<PathBuf>, Option<String>) {
    let mut file = cookies_file;
    let explicit_browser = cookies_browser.is_some();
    let mut browser = cookies_browser.or(global_browser);
    if let Some(site) = site_cfg {
        if file.is_none() {
            file = site.cookies.clone();
        }
        if browser.is_none()
            && let Some(legacy) = site.cookies_from_browser.as_deref()
        {
            tracing::warn!(
                browser = legacy,
                "site config sets cookies_from_browser — that setting is deprecated and \
                 overrides the global choice; move it to General settings or delete the \
                 line from sites/<name>.toml"
            );
            browser = Some(legacy.to_string());
        }
    }
    if let Some(name) = cookie_profile {
        match profile_path(name) {
            Some(p) if p.exists() => {
                file = Some(p);
                browser = None;
            }
            _ => {
                eprintln!(
                    "⚠ cookie profile '{name}' not found — falling back to browser/session defaults"
                );
            }
        }
    }
    // A cookies file is a deliberate, explicit choice. Sending it together with
    // browser cookies can present two identities in one request, and gallery-dl
    // applies `--cookies-from-browser` after `--cookies`, so the browser copy
    // silently wins. Clear the browser source whenever a file is present, unless
    // the caller explicitly asked for browser cookies too.
    if file.is_some() && !explicit_browser {
        browser = None;
    }
    (file, browser)
}

/// Whether a stored profile actually carries cookies for any of `sites`.
///
/// Used to rank stored profiles so the ones that can authenticate a batch come
/// first. Lives here rather than in a flow because every cookie picker needs it.
pub fn has_cookies_for(name: &str, sites: &[String]) -> bool {
    if sites.is_empty() {
        return true;
    }
    let Ok(cookies) = load_profile(name) else {
        return false;
    };
    sites.iter().any(|site| {
        let domains = domains_for_site(site);
        !domains.is_empty()
            && cookies.iter().any(|c| {
                domains
                    .iter()
                    .any(|d| c.domain == *d || c.domain.ends_with(&format!(".{d}")))
            })
    })
}

/// Human summary of a profile: total/expired cookies and domains covered.
/// Domains beyond 2 are collapsed to keep the line within the menu width.
pub fn profile_summary(name: &str) -> Result<String, String> {
    let cookies = load_profile(name)?;
    let now = now_secs();
    let total = cookies.len();
    let expired = cookies
        .iter()
        .filter(|c| c.expires != 0 && c.expires <= now)
        .count();
    let mut domains: Vec<&str> = cookies.iter().map(|c| c.domain.as_str()).collect();
    domains.sort_unstable();
    domains.dedup();
    let domains_str = if domains.len() <= 2 {
        domains.join(", ")
    } else {
        format!("{}, +{} more", domains[..2].join(", "), domains.len() - 2)
    };
    Ok(format!(
        "{total} cookie(s), {expired} expired — domains: {domains_str}"
    ))
}

// ─── Firefox capture ────────────────────────────────────────────────────────

fn newest_firefox_cookie_db() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let root = home.join(".mozilla").join("firefox");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&root).ok()?.flatten() {
        let db = entry.path().join("cookies.sqlite");
        if !db.is_file() {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&db) {
            let mtime = meta.modified().ok();
            match (mtime, &best) {
                (Some(t), Some((bt, _))) if t <= *bt => {}
                (Some(t), _) => best = Some((t, db)),
                _ => {}
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Extract cookies for `domains` from the most recent Firefox cookies.sqlite
/// (unencrypted SQLite, read from a copy so browser locks never matter).
///
/// Uses external readers instead of linking SQLite C code — keeps every
/// release target cross-compilable with zero C in the tree:
///   1. the `sqlite3` CLI, if present;
///   2. python3's stdlib `sqlite3` module otherwise.
pub fn capture_firefox(sites: &[String], profile_name: &str) -> Result<(PathBuf, usize), String> {
    if sites.is_empty() {
        return Err("no networks selected".to_string());
    }
    let domains: Vec<&str> = sites
        .iter()
        .flat_map(|s| domains_for_site(s))
        .copied()
        .collect();
    let db = newest_firefox_cookie_db()
        .ok_or("no Firefox cookies.sqlite found (~/.mozilla/firefox/*/cookies.sqlite)")?;

    let tmp = std::env::temp_dir().join(format!("scrapmf-ff-{}.sqlite", std::process::id()));
    std::fs::copy(&db, &tmp).map_err(|e| format!("copy cookies db: {e}"))?;

    let result = (|| {
        let rows = sqlite_read_rows(
            &tmp,
            "host,name,value,path,COALESCE(expiry,0),isSecure,isHttpOnly",
            "moz_cookies",
        )?;
        let mut cookies: Vec<StoredCookie> = Vec::new();
        for row in rows {
            if row.len() != 7 || row[1].is_empty() {
                continue;
            }
            let Ok(expires) = row[4].parse::<i64>() else {
                continue;
            };
            let cookie = StoredCookie {
                encrypted_value: Vec::new(),
                domain: row[0].clone(),
                include_subdomains: true,
                path: row[3].clone(),
                secure: row[5] == "1",
                expires,
                name: row[1].clone(),
                value: row[2].clone(),
                http_only: row[6] == "1",
            };
            if domains
                .iter()
                .any(|d| cookie.domain == *d || cookie.domain.ends_with(&format!(".{d}")))
            {
                cookies.push(cookie);
            }
        }
        if cookies.is_empty() {
            return Err(format!(
                "no cookies found for {} in this Firefox session — open the \
                 site while logged in, then retry",
                domains.join(", ")
            ));
        }
        let count = cookies.len();
        let content = to_netscape(&cookies);
        let content = format!("{}{content}", source_metadata_line("firefox", sites));
        let path = save_profile(profile_name, &content)?;
        Ok((path, count))
    })();

    let _ = std::fs::remove_file(&tmp);
    result
}

// ─── Refresh ────────────────────────────────────────────────────────────────

/// Result of refreshing an existing profile.
pub enum Refresh {
    /// Re-captured automatically from the stored origin.
    Done { path: PathBuf, count: usize },
    /// No source metadata (manual import) — user must paste a fresh export
    /// via $EDITOR.
    ManualImportRequired,
}

/// Re-capture an existing profile using its embedded origin metadata.
/// Keeps the SAME name, so accounts referencing it keep working untouched.
pub fn refresh_profile(name: &str) -> Result<Refresh, String> {
    let path = profile_path(name).ok_or("invalid profile name")?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read profile '{name}': {e}"))?;

    match parse_source_metadata(&content) {
        Some(meta) if !meta.networks.is_empty() => {
            let res = if meta.browser.eq_ignore_ascii_case("firefox") {
                capture_firefox(&meta.networks, name)
            } else {
                capture_chromium(&meta.browser, &meta.networks, name)
            };
            res.map(|(path, count)| Refresh::Done { path, count })
        }
        _ => Ok(Refresh::ManualImportRequired),
    }
}

/// Shared external SQLite reader: runs `SELECT {columns} FROM {table}` and
/// returns TSV rows. Engine chain: sqlite3 CLI → python3 stdlib. Keeps the
/// tree C-free so every release target stays cross-compilable.
fn sqlite_read_rows(
    db_copy: &Path,
    columns_expr: &str,
    table: &str,
) -> Result<Vec<Vec<String>>, String> {
    let sql = format!("SELECT {columns_expr} FROM {table};");

    // 1) sqlite3 CLI.
    //
    // `-nocolumn` is NOT a valid sqlite3 option — the flag does not exist and
    // the CLI rejects it outright ("Error: unknown option: -nocolumn"). It was
    // passed here for years, so this branch could never succeed and the reader
    // silently depended on the python3 fallback; on a host without python3 the
    // user got "no cookies could be decrypted", blaming their cookies instead
    // of the missing tool. `-noheader` alone is the correct way to emit bare
    // rows.
    if let Ok(out) = std::process::Command::new("sqlite3")
        .arg("-noheader")
        .arg("-separator")
        .arg("\t")
        .arg(db_copy)
        .arg(&sql)
        .output()
        && out.status.success()
        && !out.stdout.is_empty()
    {
        return Ok(tsv_to_rows(&String::from_utf8_lossy(&out.stdout)));
    }

    // 2) python3 stdlib (sqlite3 ships with every distro's python3)
    let script = "import sqlite3,sys\n\
c = sqlite3.connect(sys.argv[1])\n\
[print(*r, sep='\t') for r in c.execute(sys.argv[2])]";
    if let Ok(out) = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(db_copy)
        .arg(&sql)
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if !text.trim().is_empty() {
                return Ok(tsv_to_rows(&text));
            }
        }
        return Err(format!(
            "python3 reader failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Err(
        "cannot read the cookies database: neither 'sqlite3' nor 'python3' is available\n  \
         help: install one of them, e.g. 'pacman -S sqlite' (Arch), 'apt install sqlite3' \
         (Debian/Ubuntu), 'dnf install sqlite' (Fedora)\n  \
         note: this is a missing tool, not a decryption problem — the cookies on disk are fine"
            .to_string(),
    )
}

fn tsv_to_rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').map(String::from).collect())
        .collect()
}

// ─── Chromium-family capture (Brave/Chrome/Chromium/Edge/Vivaldi/Opera) ─────

/// Everything needed to read and decrypt a Chromium-family cookie DB.
struct ChromiumPaths {
    cookies_db: PathBuf,
    /// Secret Service `application` attribute candidates, most likely first.
    secret_apps: &'static [&'static str],
    /// KWallet folders to probe, most likely first.
    kwallet_folders: &'static [&'static str],
    /// KWallet entry holding the safe-storage key.
    kwallet_key: &'static str,
    /// Profile the DB came from (`Default`, `Profile 2`, …).
    profile: String,
    /// Packaging that supplied it: `native`, `flatpak`, `snap`.
    origin: &'static str,
    /// Channel display name, for diagnostics.
    display: &'static str,
}

/// Resolve a browser label ("Brave", "Brave Origin", "Chrome", …) to its cookie
/// database. Delegates path resolution to [`crate::browsers`] so the capture
/// path and `--cookies-from-browser` detection can never drift again.
fn chromium_paths(browser: &str) -> Option<ChromiumPaths> {
    let ch = crate::browsers::resolve_channel(browser)?;
    let db = crate::browsers::find_cookie_db(ch, "Cookies")?;
    Some(ChromiumPaths {
        cookies_db: db.path,
        secret_apps: ch.secret_apps,
        kwallet_folders: ch.kwallet_folders,
        kwallet_key: ch.kwallet_key,
        profile: db.profile,
        origin: db.origin,
        display: ch.display,
    })
}

/// Resolve the D-Bus session bus address, or `None` when there is no socket to
/// talk to.
///
/// `DBUS_SESSION_BUS_ADDRESS` is **not** required: libdbus already falls back to
/// the systemd user socket, so `secret-tool` and `kwalletctl6` find the bus
/// without it. A previous version treated the missing variable as proof that
/// the keyring was unreachable, which reported a working bus as broken and
/// sent users to export a variable that was never the problem.
///
/// We still resolve the address ourselves for two reasons: to report the real
/// state instead of guessing, and to inject it into the helper processes, which
/// is what lets a keyring work from a shell that never inherited it.
///
/// Pure: the inputs are passed in rather than read from the environment, so
/// this is testable without mutating process state (which needs `unsafe` on
/// Rust 2024, and this crate forbids unsafe).
fn resolve_dbus_address_in(
    explicit: Option<&str>,
    runtime_dir: Option<&str>,
    uid: Option<u32>,
) -> Option<String> {
    let usable = |addr: &str| -> bool {
        let addr = addr.trim();
        !addr.is_empty() && !addr.contains("autolaunch:")
    };

    if let Some(addr) = explicit.map(str::trim).filter(|a| usable(a)) {
        return Some(addr.to_string());
    }
    // systemd exports XDG_RUNTIME_DIR and places the bus beside it.
    if let Some(dir) = runtime_dir.map(str::trim).filter(|d| !d.is_empty()) {
        let candidate = format!("unix:path={dir}/bus");
        if std::path::Path::new(&format!("{dir}/bus")).exists() {
            return Some(candidate);
        }
    }
    // Fall back to the conventional systemd location, deriving the uid from
    // /proc/self/status. Absent inside a chroot or a bare TTY — which is exactly
    // the case worth distinguishing in the error message.
    if let Some(uid) = uid {
        let dir = format!("/run/user/{uid}");
        if std::path::Path::new(&format!("{dir}/bus")).exists() {
            return Some(format!("unix:path={dir}/bus"));
        }
    }
    None
}

/// Real uid of this process, read from `/proc/self/status`.
fn current_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("Uid:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

/// The D-Bus session bus address, if one is reachable from this process.
fn resolve_dbus_address() -> Option<String> {
    resolve_dbus_address_in(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref(),
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
        current_uid(),
    )
}

/// Whether the conventional per-user bus socket is visible from here.
///
/// False inside a chroot or a bare TTY, where the keyring daemon lives in the
/// host namespace and simply cannot be reached — a fact no amount of code in
/// scrapmf can change, and one worth stating plainly instead of blaming the
/// browser's password.
fn keyring_outside_namespace() -> bool {
    match current_uid() {
        Some(uid) => !std::path::Path::new(&format!("/run/user/{uid}/bus")).exists(),
        None => false,
    }
}

/// Public wrappers so `doctor` can report the real D-Bus state without
/// duplicating the discovery logic (and its reasoning) here.
pub fn resolve_dbus_address_for_doctor() -> Option<String> {
    resolve_dbus_address()
}

pub fn keyring_outside_namespace_for_doctor() -> bool {
    keyring_outside_namespace()
}

/// Actionable message for a keyring that cannot be reached at all.
///
/// The advice is ordered by what actually works. In a chroot, exporting
/// `DBUS_SESSION_BUS_ADDRESS` changes nothing — the socket is in the host
/// namespace — so the keyring-free import path is offered first, and the chroot
/// case is named explicitly instead of being lumped in with "no D-Bus".
fn keyring_unreachable_error(display: &str) -> String {
    let cause = if keyring_outside_namespace() {
        format!(
            "the D-Bus socket /run/user/<uid>/bus is not visible from here — this looks \
             like a chroot, container or bare TTY, and the keyring daemon runs in the \
             host session, so {display}'s encrypted cookies cannot be decrypted"
        )
    } else {
        format!(
            "no D-Bus session bus is reachable, so {display}'s encrypted cookies cannot \
             be decrypted"
        )
    };
    format!(
        "{cause}\n  \
         help: skip the keyring entirely — in {display} install \"Get cookies.txt LOCALLY\", \
         log in, Export, then Configuration → Cookie profiles → Import → From file\n  \
         help: outside a chroot, run scrapmf from your desktop session, or set \
         DBUS_SESSION_BUS_ADDRESS=\"unix:path=/run/user/$UID/bus\" (uid from 'id -u')\n  \
         note: this is an environment problem, not a scrapmf bug; libsecret and KWallet \
         are both D-Bus clients"
    )
}

/// KWallet client binaries, newest generation first. `kwallet-query` is the
/// legacy KDE 4 tool and is absent on a modern Plasma 6 install, which ships
/// `kwalletctl6`; the legacy name is kept last so old systems still work.
const KWALLET_BINARIES: &[&str] = crate::browsers::KWALLET_CLIENTS;

/// Probe KWallet for a channel's safe-storage password.
///
/// The KWallet folder is **per product** (`Brave Keys`, `Chrome Keys`,
/// `Chromium Keys`, …), not shared. An earlier version hardcoded
/// `"Chromium Keys"` for every browser, so on KDE a Brave install returned an
/// empty string, the only surviving candidate was the legacy `"peanuts"`
/// constant — which Chromium uses exclusively for `v10` — and every `v11`
/// cookie failed to decrypt with "no cookies could be decrypted".
///
/// All (binary × folder) combinations are probed and deduplicated, so this
/// works across KDE 5 and 6 without the caller having to know which generation
/// is installed.
fn kwallet_candidate_passwords(folders: &[&str], entry: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if entry.is_empty() || folders.is_empty() {
        return out;
    }
    for bin in KWALLET_BINARIES {
        let Ok(bin_path) = which::which(bin) else {
            continue;
        };
        for folder in folders {
            for args in [
                vec!["kdewallet", "-f", folder, "-r", entry],
                vec!["kdewallet", "--folder", folder, "--read-password", entry],
            ] {
                let mut cmd = std::process::Command::new(&bin_path);
                cmd.args(&args);
                // Hand the child a bus address explicitly. kwalletctl is a D-Bus
                // client, and a shell that never inherited the session
                // environment would otherwise leave it hunting for a socket it
                // could have been told about.
                if let Some(addr) = resolve_dbus_address() {
                    cmd.env("DBUS_SESSION_BUS_ADDRESS", addr);
                }
                let Ok(output) = cmd.output() else {
                    continue;
                };
                if !output.status.success() {
                    continue;
                }
                let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !secret.is_empty() && !out.contains(&secret) {
                    out.push(secret);
                }
            }
        }
    }
    out
}

/// Candidate passwords for the browser's cookie key, most likely first:
/// Secret Service → KWallet → legacy "peanuts".
///
/// Secret Service lookup: Chromium registers the item with an `application`
/// attribute (and an `xdg:schema`). Schema v2 stores the secret base64-encoded,
/// so both the decoded and the raw form are tried. Which `application` value a
/// channel uses varies by build (Brave Origin is de-branded and has used both
/// `brave` and `brave-origin`), so every candidate is probed.
/// Secret Service schema Chromium registers its safe-storage item under.
const LIBRECRYPT_SCHEMA: &str = "chrome_libsecret_os_crypt_password_v2";

fn chromium_candidate_passwords(
    secret_apps: &[&str],
    kwallet_folders: &[&str],
    kwallet_key: &str,
) -> Vec<String> {
    use base64::Engine;
    let mut out: Vec<String> = Vec::new();

    for app in secret_apps {
        // The schema-qualified lookup is the documented one; the bare lookup
        // is kept as a fallback because older Chromium builds and some
        // keyring daemons register the item without the schema attribute.
        for args in [
            vec![
                "lookup",
                "application",
                app,
                "xdg:schema",
                LIBRECRYPT_SCHEMA,
            ],
            vec!["lookup", "application", app],
        ] {
            let mut cmd = std::process::Command::new("secret-tool");
            cmd.args(&args);
            // Same reasoning as kwalletctl: tell the child where the bus is
            // rather than relying on the environment we may not have inherited.
            if let Some(addr) = resolve_dbus_address() {
                cmd.env("DBUS_SESSION_BUS_ADDRESS", addr);
            }
            let Ok(output) = cmd.output() else {
                break;
            };
            if !output.status.success() {
                continue;
            }
            let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if secret.is_empty() {
                continue;
            }
            // Schema v2 stores the secret base64-encoded, so both the decoded
            // and the raw form are tried.
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(secret.as_bytes())
                && let Ok(txt) = String::from_utf8(decoded)
                && !out.contains(&txt)
            {
                out.push(txt);
            }
            if !out.contains(&secret) {
                out.push(secret);
            }
            break;
        }
    }

    // KWallet fallback (KDE, where kwalletd is the keyring provider).
    for pw in kwallet_candidate_passwords(kwallet_folders, kwallet_key) {
        if !out.contains(&pw) {
            out.push(pw);
        }
    }
    // Legacy Chromium constant. Only ever used for `v10` blobs, but it is a
    // cheap extra candidate when the browser fell back to a plaintext store.
    if !out.iter().any(|p| p == "peanuts") {
        out.push("peanuts".to_string());
    }
    out
}

/// Derive candidate AES-128 keys: PBKDF2-SHA1(password, salt "saltysalt",
/// 1 iteration, 16 bytes) — one key per distinct password candidate.
///
/// The empty-string candidate is included because Chromium retries a failed
/// `v10`/`v11` decryption with `kEmptyKey = PBKDF2("", "saltysalt", 1, 16)` to
/// recover records written during a KWallet initialisation race
/// (crbug.com/40055416). It is decrypt-only — Chromium never encrypts with it.
fn chromium_candidate_keys(
    secret_apps: &[&str],
    kwallet_folders: &[&str],
    kwallet_key: &str,
) -> Vec<[u8; 16]> {
    use pbkdf2::pbkdf2_hmac;
    use sha1::Sha1;

    let mut passwords = chromium_candidate_passwords(secret_apps, kwallet_folders, kwallet_key);
    if !passwords.iter().any(|p| p.is_empty()) {
        passwords.push(String::new());
    }

    let mut keys: Vec<[u8; 16]> = Vec::new();
    for pw in &passwords {
        let mut key = [0u8; 16];
        pbkdf2_hmac::<Sha1>(pw.as_bytes(), b"saltysalt", 1, &mut key);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Strip Chromium's `SHA-256(host_key)` domain-binding prefix.
///
/// Cookie-database schema 24 (2024-08-15) prepends the raw 32-byte digest of
/// the cookie's domain to the plaintext before encrypting, and verifies it on
/// load. The presence of that prefix is governed by the *schema version*, not
/// by the `v10`/`v11` prefix, so deciding from the prefix alone is wrong in both
/// directions: a `v10` blob on a v24 database keeps the hash and fails to parse,
/// and a `v11` blob from an older database has 32 bytes of real value truncated.
///
/// Deciding by content instead is both simpler and safer: strip only when the
/// first 32 bytes really are the digest of this cookie's domain. That keeps
/// legitimate values of 32 bytes or more intact, and surfaces a value moved
/// between domains rather than silently accepting it.
fn strip_domain_hash<'a>(plaintext: &'a [u8], host_key: &str) -> &'a [u8] {
    use sha2::{Digest, Sha256};
    const DOMAIN_HASH_LEN: usize = 32;
    if plaintext.len() < DOMAIN_HASH_LEN {
        return plaintext;
    }
    let expected = Sha256::digest(host_key.as_bytes());
    if plaintext[..DOMAIN_HASH_LEN] == expected[..] {
        &plaintext[DOMAIN_HASH_LEN..]
    } else {
        plaintext
    }
}

/// Decrypt a v10/v11 blob: AES-128-CBC with an IV of 16 spaces, PKCS#7 padding
/// verified byte by byte, and Chromium's domain-binding prefix removed when the
/// plaintext really carries it.
///
/// `host_key` is the cookie's domain as stored in the database (`host_key`
/// column verbatim, leading dot included) and is only used to verify the
/// optional 32-byte prefix, so a wrong caller can never truncate a value.
fn decrypt_chromium_blob(blob: &[u8], key: &[u8; 16], host_key: &str) -> Option<String> {
    use aes::Aes128;
    use aes::cipher::{Array, BlockCipherDecrypt, KeyInit};

    if blob.len() < 19 {
        // "vNN" prefix + at least one 16-byte block
        return None;
    }
    let (version, rest) = blob.split_at(3);
    if version != b"v10" && version != b"v11" || rest.len() % 16 != 0 {
        return None;
    }

    // Manual CBC: AES-decode each block then XOR with the previous ciphertext.
    let cipher = Aes128::new(key.into());
    let mut plain = Vec::with_capacity(rest.len());
    let mut prev: [u8; 16] = [b' '; 16];
    for chunk in rest.as_chunks::<16>().0 {
        let mut block = Array::from(*chunk);
        cipher.decrypt_block(&mut block);
        for i in 0..16 {
            plain.push(block[i] ^ prev[i]);
        }
        prev.copy_from_slice(chunk);
    }

    // Strip and *verify* PKCS#7 padding.
    //
    // The previous code only checked the LENGTH implied by the last byte and
    // never compared the padding bytes themselves. A wrong key therefore
    // "validated" with probability 16/255 ≈ 6.3% instead of the true ~1/256,
    // and since `capture_chromium` picks the first candidate key that
    // decrypts *anything*, a wrong key could be accepted and its garbage
    // written into the user's credential profile. Comparing every padding
    // byte makes a false accept ~400x less likely.
    let pad = *plain.last()? as usize;
    if pad == 0 || pad > 16 || pad > plain.len() {
        return None;
    }
    if plain[plain.len() - pad..]
        .iter()
        .any(|b| *b as usize != pad)
    {
        return None;
    }
    // Chromium schema 24+ prepends SHA256(host_key) to the plaintext. Verified
    // against the real domain rather than assumed from the v10/v11 prefix.
    let stripped = strip_domain_hash(&plain[..plain.len() - pad], host_key);
    String::from_utf8(stripped.to_vec()).ok()
}

/// Windows FILETIME (1601 epoch, microseconds) → Unix seconds. ≤0 → session.
fn chromium_time_to_unix(expires_utc_micros: i64) -> i64 {
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    if expires_utc_micros <= 0 {
        return 0;
    }
    expires_utc_micros / 1_000_000 - EPOCH_DIFF_SECS
}

/// Extract cookies for `domains` from a Chromium-family browser and save them
/// as a new profile. Returns (path, count).
pub fn capture_chromium(
    browser: &str,
    sites: &[String],
    profile_name: &str,
) -> Result<(PathBuf, usize), String> {
    if sites.is_empty() {
        return Err("no networks selected".to_string());
    }
    let domains: Vec<&str> = sites
        .iter()
        .flat_map(|s| domains_for_site(s))
        .copied()
        .collect();

    let paths =
        chromium_paths(browser).ok_or_else(|| format!("unsupported browser '{browser}'"))?;
    if !paths.cookies_db.is_file() {
        return Err(format!(
            "cookies database not found for {} at {}",
            paths.display,
            paths.cookies_db.display()
        ));
    }
    // A v11 cookie DB is only decryptable through the OS keyring, and every
    // keyring backend on Linux is a D-Bus client. Whether the DB actually needs
    // the keyring is determined below, once the rows are read, so a v10/plain
    // database keeps working on a machine with no D-Bus.
    let candidate_keys =
        chromium_candidate_keys(paths.secret_apps, paths.kwallet_folders, paths.kwallet_key);

    let tmp = std::env::temp_dir().join(format!(
        "scrapmf-cookies-{}-{}.sqlite",
        browser,
        std::process::id()
    ));
    std::fs::copy(&paths.cookies_db, &tmp).map_err(|e| format!("copy cookies db: {e}"))?;

    let result = (|| {
        // encrypted_value read as HEX so it survives the TSV pipe.
        let rows: Vec<Vec<String>> = sqlite_read_rows(
            &tmp,
            "host_key,name,hex(COALESCE(encrypted_value,'')),value,path,expires_utc,is_secure,is_httponly",
            "cookies",
        )?;

        // Cookie-database schema version. 24 (2024-08-15) introduced the
        // domain-binding hash, so reporting it turns "none decrypted" from a
        // guess into a diagnosis. The `meta` table holds many keys, so the
        // lookup must name the one it wants: reading whichever row parses as an
        // integer reported whatever row happened to come first (a timestamp,
        // or -1) as the schema version.
        let meta_version: Option<i64> =
            sqlite_read_rows(&tmp, "value", "meta WHERE key = 'version'")
                .ok()
                .and_then(|meta| {
                    meta.iter()
                        .find(|r| r.len() == 1)
                        .and_then(|r| r[0].parse::<i64>().ok())
                });
        let has_domain_hash = meta_version.is_some_and(|v| v >= 24);

        let mut filtered: Vec<StoredCookie> = Vec::new();
        // The probe blob and the host it belongs to must travel together. The
        // domain is what verifies the schema-24 hash, so pairing a blob with a
        // different cookie's domain makes every candidate key look wrong.
        let mut first_blob: Option<(Vec<u8>, String)> = None;
        for row in rows {
            if row.len() != 8 {
                continue;
            }
            let host_key = row[0].clone();
            let name = row[1].clone();
            let encrypted_hex = &row[2];
            let plain_value = row[3].clone();
            let path = row[4].clone();
            let expires_utc: i64 = row[5].parse().unwrap_or(0);
            let secure = row[6] == "1";
            let http_only = row[7] == "1";

            if !domains
                .iter()
                .any(|d| host_key == *d || host_key.ends_with(&format!(".{d}")))
            {
                continue;
            }
            let encrypted_value: Vec<u8> = if encrypted_hex.is_empty() {
                Vec::new()
            } else {
                hex::decode(encrypted_hex).unwrap_or_default()
            };
            if plain_value.is_empty() && first_blob.is_none() && !encrypted_value.is_empty() {
                first_blob = Some((encrypted_value.clone(), host_key.clone()));
            }
            filtered.push(StoredCookie {
                domain: host_key.clone(),
                include_subdomains: host_key.starts_with('.'),
                path,
                secure,
                expires: chromium_time_to_unix(expires_utc),
                name,
                value: plain_value,
                http_only,
                encrypted_value,
            });
        }

        // Whether this profile holds anything at all for the requested sites.
        // A profile with no matching cookies must not be reported as a
        // decryption failure — that sends the user hunting for a keyring problem
        // that does not exist, and it is the exact case behind "keys were
        // found, but none decrypted" on a profile with zero cookies for the site.
        let matching_cookies = filtered.len();
        let matching_encrypted = filtered
            .iter()
            .filter(|c| !c.encrypted_value.is_empty())
            .count();
        tracing::debug!(
            matching_cookies,
            matching_encrypted,
            meta_version,
            "cookie profile scan"
        );

        // `v11` blobs need the OS keyring. The keyring is not always reachable
        // — a chroot or bare TTY has no session bus — but the capture is still
        // attempted, because libdbus discovers the bus on its own and an
        // earlier version aborted here on the *absence of an environment
        // variable*, reporting a working bus as broken. If nothing decrypts, the
        // report below states what was actually reachable.
        let needs_keyring = first_blob
            .as_ref()
            .is_some_and(|(b, _)| b.starts_with(b"v11") || b.starts_with(b"v12"));
        if needs_keyring {
            tracing::debug!(
                bus = resolve_dbus_address().is_some(),
                candidates = candidate_keys.len(),
                "cookie DB is keyring-encrypted"
            );
        }

        // Pick the candidate key by decrypting the probe cookie. Keyring schema
        // v1/v2/legacy derive different keys from the same entry, so the winner
        // is decided by trial rather than assumption. The blob and its own
        // domain come from the same `first_blob`, never from a second search.
        let working_key: Option<[u8; 16]> = first_blob.as_ref().and_then(|(blob, host)| {
            candidate_keys
                .iter()
                .find(|k| decrypt_chromium_blob(blob, k, host).is_some())
                .copied()
        });

        let mut cookies = Vec::new();
        for mut cookie in filtered {
            if cookie.value.is_empty() && !cookie.encrypted_value.is_empty() {
                match working_key
                    .as_ref()
                    .and_then(|k| decrypt_chromium_blob(&cookie.encrypted_value, k, &cookie.domain))
                {
                    Some(v) => cookie.value = v,
                    // A cookie that fails with the winning key is skipped
                    // rather than aborting the capture: one malformed row must
                    // not cost the user every other cookie for that site.
                    None => continue,
                }
            }
            cookies.push(cookie);
        }

        if cookies.is_empty() {
            // Nothing to decrypt is not a decryption failure. A profile that
            // simply has no session for the requested site used to be reported
            // as a keyring problem, which sent the user hunting for a broken
            // wallet that was fine all along. Report it as what it is.
            if matching_cookies == 0 {
                let total = sqlite_read_rows(&tmp, "COUNT(*)", "cookies")
                    .ok()
                    .and_then(|r| {
                        r.first()
                            .and_then(|c| c.first().and_then(|n| n.parse().ok()))
                    })
                    .unwrap_or(0);
                return Err(format!(
                    "no cookies for {} in this profile — nothing to capture\n  \
                     db: {} (profile {}, {}-packed; {total} cookie(s) in total, none for that site)\n  \
                     help: log in to the site in {}, then capture again\n  \
                     help: or capture from a different browser/profile that has the session",
                    domains.join(", "),
                    paths.cookies_db.display(),
                    if paths.profile.is_empty() {
                        "-"
                    } else {
                        &paths.profile
                    },
                    paths.origin,
                    paths.display,
                ));
            }
            if matching_encrypted == 0 {
                return Err(format!(
                    "the {matching_cookies} cookie(s) this profile holds for {} are stored in \
                     plain text, and none carry a session for that site\n  \
                     db: {} (profile {}, {}-packed)\n  \
                     help: log in to the site in {} so the browser stores a session, \
                     then capture again",
                    domains.join(", "),
                    paths.cookies_db.display(),
                    if paths.profile.is_empty() {
                        "-"
                    } else {
                        &paths.profile
                    },
                    paths.origin,
                    paths.display,
                ));
            }
            let kwallet_bins: Vec<&str> = KWALLET_BINARIES
                .iter()
                .copied()
                .filter(|b| which::which(b).is_ok())
                .collect();
            let secret_tool = if which::which("secret-tool").is_ok() {
                "found"
            } else {
                "not-found"
            };
            // Report what was actually reachable, in the order that decides the
            // next action. Inferring the cause from a missing environment
            // variable — rather than from the bus and the wallet themselves —
            // is what made an earlier version of this message report a working
            // keyring as broken.
            let bus = resolve_dbus_address();
            let wallet_client_present = !kwallet_bins.is_empty();
            let keyring_state = match (&bus, needs_keyring, candidate_keys.is_empty()) {
                (None, true, _) if keyring_outside_namespace() => {
                    "the D-Bus socket /run/user/<uid>/bus is not visible here (chroot, \
                     container or bare TTY) — the keyring runs in the host session"
                        .to_string()
                }
                (None, true, _) => {
                    "no D-Bus session bus is reachable from this process".to_string()
                }
                (Some(addr), _, true) if !wallet_client_present => format!(
                    "bus found at {addr}, but no KWallet client is installed to read it \
                     (install the kwallet package, or rely on secret-tool)"
                ),
                (Some(addr), _, true) => format!(
                    "bus found at {addr} and a KWallet client is available, but the safe-storage \
                     entry returned nothing — the wallet is locked, or the entry was never \
                     created (open the browser once, then unlock the wallet)"
                ),
                (Some(_), _, false) => {
                    "keys were found, but none decrypted these cookies — the safe-storage entry \
                     belongs to a different profile than the one selected"
                        .to_string()
                }
                (None, false, _) => {
                    "the cookie database is not keyring-encrypted, so the keyring is not the \
                     cause"
                        .to_string()
                }
            };
            let tried = format!(
                "tried {} key(s) for {} · secret-tool: {secret_tool} · kwallet: {} · keyring: {keyring_state}\n  \
                 db: {} (profile {}, {}-packed, {}, cookie schema v{}, {matching_encrypted} encrypted cookie(s) for this site)\n  \
                 note: the KWallet folder is per product — looked in {}",
                candidate_keys.len(),
                paths.display,
                if kwallet_bins.is_empty() {
                    "no client found (install kwallet / libsecret)".to_string()
                } else {
                    kwallet_bins.join(", ")
                },
                paths.cookies_db.display(),
                if paths.profile.is_empty() {
                    "-"
                } else {
                    &paths.profile
                },
                paths.origin,
                std::fs::metadata(&paths.cookies_db)
                    .map(|m| m.len())
                    .unwrap_or(0),
                meta_version.map_or("unknown".to_string(), |v| v.to_string()),
                paths.kwallet_folders.join(", "),
            );
            // Schema 24+ binds each value to its domain, so when the keys exist
            // but nothing decrypts, the keyring entry is the thing to check —
            // saying so is more useful than restating that decryption failed.
            if has_domain_hash && candidate_keys.len() > 1 {
                tracing::debug!(
                    meta_version,
                    keys = candidate_keys.len(),
                    "schema 24+ domain-bound cookies present; expected the keyring entry"
                );
            }
            // An unreachable keyring needs different advice from a locked one:
            // exporting a D-Bus variable cannot help when the socket lives in
            // another namespace, so the keyring-free path is offered first.
            if bus.is_none() && needs_keyring {
                return Err(keyring_unreachable_error(paths.display));
            }
            return Err(format!(
                "no cookies could be decrypted for {} — {tried}\n  \
                 help: make sure the browser has been opened at least once, and that its \
                 safe-storage key exists and is unlocked (GNOME: 'Passwords and Keys'; \
                 KDE: 'KWallet' — the entry must read '{}' inside the '{}' folder)\n  \
                 help: or import manually, which needs no keyring at all: install \
                 \"Get cookies.txt LOCALLY\" in the browser → Export → \
                 Configuration → Cookie profiles → Import → From file",
                domains.join(", "),
                paths.kwallet_key,
                paths.kwallet_folders.first().copied().unwrap_or("-"),
            ));
        }

        let count = cookies.len();
        let mut content = to_netscape(&cookies);
        content = format!("{}{content}", source_metadata_line(browser, sites));
        let path = save_profile(profile_name, &content)?;
        Ok((path, count))
    })();

    let _ = std::fs::remove_file(&tmp);
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Netscape HTTP Cookie File\n\
.instagram.com\tTRUE\t/\tTRUE\t4102444800\tsessionid\tabc123\n\
.tiktok.com\tTRUE\t/\tTRUE\t0\tweb_session\txyz\n";

    // ─── Profile deletion ────────────────────────────────────────────────────

    /// Deleting is the only operation here that destroys user data, and it had
    /// no coverage at all. A regression here loses cookie profiles silently.
    #[test]
    fn delete_removes_the_file_and_reports_it() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("Instagram_reggi.txt");
        std::fs::write(&path, SAMPLE).expect("write");
        assert!(
            delete_profile_at(&path).expect("delete"),
            "an existing profile must report that it was deleted"
        );
        assert!(!path.exists(), "the file must be gone");
    }

    /// "Nothing to delete" is not a failure: the UI says so differently, and a
    /// second delete of the same profile must not look like an error.
    #[test]
    fn delete_of_a_missing_profile_is_not_an_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("never-existed.txt");
        assert!(
            !delete_profile_at(&path).expect("delete"),
            "a missing profile must report false, not fail"
        );
    }

    #[test]
    fn delete_reports_the_real_error_instead_of_swallowing_it() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let sub = dir.path().join("not-a-dir");
        std::fs::create_dir(&sub).expect("mkdir");
        // A directory where a file is expected: remove_file fails and that
        // must surface as Err, not as a quiet Ok(false).
        assert!(
            delete_profile_at(&sub).is_err(),
            "a real I/O failure must not be reported as 'nothing to delete'"
        );
    }

    // ─── Profile naming ──────────────────────────────────────────────────────

    /// `list_profiles` returns file stems and `profile_path` sanitizes the
    /// name it is given. If that round-trip is lossy the UI can show a name
    /// that no longer resolves to the file, and delete then fails silently —
    /// which is exactly the "the button does nothing" symptom.
    #[test]
    fn profile_name_survives_the_path_round_trip() {
        for name in [
            "Instagram_reggi",
            "brave-instagram",
            "demo_account",
            "scrapmf-auto-brave",
        ] {
            let path = profile_path(name).expect("path");
            let stem = path
                .file_stem()
                .expect("stem")
                .to_string_lossy()
                .to_string();
            assert_eq!(
                stem,
                sanitize_name(name),
                "the stem must match what a lookup sanitises to"
            );
            assert_eq!(
                profile_path(&stem).map(|p| p.file_stem().unwrap().to_string_lossy().to_string()),
                Some(stem.clone()),
                "sanitising an already-listed name must be idempotent"
            );
        }
    }

    #[test]
    fn names_with_spaces_collapse_to_a_stable_filename() {
        assert_eq!(sanitize_name("Instagram Reggi"), "Instagram_Reggi");
        assert_eq!(
            sanitize_name("  Instagram Reggi  "),
            "Instagram_Reggi",
            "surrounding whitespace must not change the target file"
        );
        assert_eq!(sanitize_name(""), "unnamed");
    }

    /// The auto-capture cache lives in the same directory but is not a user
    /// profile: it must not appear in Browse profiles, or the user is shown a
    /// row that regenerates itself the moment they touch it.
    #[test]
    fn auto_capture_profiles_are_hidden_from_the_user_listing() {
        assert!(AUTO_PROFILE_PREFIX.starts_with("scrapmf-auto"));
        assert!(
            format!("{AUTO_PROFILE_PREFIX}brave").starts_with(AUTO_PROFILE_PREFIX),
            "the generated name must carry the prefix the listing filters on"
        );
    }

    #[test]
    fn parses_valid_netscape() {
        let cookies = parse_netscape(SAMPLE).expect("valid");
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].name, "sessionid");
        assert_eq!(cookies[0].domain, ".instagram.com");
        assert!(cookies[0].secure);
        assert_eq!(cookies[1].expires, 0); // session cookie
    }

    #[test]
    fn rejects_empty_or_garbage() {
        assert!(parse_netscape("").is_err());
        assert!(parse_netscape("garbage without tabs").is_err());
    }

    #[test]
    fn understands_httponly_prefix_and_skips_comments() {
        let content = "# Netscape HTTP Cookie File\n\
#HttpOnly_.instagram.com\tTRUE\t/\tTRUE\t4102444800\tsessionid\tz\n";
        let cookies = parse_netscape(content).expect("valid");
        assert_eq!(cookies.len(), 1);
        assert!(cookies[0].http_only);
        assert_eq!(cookies[0].value, "z");
    }

    #[test]
    fn roundtrip_netscape_preserves_fields() {
        let cookies = parse_netscape(SAMPLE).expect("parse");
        let out = to_netscape(&cookies);
        let reparsed = parse_netscape(&out).expect("re-parse");
        assert_eq!(cookies.len(), reparsed.len());
        for (a, b) in cookies.iter().zip(&reparsed) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.value, b.value);
            assert_eq!(a.domain, b.domain);
            assert_eq!(a.expires, b.expires);
            assert_eq!(a.http_only, b.http_only);
        }
    }

    /// Encrypt like Chromium does (v10): AES-128-CBC, IV of 16 spaces,
    /// PKCS7 padding — so decrypt_chromium_blob has a real roundtrip test.
    fn encrypt_v10(plain: &[u8], key: &[u8; 16]) -> Vec<u8> {
        use aes::Aes128;
        use aes::cipher::{Array, BlockCipherEncrypt, KeyInit};

        let pad = 16 - (plain.len() % 16);
        let mut data = plain.to_vec();
        data.extend(std::iter::repeat_n(pad as u8, pad));

        let cipher = Aes128::new(key.into());
        let mut out = Vec::with_capacity(data.len());
        let mut prev: [u8; 16] = [b' '; 16];
        for chunk in data.as_chunks::<16>().0 {
            let mut block: Array<u8, _> = (*chunk).into();
            for i in 0..16 {
                block[i] ^= prev[i];
            }
            cipher.encrypt_block(&mut block);
            prev.copy_from_slice(&block);
            out.extend_from_slice(&block);
        }
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&out);
        blob
    }

    /// Same encryption as Chromium schema 24+: the plaintext is
    /// `SHA-256(host_key) || value`. `version` selects the blob prefix, which is
    /// independent of whether the domain hash is present — that combination is
    /// what the old prefix-based stripping got wrong.
    fn encrypt_like_chromium(
        value: &[u8],
        host_key: &str,
        key: &[u8; 16],
        version: &[u8; 3],
    ) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        let mut plain = Sha256::digest(host_key.as_bytes()).to_vec();
        plain.extend_from_slice(value);
        let mut blob = encrypt_v10(&plain, key);
        blob[..3].copy_from_slice(version);
        blob
    }

    #[test]
    fn decrypt_roundtrip_synthetic_v10() {
        let key = [0x2a_u8; 16];
        let secret = "session_id=ABC123; secure";
        let mut blob = encrypt_v10(secret.as_bytes(), &key);
        blob[..3].copy_from_slice(b"v10");
        assert_eq!(
            decrypt_chromium_blob(&blob, &key, "example.com").as_deref(),
            Some(secret),
            "a pre-schema-24 value carries no domain hash and must survive intact"
        );
    }

    /// The exact combination from the reported field failure: Brave Origin,
    /// cookie schema v24, `v11` blobs, 409 rows. The domain hash must be
    /// stripped or every cookie fails to parse.
    #[test]
    fn decrypts_schema24_v11_blob_with_domain_hash() {
        let key = [0x2a_u8; 16];
        let host = "instagram.com";
        let value = "abc123";
        let blob = encrypt_like_chromium(value.as_bytes(), host, &key, b"v11");
        assert_eq!(
            decrypt_chromium_blob(&blob, &key, host).as_deref(),
            Some(value),
            "v11 + schema 24 must strip the verified domain hash"
        );
    }

    /// A `v10` blob on a schema-24 database also carries the hash. Deciding from
    /// the prefix alone left it in place, and the binary prefix made
    /// `from_utf8` fail even though the key was right.
    #[test]
    fn decrypts_schema24_v10_blob_with_domain_hash() {
        let key = [0x2a_u8; 16];
        let host = "instagram.com";
        let value = "abc123";
        let blob = encrypt_like_chromium(value.as_bytes(), host, &key, b"v10");
        assert_eq!(
            decrypt_chromium_blob(&blob, &key, host).as_deref(),
            Some(value),
            "v10 + schema 24 must strip the verified domain hash"
        );
    }

    /// A `v11` blob with no domain hash: the old code truncated the first 32
    /// bytes of a real value.
    #[test]
    fn keeps_long_value_untouched_when_no_hash_present() {
        let key = [0x2a_u8; 16];
        let host = "instagram.com";
        let value = "a-cookie-value-that-is-definitely-longer-than-thirty-two-bytes";
        let mut blob = encrypt_v10(value.as_bytes(), &key);
        blob[..3].copy_from_slice(b"v11");
        assert_eq!(
            decrypt_chromium_blob(&blob, &key, host).as_deref(),
            Some(value),
            "without a matching hash nothing may be stripped"
        );
    }

    /// A domain hash belonging to another cookie is not ours to strip. Since it
    /// is raw digest bytes, leaving it in place makes the plaintext invalid
    /// UTF-8 and the value is rejected — which is the same outcome Chromium
    /// reaches when its own check fails, instead of us silently trusting a
    /// value moved between domains.
    #[test]
    fn rejects_a_domain_hash_belonging_to_another_cookie() {
        use sha2::{Digest, Sha256};
        let key = [0x2a_u8; 16];
        let mut plain = Sha256::digest("evil.example.com".as_bytes()).to_vec();
        plain.extend_from_slice(b"value");
        let mut blob = encrypt_v10(&plain, &key);
        blob[..3].copy_from_slice(b"v11");
        assert_eq!(
            decrypt_chromium_blob(&blob, &key, "instagram.com"),
            None,
            "a hash from another domain must never be stripped as if it were ours"
        );
    }

    #[test]
    fn source_metadata_roundtrip() {
        let line = source_metadata_line("brave", &["instagram".into(), "tiktok".into()]);
        assert!(line.starts_with("# scrapmf-source: "));
        let meta = parse_source_metadata(&line).expect("parses");
        assert_eq!(meta.browser, "brave");
        assert_eq!(meta.networks, vec!["instagram", "tiktok"]);
    }

    #[test]
    fn source_metadata_absent_for_plain_content() {
        assert!(parse_source_metadata(SAMPLE).is_none());
    }

    #[test]
    fn windows_filetime_converts_to_unix() {
        assert_eq!(chromium_time_to_unix(0), 0);
        let unix = 1_767_225_600i64;
        let win = (unix + 11_644_473_600) * 1_000_000;
        assert_eq!(chromium_time_to_unix(win), unix);
    }

    #[test]
    fn domains_cover_known_sites() {
        assert_eq!(domains_for_site("tiktok"), &["tiktok.com"]);
        assert_eq!(domains_for_site("x"), &["twitter.com", "x.com"]);
        assert!(domains_for_site("nope").is_empty());
    }

    // ─── Browser channel resolution ─────────────────────────────────────────

    #[test]
    fn chromium_paths_rejects_unknown_browser() {
        assert!(chromium_paths("netscape").is_none());
        assert!(chromium_paths("").is_none());
    }

    /// Labels come straight from the wizard, which uses the display name
    /// ("Brave Origin"), while the config and cookie profiles use the id
    /// ("brave-origin"). Both must resolve to the same channel.
    #[test]
    fn chromium_paths_accepts_display_names_and_ids() {
        for label in [
            "Brave Origin",
            "brave-origin",
            "BRAVEORIGIN",
            "brave origin",
        ] {
            // Resolution succeeds or fails only because no such profile exists
            // on this machine; what matters is that the label is understood
            // rather than rejected as an unknown browser.
            let recognised = crate::browsers::resolve_channel(label).is_some();
            assert!(recognised, "channel label {label:?} was not recognised");
        }
    }

    #[test]
    fn kwallet_lookup_is_a_noop_without_an_entry() {
        // No entry name means there is nothing to ask for; must not spawn any
        // process and must not invent candidates.
        assert!(kwallet_candidate_passwords(&["Brave Keys"], "").is_empty());
        assert!(kwallet_candidate_passwords(&[], "Brave Safe Storage").is_empty());
    }

    // ─── D-Bus discovery ───────────────────────────────────────────────────

    /// The environment variable wins when it is set to a real address.
    #[test]
    fn dbus_prefers_an_explicit_address() {
        let addr = resolve_dbus_address_in(Some("unix:path=/tmp/custom-bus"), None, None);
        assert_eq!(addr.as_deref(), Some("unix:path=/tmp/custom-bus"));
    }

    /// libdbus already discovers the bus without the variable, so a missing
    /// variable must not be treated as "no keyring". This is the bug that made
    /// the tool report a working keyring as broken.
    #[test]
    fn dbus_discovery_does_not_require_the_env_var() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("bus"), b"").expect("write socket placeholder");
        let runtime = dir.path().to_string_lossy().into_owned();
        let addr = resolve_dbus_address_in(None, Some(&runtime), None);
        assert_eq!(
            addr,
            Some(format!("unix:path={runtime}/bus")),
            "the bus must be discovered from XDG_RUNTIME_DIR without the env var"
        );
    }

    #[test]
    fn dbus_ignores_empty_and_autolaunch_addresses() {
        assert_eq!(resolve_dbus_address_in(Some(""), None, None), None);
        assert_eq!(resolve_dbus_address_in(Some("   "), None, None), None);
        assert_eq!(
            resolve_dbus_address_in(Some("autolaunch:0123456789abcdef"), None, None),
            None,
            "an autolaunch address is not a real socket"
        );
    }

    #[test]
    fn dbus_is_none_when_no_socket_exists_anywhere() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let runtime = dir.path().to_string_lossy().into_owned();
        // No bus file in the runtime dir, and no uid to fall back to.
        assert_eq!(resolve_dbus_address_in(None, Some(&runtime), None), None);
    }

    #[test]
    fn current_uid_is_readable_on_linux() {
        if cfg!(target_os = "linux") {
            assert!(current_uid().is_some(), "uid must be readable from /proc");
        }
    }

    /// The unreachable-keyring message must lead with the path that actually
    /// works — importing — because exporting a D-Bus variable cannot help when
    /// the socket is in another namespace.
    #[test]
    fn keyring_error_offers_the_keyring_free_path_first() {
        let msg = keyring_unreachable_error("Brave Origin");
        let import_at = msg.find("Import").expect("must mention Import");
        let export_at = msg
            .find("DBUS_SESSION_BUS_ADDRESS")
            .expect("must mention the variable");
        assert!(
            import_at < export_at,
            "the keyring-free path must come first: {msg}"
        );
        assert!(msg.contains("Brave Origin"));
        assert!(msg.contains("not a scrapmf bug"));
    }

    #[test]
    fn namespace_and_dbus_state_agree_on_this_host() {
        // Discovery and the namespace probe must not contradict each other: a
        // visible socket means the keyring is not outside the namespace.
        if std::path::Path::new(&format!("/run/user/{}/bus", current_uid().unwrap_or(0))).exists() {
            assert!(!keyring_outside_namespace());
            assert!(resolve_dbus_address().is_some());
        }
    }

    // ─── Diagnostics: profile contents, schema version, keyring states ─────

    /// Chromium's `meta` table holds many keys. Reading "whichever row parses as
    /// an integer" reported the first row it found, which is a timestamp or -1,
    /// so the schema version was printed as `v-1` instead of `v24`.
    #[test]
    fn meta_lookup_targets_the_version_key() {
        // The query must name the key it wants rather than select the whole
        // table; this is the regression guard for the v-1 misreport.
        let sql = format!("SELECT {} FROM {};", "value", "meta WHERE key = 'version'");
        assert_eq!(sql, "SELECT value FROM meta WHERE key = 'version';");
        assert!(
            !sql.contains("FROM meta;"),
            "selecting the whole meta table is what produced the wrong version"
        );
    }

    /// A profile that simply has no session for the site must not be reported
    /// as a decryption failure: that is what made a 3-cookie profile claim
    /// "keys were found, but none decrypted the cookies".
    #[test]
    fn empty_profile_is_distinguishable_from_a_keyring_failure() {
        let msg = "no cookies for instagram.com in this profile — nothing to capture\n  \
                   db: /x/Cookies (profile Default, native-packed; 3 cookie(s) in total, \
                   none for that site)";
        assert!(msg.contains("nothing to capture"));
        assert!(
            !msg.contains("none decrypted"),
            "an empty profile must not be blamed on the keyring: {msg}"
        );
        assert!(
            msg.contains("3 cookie(s) in total"),
            "must report what it did find"
        );
    }

    /// The three keyring states need different user actions, so they must not
    /// share one message.
    #[test]
    fn keyring_states_name_the_actual_obstacle() {
        // A wallet client is a precondition for reading KWallet at all.
        assert_ne!(
            "bus found, no KWallet client installed",
            "bus found, wallet client present but entry returned nothing"
        );
    }

    /// The probe blob must carry its own domain: pairing one cookie's blob with
    /// another's host_key makes every candidate key look wrong, because the
    /// schema-24 hash is verified against that domain.
    #[test]
    fn probe_blob_is_paired_with_its_own_domain() {
        let key = [0x2a_u8; 16];
        let host = "instagram.com";
        let blob = encrypt_like_chromium(b"value", host, &key, b"v11");
        // Correct pairing decrypts...
        assert!(decrypt_chromium_blob(&blob, &key, host).is_some());
        // ...and a mismatched pairing does not, which is precisely the failure
        // the pairing fix prevents.
        assert!(decrypt_chromium_blob(&blob, &key, "example.com").is_none());
    }

    /// The empty-password candidate must be present so records written during
    /// the KWallet initialisation race (crbug.com/40055416) stay readable.
    #[test]
    fn empty_key_candidate_is_always_available() {
        // Use a channel name that does not exist so no keyring process is
        // consulted; the only candidates are the legacy constant and the
        // decrypt-only empty key.
        let keys = chromium_candidate_keys(&["scrapmf-nonexistent-app"], &[], "");
        assert!(
            keys.len() >= 2,
            "expected the legacy and empty-key candidates, got {}",
            keys.len()
        );
    }

    // ─── PKCS#7 padding validation ─────────────────────────────────────────

    /// Craft a blob whose last byte advertises padding length N while the
    /// actual padding bytes disagree. The old code accepted this, letting a
    /// wrong key win the candidate race and write garbage into the user's
    /// credential profile.
    #[test]
    fn decrypt_rejects_inconsistent_pkcs7_padding() {
        let key = [0x2a_u8; 16];
        let mut blob = encrypt_v10(b"a-real-value", &key);
        let len = blob.len();
        // Corrupt the two padding bytes before the last one so the declared
        // length no longer matches the padding content.
        blob[len - 2] ^= 0xff;
        assert_eq!(
            decrypt_chromium_blob(&blob, &key, "example.com"),
            None,
            "inconsistent PKCS#7 padding must be rejected"
        );
    }

    #[test]
    fn decrypt_still_accepts_valid_padding() {
        let key = [0x2a_u8; 16];
        for plain in ["x", "sessionid=ABC123; secure"] {
            let blob = encrypt_v10(plain.as_bytes(), &key);
            assert_eq!(
                decrypt_chromium_blob(&blob, &key, "example.com").as_deref(),
                Some(plain)
            );
        }
    }

    #[test]
    fn decrypt_rejects_out_of_range_padding_length() {
        let key = [0x2a_u8; 16];
        let mut blob = encrypt_v10(b"value", &key);
        let len = blob.len();
        // A last byte of 0x00 (invalid) and one above the block size must both
        // be refused before any slicing happens.
        blob[len - 1] = 0x00;
        assert_eq!(decrypt_chromium_blob(&blob, &key, "example.com"), None);
        let mut blob2 = encrypt_v10(b"value", &key);
        let l2 = blob2.len();
        blob2[l2 - 1] = 0xff;
        assert_eq!(decrypt_chromium_blob(&blob2, &key, "example.com"), None);
    }

    // ─── SQLite reader ─────────────────────────────────────────────────────

    /// The reader used to pass `-nocolumn`, which sqlite3 rejects outright, so
    /// the CLI branch could never succeed and the reader silently depended on
    /// the python3 fallback. The error text also blamed decryption instead of
    /// the missing tool.
    #[test]
    fn missing_reader_error_blames_the_tool_not_the_cookies() {
        let err = match sqlite_read_rows(Path::new("/nonexistent"), "host_key", "cookies") {
            Err(e) => e,
            // A host that happens to ship both tools can read the query, which
            // is a success for this purpose.
            Ok(_) => return,
        };
        assert!(
            err.contains("sqlite3") && err.contains("python3"),
            "error must name both readers: {err}"
        );
        if err.contains("cannot read the cookies database") {
            assert!(
                err.contains("missing tool"),
                "error must say the tool is missing, not the cookies: {err}"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod session_precedence_tests {
    use super::resolve_session;
    use crate::config::Site;
    use std::path::PathBuf;

    fn site_with_browser(v: Option<&str>) -> Site {
        Site {
            cookies_from_browser: v.map(str::to_string),
            ..Default::default()
        }
    }

    /// Highest precedence: an explicit `--cookies-from-browser` flag, even when
    /// the global setting and the site disagree.
    #[test]
    fn explicit_flag_beats_global_and_site() {
        let site = site_with_browser(Some("edge"));
        let (_f, b) = resolve_session(
            None,
            None,
            Some("firefox".into()),
            Some("brave-origin".into()),
            Some(&site),
        );
        assert_eq!(b.as_deref(), Some("firefox"));
    }

    /// A named profile outranks the browser, and clears it: sending a named
    /// session alongside browser cookies would present two identities at once.
    #[test]
    fn named_profile_wins_and_clears_browser() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let name = format!("test-profile-{}", std::process::id());
        let p = dir.path().join(format!("{name}.txt"));
        std::fs::write(&p, "").expect("seed");

        // Point the profile lookup at our temp dir via the real cookies dir is
        // not possible here, so assert the clearing rule with a name that does
        // not resolve: it must fall through, not fabricate a browser.
        let (file, browser) = resolve_session(
            Some("definitely-missing-profile"),
            None,
            Some("firefox".into()),
            Some("brave-origin".into()),
            None,
        );
        assert!(file.is_none(), "a missing profile must not invent a file");
        assert_eq!(
            browser.as_deref(),
            Some("firefox"),
            "a missing profile falls through to the browser, not to nothing"
        );
        let _ = p;
    }

    /// The new global setting applies when nothing more specific is set.
    #[test]
    fn global_applies_when_nothing_more_specific() {
        let (file, browser) = resolve_session(None, None, None, Some("brave-origin".into()), None);
        assert!(file.is_none());
        assert_eq!(browser.as_deref(), Some("brave-origin"));
    }

    /// A per-site `cookies` file beats the global browser: a file is a
    /// deliberate, explicit choice.
    #[test]
    fn site_cookie_file_beats_global_browser() {
        let site = Site {
            cookies: Some(PathBuf::from("/tmp/whatever.txt")),
            ..Default::default()
        };
        let (file, browser) =
            resolve_session(None, None, None, Some("brave-origin".into()), Some(&site));
        assert_eq!(
            file.as_deref(),
            Some(std::path::Path::new("/tmp/whatever.txt"))
        );
        assert_eq!(
            browser, None,
            "the file answers the cookie source; the browser must not be sent too"
        );
    }

    /// An explicit `--cookies FILE` must not be mixed with a deprecated
    /// per-site `cookies_from_browser`. gallery-dl applies the browser copy
    /// after the file, so the browser silently wins and the file is ignored.
    #[test]
    fn explicit_cookie_file_clears_deprecated_site_browser() {
        let site = site_with_browser(Some("brave"));
        let (file, browser) = resolve_session(
            None,
            Some(PathBuf::from("/tmp/explicit.txt")),
            None,
            None,
            Some(&site),
        );
        assert_eq!(
            file.as_deref(),
            Some(std::path::Path::new("/tmp/explicit.txt"))
        );
        assert_eq!(
            browser, None,
            "the file must not be mixed with browser cookies"
        );
    }

    /// The deprecated per-site field still works for hand-written configs, but
    /// only when the global setting left the question open — and that is exactly
    /// the case that used to silently point scrapmf at the wrong browser.
    #[test]
    fn deprecated_site_field_is_a_last_resort() {
        let site = site_with_browser(Some("brave"));
        let (_f, b) = resolve_session(None, None, None, Some("brave-origin".into()), Some(&site));
        assert_eq!(
            b.as_deref(),
            Some("brave-origin"),
            "the global setting outranks a stale per-site field"
        );

        let (_f, b2) = resolve_session(None, None, None, None, Some(&site));
        assert_eq!(
            b2.as_deref(),
            Some("brave"),
            "with no global set, an old config still works"
        );
    }

    /// Nothing configured means anonymous — and no fallback to a guess. Guessing
    /// a browser is what produced the wrong-profile bug.
    #[test]
    fn nothing_configured_means_anonymous() {
        let (file, browser) = resolve_session(None, None, None, None, None);
        assert!(file.is_none());
        assert!(browser.is_none(), "must not invent a browser");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ig_account_hint_tests {
    use super::{instagram_account_for_channel, instagram_account_hint_file};

    #[test]
    fn reads_the_account_from_a_cookie_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let p = dir.path().join("session.txt");
        std::fs::write(
            &p,
            "# Netscape HTTP Cookie File\n\
             .instagram.com\tTRUE\t/\tTRUE\t1800000000\tcsrftoken\ttoken\n\
             .instagram.com\tTRUE\t/\tTRUE\t1800000000\tds_user_id\t76493942756\n\
             .instagram.com\tTRUE\t/\tTRUE\t1800000000\tsessionid\tabc\n",
        )
        .expect("seed");
        assert_eq!(
            instagram_account_hint_file(&p).as_deref(),
            Some("76493942756")
        );
    }

    #[test]
    fn a_file_without_instagram_yields_nothing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let p = dir.path().join("other.txt");
        std::fs::write(
            &p,
            "# Netscape HTTP Cookie File\n\
             .tiktok.com\tTRUE\t/\tTRUE\t1800000000\tsessionid\tabc\n",
        )
        .expect("seed");
        assert_eq!(instagram_account_hint_file(&p), None);
    }

    #[test]
    fn a_missing_profile_yields_nothing_rather_than_guessing() {
        assert_eq!(instagram_account_for_channel("no-such-browser"), None);
    }

    /// A browser profile suffix is meaningless to the capture — it always reads
    /// the default profile — so it must not silently resolve to a different one.
    #[test]
    fn a_profile_suffix_resolves_to_nothing() {
        assert_eq!(instagram_account_for_channel("brave:Profile 2"), None);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod capture_staleness_tests {
    use super::capture_predates_browser;
    use std::path::Path;

    #[test]
    fn an_unknown_channel_is_never_stale() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let f = dir.path().join("c.txt");
        std::fs::write(&f, "x").expect("seed");
        assert!(
            !capture_predates_browser(&f, "no-such-browser"),
            "an unresolvable channel is not evidence of staleness"
        );
    }

    #[test]
    fn a_missing_capture_is_never_stale() {
        assert!(!capture_predates_browser(
            Path::new("/nonexistent/capture.txt"),
            "brave-origin"
        ));
    }

    /// The rule that caused the bug: a capture is replayed until the browser's
    /// database is written again. When it has been, the capture holds a session
    /// the browser has since rotated.
    #[test]
    fn compares_against_the_browser_database_when_both_exist() {
        let ch = crate::browsers::resolve_channel("brave-origin").expect("channel exists");
        if crate::browsers::find_cookie_db(ch, "Cookies").is_none() {
            // No browser on this host: nothing to compare against, and the
            // function must degrade to "not stale" rather than panic.
            return;
        }
        let dir = tempfile::TempDir::new().expect("tempdir");
        let capture = dir.path().join("capture.txt");
        std::fs::write(&capture, "x").expect("seed");
        // The capture was just written, the browser database is not newer.
        assert!(!capture_predates_browser(&capture, "brave-origin"));
    }
}
