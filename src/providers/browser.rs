use std::path::PathBuf;

use crate::browsers::{self, BrowserChannel, CookieDb};

#[derive(Debug, Clone)]
pub struct BrowserInfo {
    pub id: &'static str,
    pub display: String,
    pub cookie_db: Option<PathBuf>,
    pub profile: Option<String>,
    pub available: bool,
}

/// Filename holding the cookie database for a channel.
fn cookie_db_name(ch: &BrowserChannel) -> &'static str {
    if ch.id == "firefox" {
        "cookies.sqlite"
    } else {
        "Cookies"
    }
}

/// Detect available browsers for `--cookies-from-browser`.
///
/// Path resolution is delegated entirely to [`crate::browsers`] so this and
/// the cookie-capture wizard can never disagree about where a browser's
/// cookies live. The previous version kept its own copy of the channel table
/// and had already drifted — it knew about Brave Origin while the capture path
/// did not.
pub fn detect_available_browsers() -> Vec<BrowserInfo> {
    browsers::CHANNELS
        .iter()
        .map(|ch| {
            let bin = browsers::binary_for(ch);
            let found = browsers::find_cookie_db(ch, cookie_db_name(ch));
            let available = bin.is_some() && found.is_some();
            let profile = found
                .as_ref()
                .map(|db: &CookieDb| db.profile.clone())
                .filter(|p| !p.is_empty());
            let display = match found.as_ref() {
                Some(db) if available => format!(
                    "{}{} — {}",
                    ch.display,
                    profile
                        .as_ref()
                        .map(|p| format!(" ({p})"))
                        .unwrap_or_default(),
                    cookie_db_summary(&db.path)
                ),
                Some(db) => format!(
                    "{} — {} ({}, not installed)",
                    ch.display,
                    cookie_db_summary(&db.path),
                    db.origin
                ),
                None => format!("{} — no cookie DB", ch.display),
            };
            BrowserInfo {
                id: ch.id,
                display,
                cookie_db: found.map(|db| db.path),
                profile,
                available,
            }
        })
        .collect()
}

/// Factual one-line summary of a detected cookie DB. Never guesses the
/// cookie count (that would require decrypting the DB) — reports size instead.
fn cookie_db_summary(db: &std::path::Path) -> String {
    match std::fs::metadata(db) {
        Ok(md) if md.len() > 0 => {
            let kb = md.len() / 1024;
            format!("cookie DB ({kb} KB)")
        }
        _ => "empty cookie DB".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::detect_available_browsers;
    use crate::browsers;

    #[test]
    fn detect_runs_without_panic() {
        let browsers = detect_available_browsers();
        // Should always have at least firefox entry
        assert!(!browsers.is_empty());
        assert!(browsers.iter().any(|b| b.id == "firefox"));
    }

    #[test]
    fn available_if_binary_and_db() {
        let browsers = detect_available_browsers();
        for b in browsers {
            if b.available {
                assert!(b.cookie_db.is_some());
            }
        }
    }

    /// The regression behind this refactor: `doctor` learned about Brave
    /// Origin while the capture path did not, so a user on Origin was told one
    /// thing by `doctor` and another by the wizard. Both now read the same
    /// registry, so the two views cannot disagree.
    #[test]
    fn detection_and_capture_see_the_same_channels() {
        let detected: Vec<&str> = detect_available_browsers()
            .into_iter()
            .map(|b| b.id)
            .collect();
        for ch in browsers::CHANNELS {
            assert!(
                detected.contains(&ch.id),
                "channel {} missing from detection",
                ch.id
            );
        }
    }

    #[test]
    fn brave_origin_channel_is_reported_by_detection() {
        let detected = detect_available_browsers();
        assert!(
            detected.iter().any(|b| b.id == "brave-origin"),
            "Brave Origin must appear in --cookies-from-browser detection"
        );
    }
}
