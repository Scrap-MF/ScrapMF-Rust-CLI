//! Instagram ID → username resolver for quick scrape.
//! Only for `site == "instagram"`. Accepts numeric ID (7-19 digits) and
//! returns the canonical `username` via the private IG API using the same
//! cookies the site is configured with (file or browser). No ID is ever
//! logged or persisted; only the resolved username is used for paths.

use std::path::Path;

use anyhow::{Context, Result};

/// True if `raw` looks like a numeric Instagram user ID (only digits, 7-19
/// chars after trimming `@`/`id:`). Usernames contain letters or `.`/`_`.
pub fn is_id_like(raw: &str) -> bool {
    let t = raw
        .trim()
        .trim_start_matches('@')
        .trim_start_matches("id:")
        .trim();
    if t.is_empty() || t.len() < 7 || t.len() > 19 {
        return false;
    }
    t.chars().all(|c| c.is_ascii_digit())
}

/// Normalize raw input: trim, strip leading `@`/`id:`.
pub fn normalize_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('@')
        .trim_start_matches("id:")
        .trim()
        .to_string()
}

/// Resolve `id` → `username` for Instagram.
pub fn resolve_instagram_username(
    id: &str,
    cookies_file: Option<&Path>,
    cookies_browser: Option<&str>,
) -> Result<String> {
    let id = normalize_id(id);
    if !is_id_like(&id) {
        anyhow::bail!("not an ID-like value");
    }
    if let Ok(u) = resolve_via_ig_api(&id, cookies_file, cookies_browser) {
        return Ok(u);
    }
    resolve_via_gallery_dl(&id, cookies_file, cookies_browser)
}

fn resolve_via_ig_api(
    id: &str,
    cookies_file: Option<&Path>,
    cookies_browser: Option<&str>,
) -> Result<String> {
    if which::which("curl").is_err() {
        anyhow::bail!("curl not found");
    }
    let temp_cookie: Option<std::path::PathBuf> = if let Some(p) = cookies_file {
        Some(p.to_path_buf())
    } else if let Some(browser) = cookies_browser {
        let tmp_name = format!("__resolve_{}", std::process::id());
        match crate::config::cookies::capture_chromium(
            browser,
            &["instagram".to_string()],
            &tmp_name,
        ) {
            Ok((path, _)) => Some(path),
            Err(_) => {
                match crate::config::cookies::capture_firefox(&["instagram".to_string()], &tmp_name)
                {
                    Ok((path, _)) => Some(path),
                    Err(e) => anyhow::bail!("cannot capture cookies for resolver: {e}"),
                }
            }
        }
    } else {
        None
    };

    // Extract csrftoken for X-CSRFToken header if we have a Netscape file
    let cookie_path_for_csrf: Option<&Path> = temp_cookie.as_deref().or(cookies_file);
    let csrf_token = cookie_path_for_csrf
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|content| {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') && !line.starts_with("#HttpOnly_") {
                    continue;
                }
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.len() == 7 && cols[5] == "csrftoken" {
                    return Some(cols[6].to_string());
                }
            }
            None
        });

    let mut result: Option<String> = None;
    for base in [
        format!("https://i.instagram.com/api/v1/users/{id}/info/"),
        format!("https://www.instagram.com/api/v1/users/{id}/info/"),
    ] {
        let mut cmd = std::process::Command::new("curl");
        cmd.arg("-sL")
            .arg("--max-time")
            .arg("12")
            .arg("-H")
            .arg("User-Agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
            .arg("-H")
            .arg("X-IG-App-ID: 936619743392459")
            .arg("-H")
            .arg("Accept: application/json")
            .arg("-H")
            .arg("Referer: https://www.instagram.com/")
            .arg("-H")
            .arg("X-Requested-With: XMLHttpRequest");
        if let Some(ref t) = csrf_token {
            cmd.arg("-H").arg(format!("X-CSRFToken: {t}"));
        }
        cmd.arg(&base);
        if let Some(ref c) = temp_cookie {
            cmd.arg("--cookie").arg(c);
        } else if let Some(p) = cookies_file {
            cmd.arg("--cookie").arg(p);
        }
        let out = match cmd.output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(u) = v
                .get("user")
                .and_then(|u| u.get("username"))
                .and_then(|s| s.as_str())
            {
                result = Some(u.to_string());
                break;
            }
            if let Some(u) = v.get("username").and_then(|s| s.as_str()) {
                result = Some(u.to_string());
                break;
            }
            if let Some(u) = v
                .get("data")
                .and_then(|d| d.get("user"))
                .and_then(|u| u.get("username"))
                .and_then(|s| s.as_str())
            {
                result = Some(u.to_string());
                break;
            }
        }
    }
    if let Some(p) = &temp_cookie
        && p.to_string_lossy().contains("__resolve_")
    {
        let _ = std::fs::remove_file(p);
    }
    if let Some(u) = result {
        return Ok(u);
    }
    anyhow::bail!("username not found in API response");
}

#[allow(clippy::collapsible_if)]
fn resolve_via_gallery_dl(
    id: &str,
    cookies_file: Option<&Path>,
    cookies_browser: Option<&str>,
) -> Result<String> {
    let exe = crate::application::backend::gallery_dl_executable()
        .context("gallery-dl not available for resolver")?;
    let url = format!("https://www.instagram.com/{id}/");
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    args.push(std::ffi::OsString::from("-K"));
    if let Some(f) = cookies_file {
        args.push(std::ffi::OsString::from("--cookies"));
        args.push(f.as_os_str().to_owned());
    }
    if let Some(b) = cookies_browser {
        args.push(std::ffi::OsString::from("--cookies-from-browser"));
        args.push(std::ffi::OsString::from(b));
    }
    args.push(std::ffi::OsString::from(url));
    let out = crate::process::Executor::run_capturing(&exe.to_string_lossy(), &args)
        .context("gallery-dl -K failed")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "username" {
            if let Some(val) = lines.next() {
                let v = val.trim();
                if !v.is_empty()
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
                {
                    return Ok(v.to_string());
                }
            }
        }
        if line.contains("\"username\"") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(u) = v.get("username").and_then(|s| s.as_str()) {
                    return Ok(u.to_string());
                }
            }
            if let Some(start) = line.find("\"username\"") {
                let rest = &line[start..];
                if let Some(colon) = rest.find(':') {
                    let after = rest[colon + 1..].trim().trim_start_matches('"');
                    let end = after.find('"').unwrap_or(after.len());
                    let candidate = &after[..end];
                    if !candidate.is_empty() {
                        return Ok(candidate.to_string());
                    }
                }
            }
        }
    }
    anyhow::bail!("could not resolve ID via gallery-dl");
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{is_id_like, normalize_id};

    #[test]
    fn detects_id_like() {
        assert!(is_id_like("1234567890"));
        assert!(is_id_like("  1234567  "));
        assert!(is_id_like("@1234567890"));
        assert!(is_id_like("id:1234567890"));
        assert!(!is_id_like("someuser"));
        assert!(!is_id_like("user.name_123"));
        assert!(!is_id_like("12345"));
        assert!(!is_id_like("abc123456"));
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_id("@id:1234567890"), "1234567890");
        assert_eq!(normalize_id("  1234567890  "), "1234567890");
    }
}
