use crate::cli::interactive::menu;

/// Content types selectable per account in Scrape as Profile.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum ContentKind {
    Posts,
    Videos,
    Photos,
    Media,
    Reels,
    Highlights,
    Stories,
    Profile,
    Albums,
    /// Numeric account id, written as `Instagram_id.txt` at the account root.
    /// Not media — gallery-dl resolves it from the profile's `/info/` view.
    InstagramId,
}

impl ContentKind {
    pub(super) fn label(&self) -> &'static str {
        match self {
            ContentKind::Posts => "Posts",
            ContentKind::Videos => "Videos",
            ContentKind::Photos => "Photos",
            ContentKind::Media => "Media",
            ContentKind::Reels => "Reels",
            ContentKind::Highlights => "Highlights",
            ContentKind::Stories => "Stories",
            ContentKind::Profile => "Profile",
            ContentKind::Albums => "Albums",
            ContentKind::InstagramId => "Instagram_id",
        }
    }

    pub(super) fn from_label(s: &str) -> Option<Self> {
        match s {
            "Posts" => Some(ContentKind::Posts),
            "Videos" => Some(ContentKind::Videos),
            "Photos" => Some(ContentKind::Photos),
            "Media" => Some(ContentKind::Media),
            "Reels" => Some(ContentKind::Reels),
            "Highlights" => Some(ContentKind::Highlights),
            "Stories" => Some(ContentKind::Stories),
            "Profile" => Some(ContentKind::Profile),
            "Albums" => Some(ContentKind::Albums),
            "Instagram_id" => Some(ContentKind::InstagramId),
            _ => None,
        }
    }
}

/// Site-aware content menu options. First entry is always the shortcut label "All".
/// Delegates to the central `crate::sites` registry — adding a site = one registry entry.
pub(super) fn content_options(site: &str) -> Vec<&'static str> {
    if let Some(spec) = crate::sites::registry::find_by_id(site) {
        return spec.content_kinds.to_vec();
    }
    vec!["All", "Posts"]
}

/// Resolve MultiSelect labels to ContentKinds for a site.
/// Selecting "All" expands to every real kind of the site's menu.
/// An empty `picked` (canceled / nothing selected) stays empty — the
/// caller treats that as "skip this account".
pub(super) fn resolve_kinds(site: &str, picked: &[String]) -> Vec<ContentKind> {
    if picked.iter().any(|l| l == "All") {
        return content_options(site)
            .iter()
            .filter_map(|l| ContentKind::from_label(l))
            .collect();
    }
    picked
        .iter()
        .filter_map(|l| ContentKind::from_label(l))
        .collect()
}

/// Remove network/account segments from directory templates so the tree is
/// rooted at {scrapmf_root} (= username for quick scrapes):
///   [root, category, username, subcategory]  →  [root, subcategory]
///   [root, category, user[name], videos]     →  [root, videos]
/// All other segments (highlights grouping, date folders, ids) are preserved.
/// Directory template segments that identify the network/account. Quick
/// scrape strips these so files land under `<root>/<content-type>/`.
pub(super) const IDENTITY_SEGMENTS: &[&str] =
    &["{category}", "{username}", "{user}", "{user[name]}"];

pub(super) fn is_identity_segment(s: &str) -> bool {
    IDENTITY_SEGMENTS.contains(&s)
}

/// Whether a site offers a content-type menu (multiple content kinds).
/// Generic sites only have Posts — no menu, auto [Posts].
/// Registry-driven: any known site has a menu.
pub(super) fn site_has_content_menu(site: &str) -> bool {
    crate::sites::registry::find_by_id(site).is_some()
}

/// The "apply same selection to all" shortcut only applies when:
/// - 2+ accounts are selected, AND
/// - all of them belong to the same site, AND
/// - that site has a content menu (instagram).
pub(super) fn shortcut_applicable(selected: &[(String, String, crate::config::Account)]) -> bool {
    if selected.len() < 2 {
        return false;
    }
    let first_site = &selected[0].0;
    selected.iter().all(|(s, _, _)| s == first_site) && site_has_content_menu(first_site)
}

pub(super) fn build_tagged_urls(site: &str, username: &str) -> Vec<(ContentKind, String)> {
    if let Some(spec) = crate::sites::registry::find_by_id(site) {
        let mut out = Vec::with_capacity(spec.url_templates.len());
        for (label, tmpl) in spec.url_templates {
            let kind = ContentKind::from_label(label).unwrap_or(ContentKind::Posts);
            let url = tmpl.replace("{username}", username);
            out.push((kind, url));
        }
        if !out.is_empty() {
            return out;
        }
    }
    vec![(
        ContentKind::Posts,
        format!("https://www.{site}.com/{username}/"),
    )]
}

/// Filter tagged URLs by selected kinds. Returns (primary url, extra urls).
/// Duplicate URLs (e.g. tiktok Videos+Photos sharing /posts) are deduped.
pub(super) fn select_urls(
    tagged: &[(ContentKind, String)],
    kinds: &[ContentKind],
) -> Option<(String, Vec<String>)> {
    let chosen: Vec<&(ContentKind, String)> = if kinds.is_empty() || tagged.is_empty() {
        return None;
    } else {
        tagged.iter().filter(|(k, _)| kinds.contains(k)).collect()
    };
    if chosen.is_empty() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut unique = chosen.into_iter().filter(|(_, u)| seen.insert(u.clone()));
    let first = unique.next()?;
    let url = first.1.clone();
    let extra: Vec<String> = unique.map(|(_, u)| u.clone()).collect();
    Some((url, extra))
}

/// Build unique `{site}:{username}` labels for a list of accounts.
/// Falls back to display_name, then "(no username)", appending "#N" on collision.
pub(super) fn account_labels(site: &str, accounts: &[crate::config::Account]) -> Vec<String> {
    let mut labels: Vec<String> = Vec::with_capacity(accounts.len());
    for acc in accounts {
        let base = acc
            .username
            .clone()
            .or_else(|| acc.display_name.clone())
            .unwrap_or_else(|| "(no username)".to_string());
        let mut label = format!("{site}:{base}");
        let mut n = 2;
        while labels.contains(&label) {
            label = format!("{site}:{base}#{n}");
            n += 1;
        }
        labels.push(label);
    }
    labels
}

/// Reject combining the "All" shortcut with specific content types.
///
/// inquire's MultiSelect cannot disable options dynamically while "All" is
/// checked, so the rule is enforced at submit time through a validator:
/// the user must deselect either "All" or the rest before Enter works.
pub(super) fn validate_kind_selection(picked: &[String]) -> Result<(), String> {
    if picked.len() > 1 && picked.iter().any(|l| l == "All") {
        return Err(
            "'All' can't be combined with specific types — deselect 'All' or the rest".to_string(),
        );
    }
    Ok(())
}

/// Human description of a content selection ("all content" or a
/// comma-separated lowercase list).
pub(super) fn kinds_description(site: &str, kinds: &[ContentKind]) -> String {
    let all_count = content_options(site).len() - 1; // minus "All"
    if kinds.len() == all_count {
        "all content".to_string()
    } else {
        kinds
            .iter()
            .map(|k| k.label().to_lowercase())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(super) fn prompt_content_kinds(site: &str, label: &str) -> menu::Step<Vec<ContentKind>> {
    let opts: Vec<(String, Vec<String>)> = content_options(site)
        .into_iter()
        .map(|k| {
            let details = match k {
                "All" => vec!["select all kinds".to_string()],
                "Posts" => vec!["feed posts".to_string()],
                "Reels" => vec!["short videos".to_string()],
                "Highlights" => vec!["saved highlights".to_string()],
                "Stories" => vec!["24h stories".to_string()],
                "Profile" => vec!["avatar".to_string()],
                "Videos" => vec!["videos".to_string()],
                "Photos" => vec!["photos".to_string()],
                "Media" => vec!["photos + videos".to_string()],
                "Albums" => vec!["albums".to_string()],
                _ => Vec::new(),
            };
            (k.to_string(), details)
        })
        .collect();
    let context = format!("Content for {label}");
    // Esc steps back to the previous question instead of reading as
    // "nothing selected", which is what made a mis-typed username look like a
    // cancelled run.
    let idxs = match menu::pick_multi_back(&context, opts.clone(), &[]) {
        menu::Step::Value(v) => v,
        menu::Step::Back => return menu::Step::Back,
        menu::Step::Cancel => return menu::Step::Cancel,
    };
    let picked: Vec<String> = idxs
        .into_iter()
        .filter_map(|i| opts.get(i).map(|(l, _)| l.clone()))
        .collect();
    // Only reachable without a TTY: in the app `Enter` on an unchecked list
    // picks the highlighted row, so a confirmation always carries something.
    if picked.is_empty() {
        return menu::Step::Value(Vec::new());
    }
    if let Err(msg) = validate_kind_selection(&picked) {
        eprintln!("{msg}");
        std::thread::sleep(std::time::Duration::from_millis(800));
        return menu::Step::Value(Vec::new());
    }
    tracing::debug!(site = %site, picked = ?picked, "content kinds selected");
    menu::Step::Value(resolve_kinds(site, &picked))
}

/// Preview + Confirm + sequential execution of built jobs.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        ContentKind, account_labels, build_tagged_urls, content_options, resolve_kinds,
        select_urls, shortcut_applicable, validate_kind_selection,
    };
    use crate::config::Account;

    #[test]
    fn validator_rejects_all_combined_with_specifics() {
        let picked = vec!["All".to_string(), "Posts".to_string()];
        assert!(
            validate_kind_selection(&picked).is_err(),
            "All + Posts must be rejected"
        );
    }

    #[test]
    fn validator_accepts_all_alone_and_specific_combos() {
        assert!(validate_kind_selection(&["All".to_string()]).is_ok());
        let picked = vec!["Posts".to_string(), "Reels".to_string()];
        assert!(validate_kind_selection(&picked).is_ok());
        // empty selection is allowed (caller treats it as skip)
        assert!(validate_kind_selection(&[]).is_ok());
    }

    fn tagged() -> Vec<(ContentKind, String)> {
        vec![
            (ContentKind::Posts, "posts_url".to_string()),
            (ContentKind::Reels, "reels_url".to_string()),
            (ContentKind::Highlights, "highlights_url".to_string()),
            (ContentKind::Stories, "stories_url".to_string()),
        ]
    }

    #[test]
    fn select_urls_all_kinds() {
        let kinds = vec![
            ContentKind::Posts,
            ContentKind::Reels,
            ContentKind::Highlights,
            ContentKind::Stories,
        ];
        let (url, extra) = select_urls(&tagged(), &kinds).unwrap();
        assert_eq!(url, "posts_url");
        assert_eq!(extra, vec!["reels_url", "highlights_url", "stories_url"]);
    }

    #[test]
    fn select_urls_only_stories() {
        let (url, extra) = select_urls(&tagged(), &[ContentKind::Stories]).unwrap();
        assert_eq!(url, "stories_url");
        assert!(extra.is_empty());
    }

    #[test]
    fn select_urls_posts_and_highlights() {
        let kinds = vec![ContentKind::Posts, ContentKind::Highlights];
        let (url, extra) = select_urls(&tagged(), &kinds).unwrap();
        assert_eq!(url, "posts_url");
        assert_eq!(extra, vec!["highlights_url"]);
    }

    #[test]
    fn select_urls_empty_returns_none() {
        assert!(select_urls(&tagged(), &[]).is_none());
    }

    #[test]
    fn select_urls_no_match_returns_none() {
        let empty: Vec<(ContentKind, String)> = Vec::new();
        let kinds = vec![ContentKind::Posts];
        assert!(select_urls(&empty, &kinds).is_none());
    }

    #[test]
    fn content_kind_label_roundtrip() {
        for k in [
            ContentKind::Posts,
            ContentKind::Reels,
            ContentKind::Highlights,
            ContentKind::Stories,
            ContentKind::InstagramId,
        ] {
            assert_eq!(ContentKind::from_label(k.label()), Some(k));
        }
        assert_eq!(ContentKind::from_label("All"), None);
    }

    #[test]
    fn account_labels_unique_usernames() {
        let accounts = vec![
            Account {
                username: Some("user1".to_string()),
                ..Default::default()
            },
            Account {
                username: Some("user2".to_string()),
                ..Default::default()
            },
        ];
        assert_eq!(
            account_labels("instagram", &accounts),
            vec!["instagram:user1", "instagram:user2"]
        );
    }

    #[test]
    fn account_labels_collision_gets_suffix() {
        let accounts = vec![
            Account {
                username: Some("same".to_string()),
                ..Default::default()
            },
            Account {
                username: Some("same".to_string()),
                ..Default::default()
            },
        ];
        assert_eq!(
            account_labels("instagram", &accounts),
            vec!["instagram:same", "instagram:same#2"]
        );
    }

    #[test]
    fn account_labels_missing_username_uses_display_name_then_placeholder() {
        let accounts = vec![Account {
            display_name: Some("Alt IG".to_string()),
            ..Default::default()
        }];
        assert_eq!(
            account_labels("instagram", &accounts),
            vec!["instagram:Alt IG"]
        );
        let no_name = vec![Account::default()];
        assert_eq!(
            account_labels("instagram", &no_name),
            vec!["instagram:(no username)"]
        );
    }

    fn sel(sites: &[&str]) -> Vec<(String, String, Account)> {
        sites
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    s.to_string(),
                    format!("{s}:user{i}"),
                    Account {
                        username: Some(format!("user{i}")),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    #[test]
    fn shortcut_not_applicable_for_single_account() {
        assert!(!shortcut_applicable(&sel(&["instagram"])));
    }

    #[test]
    fn shortcut_applicable_for_same_site_multiple() {
        assert!(shortcut_applicable(&sel(&["instagram", "instagram"])));
        // tiktok now has a content menu too
        assert!(shortcut_applicable(&sel(&["tiktok", "tiktok"])));
    }

    #[test]
    fn shortcut_not_applicable_for_mixed_sites() {
        assert!(!shortcut_applicable(&sel(&["instagram", "tiktok"])));
        assert!(!shortcut_applicable(&sel(&["tiktok", "unknownsite"])));
    }

    #[test]
    fn shortcut_not_applicable_for_non_menu_site() {
        assert!(!shortcut_applicable(&sel(&["unknownsite", "unknownsite"])));
    }

    #[test]
    fn content_kind_profile_roundtrip() {
        assert_eq!(ContentKind::Profile.label(), "Profile");
        assert_eq!(
            ContentKind::from_label("Profile"),
            Some(ContentKind::Profile)
        );
    }

    #[test]
    fn content_options_per_site() {
        assert_eq!(
            content_options("instagram"),
            vec![
                "All",
                "Posts",
                "Reels",
                "Highlights",
                "Stories",
                "Profile",
                "Instagram_id"
            ]
        );
        assert_eq!(
            content_options("tiktok"),
            // Stories hidden — gallery-dl upstream limitation, not fixed by the
            // 1.32.9 -> 1.32.17 bumps. Re-checked against v1.32.17: the full set
            // of `Tiktok*Extractor` class names is byte-identical to v1.32.16,
            // so `TiktokStoriesExtractor` still does not exist.
            vec!["All", "Videos", "Photos", "Profile"]
        );
        assert_eq!(content_options("twitter"), vec!["All", "Media", "Profile"]);
        assert_eq!(content_options("vsco"), vec!["All", "Media", "Profile"]);
        assert_eq!(
            content_options("threads"),
            vec!["All", "Photos", "Videos", "Profile"]
        );
        assert_eq!(content_options("unknown"), vec!["All", "Posts"]);
    }

    #[test]
    fn tagged_urls_vsco_gallery_and_avatar() {
        let urls = build_tagged_urls("vsco", "someuser");
        let kinds: Vec<ContentKind> = urls.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, vec![ContentKind::Media, ContentKind::Profile]);
        assert_eq!(urls[0].1, "https://vsco.co/someuser/gallery");
        assert_eq!(urls[1].1, "https://vsco.co/someuser/avatar");
    }

    #[test]
    fn tagged_urls_twitter_media_and_dual_profile() {
        let urls = build_tagged_urls("twitter", "someuser");
        let kinds: Vec<ContentKind> = urls.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                ContentKind::Media,
                ContentKind::Profile,
                ContentKind::Profile
            ]
        );
        assert_eq!(urls[0].1, "https://x.com/someuser/media");
        // avatar and header are distinct URLs, both under Profile kind
        assert_ne!(urls[1].1, urls[2].1);
        assert!(urls[1].1.ends_with("/photo"));
        assert!(urls[2].1.ends_with("/header_photo"));
    }

    #[test]
    fn select_urls_twitter_profile_keeps_both_distinct_urls() {
        let tagged = build_tagged_urls("twitter", "someuser");
        let (url, extra) = select_urls(&tagged, &[ContentKind::Profile]).expect("urls");
        assert!(url.ends_with("/photo") || url.ends_with("/header_photo"));
        assert_eq!(extra.len(), 1); // the other distinct URL kept — no dedup loss
    }

    #[test]
    fn resolve_kinds_all_expands_to_every_site_kind() {
        let picked = vec!["All".to_string()];
        let ig = resolve_kinds("instagram", &picked);
        assert_eq!(
            ig,
            vec![
                ContentKind::Posts,
                ContentKind::Reels,
                ContentKind::Highlights,
                ContentKind::Stories,
                ContentKind::Profile,
                ContentKind::InstagramId
            ],
            "\"All\" must include the id kind — that is the whole point of it being \
             listed: a plain scrape is where people most want the id"
        );
        let tt = resolve_kinds("tiktok", &picked);
        assert_eq!(
            tt,
            // Stories hidden from the tiktok menu (upstream limitation)
            vec![
                ContentKind::Videos,
                ContentKind::Photos,
                ContentKind::Profile
            ]
        );
    }

    #[test]
    fn resolve_kinds_all_mixed_with_others_expands_anyway() {
        // "All" wins even if combined with explicit kinds — no duplicates.
        // TikTok expands to 3 kinds now (Stories hidden).
        let picked = vec!["All".to_string(), "Videos".to_string()];
        let kinds = resolve_kinds("tiktok", &picked);
        assert_eq!(kinds.len(), 3);
        assert!(kinds.contains(&ContentKind::Profile));
        assert!(!kinds.contains(&ContentKind::Stories));
    }

    #[test]
    fn resolve_kinds_partial_selection() {
        let picked = vec!["Posts".to_string(), "Stories".to_string()];
        let kinds = resolve_kinds("instagram", &picked);
        assert_eq!(kinds, vec![ContentKind::Posts, ContentKind::Stories]);
    }

    #[test]
    fn resolve_kinds_empty_stays_empty_skip_semantics() {
        let picked: Vec<String> = Vec::new();
        assert!(resolve_kinds("instagram", &picked).is_empty());
    }

    #[test]
    fn resolve_kinds_unknown_labels_filtered() {
        let picked = vec!["Bogus".to_string(), "Reels".to_string()];
        assert_eq!(
            resolve_kinds("instagram", &picked),
            vec![ContentKind::Reels]
        );
    }

    #[test]
    fn tagged_urls_tiktok_real_extractor_paths() {
        let urls = build_tagged_urls("tiktok", "someuser");
        let kinds: Vec<ContentKind> = urls.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                ContentKind::Videos,
                ContentKind::Photos,
                ContentKind::Stories,
                ContentKind::Profile
            ]
        );
        // Videos and Photos share the same posts URL
        assert_eq!(urls[0].1, "https://www.tiktok.com/@someuser/posts");
        assert_eq!(urls[1].1, urls[0].1);
        assert_eq!(urls[2].1, "https://www.tiktok.com/@someuser/stories");
        assert_eq!(urls[3].1, "https://www.tiktok.com/@someuser/avatar");
    }

    #[test]
    fn select_urls_dedupes_shared_tiktok_posts_url() {
        let tagged = build_tagged_urls("tiktok", "someuser");
        let kinds = vec![ContentKind::Videos, ContentKind::Photos];
        let (url, extra) = select_urls(&tagged, &kinds).expect("urls");
        // both map to /posts — must appear only once; stories/profile not selected
        assert_eq!(url, "https://www.tiktok.com/@someuser/posts");
        assert!(extra.is_empty());
    }

    #[test]
    fn select_urls_single_kind_keeps_primary_only() {
        let tagged = build_tagged_urls("tiktok", "someuser");
        let (url, extra) = select_urls(&tagged, &[ContentKind::Photos]).expect("urls");
        assert_eq!(url, "https://www.tiktok.com/@someuser/posts");
        assert!(extra.is_empty());
    }

    #[test]
    fn generated_profile_roundtrip_multi_site() {
        // Simulates what prompt_new_profile_accounts builds for 2 sites.
        // Cookies are NOT set on accounts — inherited from sites/*.toml via precedence.
        let mut accounts = std::collections::HashMap::new();
        accounts.insert(
            "instagram".to_string(),
            vec![crate::config::Account {
                username: Some("example_user".to_string()),
                ..Default::default()
            }],
        );
        accounts.insert(
            "tiktok".to_string(),
            vec![crate::config::Account {
                username: Some("tt_user".to_string()),
                ..Default::default()
            }],
        );
        let profile = crate::config::Profile {
            profile: Some("example".to_string()),
            display_name: Some("example".to_string()),
            sites: Vec::new(),
            accounts,
            output_dir: None,
            cookies: None,
            cookies_from_browser: None,
            cookie_profile: None,
            overrides: std::collections::HashMap::new(),
        };
        let body = toml::to_string_pretty(&profile).expect("serialize");
        let parsed: crate::config::Profile = toml::from_str(&body).expect("parse back");
        assert_eq!(parsed.profile.as_deref(), Some("example"));
        assert_eq!(parsed.accounts.len(), 2);
        assert_eq!(
            parsed.accounts["instagram"][0].username.as_deref(),
            Some("example_user")
        );
        // no cookie field written — site config is the source of truth
        assert!(
            parsed.accounts["instagram"][0]
                .cookies_from_browser
                .is_none()
        );
        assert!(!body.contains("cookies_from_browser"));
        // no leftover template placeholders anywhere
        assert!(!body.contains("{scarpmf_root}"));
        assert!(!body.contains("CHANGE_ME"));
    }

    #[test]
    fn write_profile_file_creates_valid_toml() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = tmp.path().join("test_person.toml");
        let profile = crate::config::Profile {
            profile: Some("test_person".to_string()),
            display_name: None,
            sites: Vec::new(),
            accounts: std::collections::HashMap::new(),
            output_dir: None,
            cookies: None,
            cookies_from_browser: None,
            cookie_profile: None,
            overrides: std::collections::HashMap::new(),
        };
        crate::config::write_profile_file(&path, &profile).expect("write");
        let s = std::fs::read_to_string(&path).expect("read");
        let parsed: crate::config::Profile = toml::from_str(&s).expect("valid toml");
        assert_eq!(parsed.profile.as_deref(), Some("test_person"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}

#[cfg(test)]
mod strip_tests {
    // Con root neutro, flatten_quick_dirs reproduce el comportamiento del
    // antiguo strip_network_segments (mantiene {scrapmf_root}, quita red).
    use crate::cli::interactive::scrape_flow::flatten_quick_dirs;

    fn strip_network_segments(dirs: Vec<String>) -> Vec<String> {
        flatten_quick_dirs(dirs, "{scrapmf_root}")
    }

    #[test]
    fn strips_instagram_global_template() {
        let dirs = vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{username}".to_string(),
            "{subcategory}".to_string(),
        ];
        assert_eq!(
            strip_network_segments(dirs),
            vec!["{scrapmf_root}", "{subcategory}"]
        );
    }

    #[test]
    fn keeps_highlight_grouping() {
        let dirs = vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{username}".to_string(),
            "highlights".to_string(),
            "{post_id}{highlight_title:?_//}".to_string(),
        ];
        assert_eq!(
            strip_network_segments(dirs),
            vec![
                "{scrapmf_root}",
                "highlights",
                "{post_id}{highlight_title:?_//}"
            ]
        );
    }

    #[test]
    fn keeps_story_date_folders() {
        let dirs = vec![
            "{scrapmf_root}".to_string(),
            "{username}".to_string(),
            "stories".to_string(),
            "\\fF {date.strftime(\"%Y\")}".to_string(),
            "\\fF {date.strftime(\"%m-%B\").lower()}".to_string(),
        ];
        assert_eq!(strip_network_segments(dirs).len(), 4);
    }

    #[test]
    fn strips_twitter_user_name_variant() {
        let dirs = vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{user[name]}".to_string(),
            "videos".to_string(),
        ];
        assert_eq!(
            strip_network_segments(dirs),
            vec!["{scrapmf_root}", "videos"]
        );
    }

    #[test]
    fn plain_word_segments_are_kept() {
        // e.g. a literal folder named "highlights" must survive
        let dirs = vec!["{scrapmf_root}".to_string(), "highlights".to_string()];
        assert_eq!(
            strip_network_segments(dirs),
            vec!["{scrapmf_root}", "highlights"]
        );
    }
}
