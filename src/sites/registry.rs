//! Central site registry — single source of truth for adding a new network.
//!
//! Adding a new site = one `SiteSpec` entry here + its `ensure_*_site` template
//! in `crate::config::templates`. No other `match site` / `if site=="..."` needed:
//! consumers (`content.rs`, `cookies.rs`, `archive.rs`, `scrape_flow.rs`, etc.)
//! delegate to this registry.
//!
//! Directory contract (canon):
//!   Save Profile: {scrapmf_root} / {category} / {username|user} / <site.toml suffix>
//!   Quick:        {username} / <site.toml suffix>
//! The suffix (posts/reels/highlights/stories, conditional dirs) stays in
//! `sites/*.toml` (`extractor.*.directory`), not here.

use crate::config::{
    ensure_example_sites, ensure_facebook_site, ensure_threads_site, ensure_tiktok_site,
    ensure_twitter_site, ensure_vsco_site,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    GalleryDl,
    Threadstractor,
}

#[derive(Debug, Clone)]
pub struct SiteSpec {
    pub id: &'static str,
    pub display: &'static str,
    /// Hosts as they appear in URLs / cookie domains. First is primary pattern.
    pub domains: &'static [&'static str],
    /// Substrings to auto-match a URL (usually same as domains, but e.g. twitter also matches twitter.com).
    pub patterns: &'static [&'static str],
    pub backend: BackendKind,
    /// Fn that ensures sites/<id>.toml exists (no clobber).
    pub ensure_fn: fn() -> anyhow::Result<()>,
    /// Content menu kinds (first is always "All").
    pub content_kinds: &'static [&'static str],
    /// (ContentKind label, url template with `{username}` placeholder) for `build_tagged_urls`.
    /// Duplicate URL (tiktok Videos/Photos sharing /posts) is intentional — deduped later.
    pub url_templates: &'static [(&'static str, &'static str)],
    /// Username placeholder used in directory templates (informational; templates.rs builds the real dir).
    pub dir_user_placeholder: &'static str,
}

// ─── Registry entries ─────────────────────────────────────────────────────────

pub const INSTAGRAM: SiteSpec = SiteSpec {
    id: "instagram",
    display: "Instagram",
    domains: &["instagram.com"],
    patterns: &["instagram.com"],
    backend: BackendKind::GalleryDl,
    ensure_fn: ensure_example_sites,
    content_kinds: &[
        "All",
        "Posts",
        "Reels",
        "Highlights",
        "Stories",
        "Profile",
        "Instagram_id",
    ],
    url_templates: &[
        ("Posts", "https://www.instagram.com/{username}/"),
        ("Reels", "https://www.instagram.com/{username}/reels/"),
        (
            "Highlights",
            "https://www.instagram.com/{username}/highlights/",
        ),
        ("Stories", "https://www.instagram.com/stories/{username}/"),
        ("Profile", "https://www.instagram.com/{username}/avatar"),
        // Resolves the numeric account id. Not a media kind: gallery-dl's
        // InstagramInfoExtractor emits a Message.Directory, never a file, so a
        // plain scrape of this URL writes zero files and still reports success.
        // `application::instagram_id` handles it instead.
        ("Instagram_id", "https://www.instagram.com/{username}/info/"),
    ],
    dir_user_placeholder: "{username}",
};

pub const TIKTOK: SiteSpec = SiteSpec {
    id: "tiktok",
    display: "TikTok",
    domains: &["tiktok.com"],
    patterns: &["tiktok.com"],
    backend: BackendKind::GalleryDl,
    ensure_fn: ensure_tiktok_site,
    content_kinds: &["All", "Videos", "Photos", "Profile"],
    url_templates: &[
        ("Videos", "https://www.tiktok.com/@{username}/posts"),
        ("Photos", "https://www.tiktok.com/@{username}/posts"),
        ("Stories", "https://www.tiktok.com/@{username}/stories"),
        ("Profile", "https://www.tiktok.com/@{username}/avatar"),
    ],
    dir_user_placeholder: "{user}",
};

pub const TWITTER: SiteSpec = SiteSpec {
    id: "twitter",
    display: "Twitter/X",
    domains: &["twitter.com", "x.com"],
    patterns: &["x.com", "twitter.com"],
    backend: BackendKind::GalleryDl,
    ensure_fn: ensure_twitter_site,
    content_kinds: &["All", "Media", "Profile"],
    url_templates: &[
        ("Media", "https://x.com/{username}/media"),
        ("Profile", "https://x.com/{username}/photo"),
        ("Profile", "https://x.com/{username}/header_photo"),
    ],
    dir_user_placeholder: "{user[name]}",
};

pub const VSCO: SiteSpec = SiteSpec {
    id: "vsco",
    display: "VSCO",
    domains: &["vsco.co"],
    patterns: &["vsco.co"],
    backend: BackendKind::GalleryDl,
    ensure_fn: ensure_vsco_site,
    content_kinds: &["All", "Media", "Profile"],
    url_templates: &[
        ("Media", "https://vsco.co/{username}/gallery"),
        ("Profile", "https://vsco.co/{username}/avatar"),
    ],
    dir_user_placeholder: "{user}",
};

pub const THREADS: SiteSpec = SiteSpec {
    id: "threads",
    display: "Threads",
    domains: &["threads.com", "threads.net"],
    patterns: &["threads.com", "threads.net"],
    backend: BackendKind::Threadstractor,
    ensure_fn: ensure_threads_site,
    content_kinds: &["All", "Photos", "Videos", "Profile"],
    url_templates: &[
        ("Photos", "https://www.threads.com/@{username}"),
        ("Videos", "https://www.threads.com/@{username}"),
        ("Profile", "https://www.threads.com/@{username}"),
    ],
    dir_user_placeholder: "{username}",
};

pub const FACEBOOK: SiteSpec = SiteSpec {
    id: "facebook",
    display: "Facebook",
    domains: &["facebook.com", "fb.com", "m.facebook.com"],
    patterns: &["facebook.com", "fb.com", "m.facebook.com"],
    backend: BackendKind::GalleryDl,
    ensure_fn: ensure_facebook_site,
    content_kinds: &["All", "Posts", "Albums", "Videos"],
    url_templates: &[
        ("Posts", "https://www.facebook.com/{username}/photos"),
        (
            "Albums",
            "https://www.facebook.com/{username}/photos_albums",
        ),
        ("Videos", "https://www.facebook.com/{username}/videos/"),
    ],
    dir_user_placeholder: "{username}",
};

pub const REGISTRY: &[SiteSpec] = &[INSTAGRAM, TIKTOK, TWITTER, VSCO, THREADS, FACEBOOK];

// ─── Lookups ──────────────────────────────────────────────────────────────────

pub fn all_specs() -> &'static [SiteSpec] {
    REGISTRY
}

pub fn find_by_id(id: &str) -> Option<&'static SiteSpec> {
    REGISTRY.iter().find(|s| s.id == id)
}

pub fn find_by_host(host: &str) -> Option<&'static SiteSpec> {
    REGISTRY.iter().find(|s| {
        s.domains
            .iter()
            .any(|d| *d == host || host.ends_with(&format!(".{d}")) || host == *d)
    })
}

pub fn find_by_url(url: &str) -> Option<&'static SiteSpec> {
    let mut best: Option<(&SiteSpec, usize)> = None;
    for spec in REGISTRY {
        for pat in spec.patterns {
            if url.contains(pat) {
                let len = pat.len();
                if best.is_none_or(|(_, l)| len > l) {
                    best = Some((spec, len));
                }
            }
        }
    }
    best.map(|(s, _)| s)
}

pub fn domains_for_site(site_key: &str) -> &'static [&'static str] {
    if let Some(s) = find_by_id(site_key) {
        return s.domains;
    }
    // Aliases kept for backward compat (old site keys / short forms)
    match site_key {
        "x" => find_by_id("twitter").map(|s| s.domains).unwrap_or(&[]),
        "fb" => find_by_id("facebook").map(|s| s.domains).unwrap_or(&[]),
        _ => &[],
    }
}

/// Host-aware account extraction. Returns (site_id, account) or None.
/// Mirrors `crate::application::archive::site_account_from_url` but data-driven.
pub fn site_account_from_host(host: &str, full_path: &str, path: &str) -> Option<(String, String)> {
    // Facebook needs special handling (profile.php?id=, people/...)
    if host == "facebook.com"
        || host == "fb.com"
        || host == "m.facebook.com"
        || host.ends_with(".facebook.com")
    {
        if let Some(pos) = full_path.find("profile.php?id=") {
            let after = &full_path[pos + "profile.php?id=".len()..];
            let id = after.split(['&', '/', '?', '#']).next().unwrap_or(after);
            if !id.is_empty() {
                return Some(("facebook".into(), id.to_string()));
            }
        }
        if full_path.starts_with("people/") {
            let parts: Vec<&str> = full_path.split('/').collect();
            if parts.len() >= 3 {
                let id = parts[2].split(['?', '#', '&']).next().unwrap_or(parts[2]);
                if !id.is_empty() {
                    return Some(("facebook".into(), id.to_string()));
                }
            }
            if parts.len() >= 2 {
                let id = parts[1].split(['?', '#', '&']).next().unwrap_or(parts[1]);
                if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                    return Some(("facebook".into(), id.to_string()));
                }
            }
        }
        let seg = |i: usize| path.split('/').nth(i).filter(|s| !s.is_empty());
        return seg(0).map(|u| ("facebook".into(), u.to_string()));
    }
    let seg = |i: usize| path.split('/').nth(i).filter(|s| !s.is_empty());
    match host {
        "instagram.com" => seg(0).map(|u| ("instagram".into(), u.trim_end_matches('/').into())),
        "tiktok.com" => seg(0)
            .and_then(|u| u.strip_prefix('@'))
            .map(|u| ("tiktok".into(), u.into())),
        "twitter.com" | "x.com" => seg(0)
            .filter(|u| !matches!(*u, "i" | "home" | "explore" | "search"))
            .map(|u| ("twitter".into(), u.into())),
        "vsco.co" => seg(0).map(|u| ("vsco".into(), u.into())),
        "threads.com" | "threads.net" => seg(0)
            .and_then(|u| u.strip_prefix('@'))
            .map(|u| ("threads".into(), u.into())),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn find_by_id_known() {
        assert_eq!(find_by_id("instagram").unwrap().id, "instagram");
        assert!(find_by_id("unknown").is_none());
    }

    #[test]
    fn find_by_url_longest_wins() {
        assert_eq!(
            find_by_url("https://www.tiktok.com/@user/video/123")
                .unwrap()
                .id,
            "tiktok"
        );
        assert_eq!(
            find_by_url("https://x.com/user/media").unwrap().id,
            "twitter"
        );
        assert!(find_by_url("https://example.com/a").is_none());
    }

    #[test]
    fn domains_lookup() {
        assert_eq!(domains_for_site("tiktok"), &["tiktok.com"]);
        assert!(domains_for_site("nope").is_empty());
    }
}
