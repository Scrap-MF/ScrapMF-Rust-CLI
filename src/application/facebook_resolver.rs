//! Facebook ID / URL → username resolver for quick scrape.
//! Accepts numeric ID (7-19 digits) or full profile URL (facebook.com,
//! fb.com, m.facebook.com, profile.php?id=, people/Name/ID, vanity) and
//! returns canonical `username` via gallery-dl -K using the site's cookies.
//! No ID is logged or persisted; only the resolved username is used for paths.

use std::path::Path;

use anyhow::{Context, Result};

/// True if `raw` looks like a numeric Facebook user ID (only digits, 7-19
/// chars after trimming `@`/`id:`/URL prefix).
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

/// True if input looks like a Facebook URL (contains facebook.com or fb.com).
pub fn is_url_like(raw: &str) -> bool {
    let lower = raw.to_lowercase();
    lower.contains("facebook.com") || lower.contains("fb.com")
}

/// Normalize raw ID: trim, strip `@`/`id:`.
pub fn normalize_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('@')
        .trim_start_matches("id:")
        .trim()
        .to_string()
}

fn title_to_username(title: &str) -> String {
    let t = title.trim();
    let stripped = if t.to_lowercase().starts_with("fotos de ") {
        t[9..].trim()
    } else {
        t
    };
    let sanitized: String = stripped
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    sanitized.trim_matches('_').to_string()
}

/// Extract identifier (username or ID) from a Facebook URL or raw input.
/// Handles:
/// - https://www.facebook.com/profile.php?id=123456789012345[&...]
/// - https://www.facebook.com/people/John-Doe/123456789012345/
/// - https://www.facebook.com/username
/// - https://m.facebook.com/username
/// - https://fb.com/username
/// - raw ID "123456789012345"
/// - raw username "someuser"
#[allow(clippy::filter_next)]
pub fn extract_identifier(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    // If it's a URL containing facebook, parse it
    if is_url_like(trimmed) {
        // Try profile.php?id= first
        if let Some(pos) = trimmed.find("profile.php?id=") {
            let after = &trimmed[pos + "profile.php?id=".len()..];
            let id = after
                .split(['&', '/', '?', '#'])
                .next()
                .unwrap_or(after)
                .trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        // Try people/Name/ID
        if let Some(pos) = trimmed.find("people/") {
            let after = &trimmed[pos + "people/".len()..];
            // after is "Name/ID/..." or "Name/ID"
            let parts: Vec<&str> = after.split('/').collect();
            if parts.len() >= 2 {
                let id = parts[1].split(['?', '#', '&']).next().unwrap_or(parts[1]);
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
            // fallback: first segment after people/ if only one part
            if let Some(first) = parts.first() {
                let cand = first.split(['?', '#', '&']).next().unwrap_or(first);
                if !cand.is_empty() {
                    return Some(cand.to_string());
                }
            }
        }
        // Generic: take first path segment (account) before ?#&
        let without_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed);
        if let Some(slash) = without_scheme.find('/') {
            let path = &without_scheme[slash + 1..];
            let path = path.split(['?', '#']).next().unwrap_or(path);
            let first = path.split('/').find(|s| !s.is_empty()).unwrap_or(path);
            let first = first.split(['?', '#', '&']).next().unwrap_or(first).trim();
            if !first.is_empty() && first != "profile.php" {
                return Some(first.to_string());
            }
        }
        return None;
    }
    // Not a URL: treat as raw ID or username
    let norm = normalize_id(trimmed);
    if norm.is_empty() { None } else { Some(norm) }
}

/// Resolve `id` → `username` for Facebook via gallery-dl -K.
/// Uses the site's cookies (file or browser) like instagram resolver.
pub fn resolve_facebook_id_to_username(
    id: &str,
    cookies_file: Option<&Path>,
    cookies_browser: Option<&str>,
) -> Result<String> {
    let id = normalize_id(id);
    if !is_id_like(&id) {
        anyhow::bail!("not an ID-like value");
    }
    resolve_via_gallery_dl(&id, cookies_file, cookies_browser)
}

#[allow(clippy::collapsible_if)]
fn resolve_via_gallery_dl(
    id: &str,
    cookies_file: Option<&Path>,
    cookies_browser: Option<&str>,
) -> Result<String> {
    let exe = crate::application::backend::gallery_dl_executable()
        .context("gallery-dl not available for resolver")?;
    // Try multiple URL forms: profile.php, vanity numeric, m.facebook
    let candidates = [
        format!("https://www.facebook.com/profile.php?id={id}"),
        format!("https://www.facebook.com/{id}"),
        format!("https://m.facebook.com/profile.php?id={id}"),
    ];
    for url in candidates {
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
        args.push(std::ffi::OsString::from(&url));
        let out = match crate::process::Executor::run_capturing(&exe.to_string_lossy(), &args) {
            Ok(o) => o,
            Err(_) => continue,
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines = stdout.lines();
        while let Some(line) = lines.next() {
            // Primary: username
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
            // Fallback for pages: title / name may hold page name when username is empty
            if line.trim() == "title" || line.trim() == "name" {
                if let Some(val) = lines.next() {
                    let v = val.trim();
                    if !v.is_empty() {
                        let sanitized = title_to_username(v);
                        if !sanitized.is_empty() {
                            return Ok(sanitized);
                        }
                    }
                }
            }
            if line.contains("\"username\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(u) = v.get("username").and_then(|s| s.as_str()) {
                        if !u.is_empty() {
                            return Ok(u.to_string());
                        }
                    }
                    // Try title/name inside JSON
                    if let Some(t) = v.get("title").and_then(|s| s.as_str()) {
                        if !t.is_empty() {
                            let sanitized = title_to_username(t);
                            if !sanitized.is_empty() {
                                return Ok(sanitized);
                            }
                        }
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
            // Generic title/name in JSON line
            if line.contains("\"title\"") || line.contains("\"name\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    for key in ["title", "name"] {
                        if let Some(t) = v.get(key).and_then(|s| s.as_str()) {
                            if !t.is_empty() {
                                let sanitized = title_to_username(t);
                                if !sanitized.is_empty() {
                                    return Ok(sanitized);
                                }
                            }
                        }
                    }
                }
            }
        }
        // If we got here, try next candidate URL
    }
    // Final fallback: try graph.facebook.com via curl (works for pages without username)
    if which::which("curl").is_ok() {
        // Try to get cookies for curl if available
        let mut curl_cookie: Option<std::path::PathBuf> = None;
        if let Some(p) = cookies_file {
            curl_cookie = Some(p.to_path_buf());
        } else if let Some(browser) = cookies_browser {
            let tmp_name = format!("__fb_resolve_{}", std::process::id());
            if let Ok((path, _)) = crate::config::cookies::capture_chromium(
                browser,
                &["facebook".to_string()],
                &tmp_name,
            ) {
                curl_cookie = Some(path);
            } else if let Ok((path, _)) =
                crate::config::cookies::capture_firefox(&["facebook".to_string()], &tmp_name)
            {
                curl_cookie = Some(path);
            }
        }
        for base in [
            format!("https://graph.facebook.com/{id}?fields=username,name,title"),
            format!("https://www.facebook.com/{id}"),
        ] {
            let mut cmd = std::process::Command::new("curl");
            cmd.arg("-sL").arg("--max-time").arg("10");
            cmd.arg("-H")
                .arg("User-Agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36");
            cmd.arg("-H").arg("Accept: application/json");
            if let Some(ref c) = curl_cookie {
                cmd.arg("--cookie").arg(c);
            }
            cmd.arg(&base);
            if let Ok(out) = cmd.output() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    for key in ["username", "name", "title"] {
                        if let Some(u) = v.get(key).and_then(|s| s.as_str()) {
                            if !u.is_empty() {
                                let sanitized: String = u
                                    .chars()
                                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                                    .collect();
                                let sanitized = sanitized.trim_matches('_').to_string();
                                if !sanitized.is_empty() {
                                    if let Some(p) = &curl_cookie
                                        && p.to_string_lossy().contains("__fb_resolve_")
                                    {
                                        let _ = std::fs::remove_file(p);
                                    }
                                    return Ok(sanitized);
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(p) = curl_cookie
            && p.to_string_lossy().contains("__fb_resolve_")
        {
            let _ = std::fs::remove_file(p);
        }
    }
    anyhow::bail!("could not resolve Facebook ID via gallery-dl");
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{extract_identifier, is_id_like, is_url_like, normalize_id};

    #[test]
    fn detects_id_like() {
        assert!(is_id_like("123456789012345"));
        assert!(is_id_like("  1234567  "));
        assert!(is_id_like("@123456789012345"));
        assert!(!is_id_like("someuser"));
        assert!(!is_id_like("people"));
        assert!(!is_id_like("12345"));
    }

    #[test]
    fn detects_url_like() {
        assert!(is_url_like("https://www.facebook.com/username"));
        assert!(is_url_like("https://m.facebook.com/profile.php?id=123"));
        assert!(is_url_like("https://fb.com/username"));
        assert!(!is_url_like("1234567890"));
        assert!(!is_url_like("someuser"));
    }

    #[test]
    fn extracts_from_urls() {
        assert_eq!(
            extract_identifier("https://www.facebook.com/profile.php?id=123456789012345&sk=about"),
            Some("123456789012345".to_string())
        );
        assert_eq!(
            extract_identifier("https://www.facebook.com/people/John-Doe/123456789012345/"),
            Some("123456789012345".to_string())
        );
        assert_eq!(
            extract_identifier("https://www.facebook.com/someuser/"),
            Some("someuser".to_string())
        );
        assert_eq!(
            extract_identifier("https://m.facebook.com/someuser/photos"),
            Some("someuser".to_string())
        );
        assert_eq!(
            extract_identifier("123456789012345"),
            Some("123456789012345".to_string())
        );
        assert_eq!(extract_identifier("someuser"), Some("someuser".to_string()));
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_id("@id:1234567890"), "1234567890");
    }
}
