//! One-time config migrations for sites/*.toml evolution.
use std::path::Path;

use anyhow::Context;

use super::fs::write_config_file;
use super::templates::tiktok_posts_conditional_directory;
use super::{RateLimit, Site, sites_dir};

/// Parse a site file, or `None` when it cannot be parsed or carries no identity.
///
/// Every migration that rewrites `sites/*.toml` MUST go through this before
/// touching the file. A migration that rebuilds from a failed parse produces a
/// `Site::default()`, and writing that over the user's file silently destroys
/// every setting in it — `write_atomic` renames, so there is no partial write
/// and no copy of the original left behind.
///
/// "No identity" means `site`, `pattern` and `extractor` are all absent: such a
/// file is empty or corrupt, and a default parse of it looks deceptively valid.
///
/// `label` identifies the caller in the warning, so the log says which
/// migration declined the file rather than a bare parse error.
fn parse_site(path: &Path, raw: &str, label: &'static str) -> Option<Site> {
    let site: Site = match toml::from_str(raw) {
        Ok(site) => site,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                label,
                "site unparseable — skipping migration (rebuild from a failed parse would erase this file's settings)"
            );
            return None;
        }
    };
    if site.site.is_none() && site.pattern.is_none() && site.extractor.is_empty() {
        tracing::warn!(
            path = %path.display(),
            label,
            "site has no identity (site/pattern/extractor all absent) — skipping migration"
        );
        return None;
    }
    Some(site)
}

pub(super) fn migrate_highlights_if_needed(path: &Path) -> anyhow::Result<bool> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    // SAFETY: never rewrite empty/corrupt files — an empty parse yields a
    // default Site which would wipe user config on rebuild.
    if raw.trim().is_empty() {
        return Ok(false);
    }
    // Parse once, here. A file that will not parse is never rewritten: the old
    // code fell back to a raw-text heuristic in that case, and then rebuilt
    // from `toml::from_str(&raw).unwrap_or_default()` — the same parser on the
    // same input, so that parse failed too and the user's file was overwritten
    // with an empty Site. The heuristic branch had no success case at all.
    let Some(mut site) = parse_site(path, &raw, "highlights") else {
        return Ok(false);
    };
    // Determine needs via parsed extractor table only (raw global templates like {post_id}_{date} are for posts/reels, not highlights)
    let mut needs = false;
    // Assigned once the site is confirmed to be an instagram site; declared
    // without a value because the old raw-text seed was only ever read on the
    // parse-failure path, which no longer exists.
    let has_highlights;
    {
        // This migration ONLY applies to instagram sites — never touch other sites.
        let is_instagram = site.site.as_deref() == Some("instagram")
            || site
                .pattern
                .as_ref()
                .is_some_and(|p| p.contains("instagram"))
            || site.extractor.keys().any(|k| k.starts_with("instagram"));
        if !is_instagram {
            return Ok(false);
        }
        has_highlights = site.extractor.contains_key("instagram:highlights");
        // Stories override missing entirely → needs migration (add stories support)
        if !site.extractor.contains_key("instagram:stories") {
            needs = true;
        }
        // Avatar override missing → needs migration (add profile pic support)
        if !site.extractor.contains_key("instagram:avatar") {
            needs = true;
        }
        // Avatar filename with {_now} date → migrate to {username}_{media_id}
        if let Some(toml::Value::String(fname)) = site
            .extractor
            .get("instagram:avatar")
            .and_then(|t| t.get("filename"))
            && fname.contains("{_now")
        {
            needs = true;
        }
        if let Some(tbl) = site.extractor.get("instagram:highlights") {
            if let toml::Value::Table(map) = tbl {
                if let Some(toml::Value::String(fname)) = map.get("filename") {
                    // Correct filename is exactly {media_id}.{extension} — any date/num/post_id is old
                    if fname.trim() != "{media_id}.{extension}" {
                        needs = true;
                    }
                } else {
                    needs = true;
                }
                if let Some(toml::Value::Array(arr)) = map.get("directory") {
                    let dir_str = arr
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    if dir_str.contains("{date}") || dir_str.contains("{num}") {
                        needs = true;
                    }
                    // Correct directory must be ["{scrapmf_root}","{category}","{username}","highlights","{post_id}{highlight_title:?_//}"] (inside instagram, same level as posts)
                    let has_highlight_subfolder =
                        dir_str.contains("post_id") && dir_str.contains("highlight_title");
                    if !has_highlight_subfolder {
                        needs = true;
                    }
                    // Must contain {category} to be inside instagram (username/instagram/highlights/...)
                    if !dir_str.contains("{category}") {
                        needs = true;
                    }
                    // If it still uses highlight_id (old alias), migrate to post_id
                    if dir_str.contains("highlight_id") {
                        needs = true;
                    }
                    // Canonical title segment: RAW highlight name with a
                    // conditional separator ({title:?_//} — two slashes, else
                    // gallery-dl raises DirectoryFormatError). Emoji-only
                    // names ("❤️‍🩹", "..") stay visible per user preference;
                    // empty titles collapse to just {post_id}. ANY older
                    // generation — plain "{post_id}_{highlight_title}", the
                    // broken one-slash spec (!g:?_/}), or the retired
                    // slugified form (!g:?_//}) — must re-migrate; exact
                    // match on the final element keeps this robust.
                    if map
                        .get("directory")
                        .and_then(|v| v.as_array())
                        .is_none_or(|arr| {
                            arr.last().and_then(|v| v.as_str())
                                != Some("{post_id}{highlight_title:?_//}")
                        })
                    {
                        needs = true;
                    }
                    // Must be under highlights subfolder
                    if !dir_str.contains("highlights") {
                        needs = true;
                    }
                    // Must be 5 elements (scrapmf_root, category, username, highlights, post_id_title)
                    if let Some(toml::Value::Array(arr)) = map.get("directory")
                        && (arr.len() != 5
                            || arr.first().and_then(|v| v.as_str()) != Some("{scrapmf_root}"))
                    {
                        needs = true;
                    }
                } else {
                    needs = true;
                }
                // If already correct, ensure no false positive
                if !needs {
                    let dir_ok =
                        map.get("directory")
                            .and_then(|v| v.as_array())
                            .is_some_and(|arr| {
                                let s = arr
                                    .iter()
                                    .map(|v| v.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",");
                                s.contains("post_id")
                                    && s.contains("highlight_title")
                                    && !s.contains("!g")
                                    && arr.last().and_then(|v| v.as_str())
                                        == Some("{post_id}{highlight_title:?_//}")
                                    && s.contains("highlights")
                                    && s.contains("{category}")
                                    && s.contains("{username}")
                                    && s.contains("{scrapmf_root}")
                                    && arr.first().and_then(|v| v.as_str())
                                        == Some("{scrapmf_root}")
                                    && arr.len() == 5
                                    && !s.contains("{date}")
                                    && !s.contains("highlight_id")
                            });
                    let file_ok = map
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| s == "{media_id}.{extension}");
                    if dir_ok && file_ok {
                        needs = false;
                    }
                }
            } else {
                needs = true;
            }
        } else {
            // No highlights override at all — needs addition
            needs = true;
        }
        if has_highlights && !needs {
            // double-check raw contains expected new strings — scrapmf_root
            // required (profile/network/account/content structure). The exact
            // two-slash conditional spec is required: the one-slash variant is
            // broken (DirectoryFormatError) and must re-migrate.
            if raw.contains("{post_id}{highlight_title:?_//}")
                && raw.contains("\"{media_id}.{extension}\"")
                && !raw.contains("{highlight_id}")
                && raw.contains("\"{scrapmf_root}\"")
                && raw.contains("\"{username}\"")
            {
                needs = false;
            }
            // If it still contains highlight_id, force migrate to post_id
            if raw.contains("{highlight_id}_{highlight_title}") {
                needs = true;
            }
        }
    }
    if !needs {
        return Ok(false);
    }
    tracing::warn!(path = %path.display(), "migrating instagram highlights to new rule (no date, highlight_id, media_id only)");

    // `site` was parsed successfully above and is still in scope, so the
    // migration rebuilds from it directly. No second parse, and therefore no
    // way for that parse to fail and hand back a default-constructed Site.
    let mut highlight_table = toml::map::Map::new();
    highlight_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("highlights".to_string())]),
    );
    highlight_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("highlights".to_string()),
            toml::Value::String("{post_id}{highlight_title:?_//}".to_string()),
        ]),
    );
    highlight_table.insert(
        "filename".to_string(),
        toml::Value::String("{media_id}.{extension}".to_string()),
    );
    site.extractor.insert(
        "instagram:highlights".to_string(),
        toml::Value::Table(highlight_table),
    );
    // Stories override (ephemeral 24h): year/month lowercase via f-string formatter
    // \f here is the two-char sequence backslash+f — gallery-dl JSON-decodes it as form feed
    // (f-string formatter prefix) when parsing `-o key=["\fF ..."]`
    let stories_year = "\\fF {date.strftime(\"%Y\")}".to_string();
    let stories_month = "\\fF {date.strftime(\"%m-%B\").lower()}".to_string();
    let mut stories_table = toml::map::Map::new();
    stories_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("stories".to_string())]),
    );
    stories_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("stories".to_string()),
            toml::Value::String(stories_year),
            toml::Value::String(stories_month),
        ]),
    );
    stories_table.insert(
        "filename".to_string(),
        toml::Value::String("{post_id}_{num:02d}.{extension}".to_string()),
    );
    site.extractor.insert(
        "instagram:stories".to_string(),
        toml::Value::Table(stories_table),
    );
    // Avatar override (profile pic): {media_id} (=profile_pic_id, no date)
    let mut avatar_table = toml::map::Map::new();
    avatar_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("profile".to_string()),
        ]),
    );
    avatar_table.insert(
        "filename".to_string(),
        toml::Value::String("{username}_{media_id}.{extension}".to_string()),
    );
    site.extractor.insert(
        "instagram:avatar".to_string(),
        toml::Value::Table(avatar_table),
    );
    // Ensure global templates remain for posts/reels if missing
    if site.filename_template.is_none() {
        site.filename_template =
            Some("{post_id}_{date:%Y-%m-%d}_{num:02d}.{extension}".to_string());
    }
    // Ensure global directory template uses profile/network/account/content order
    let needs_dir_fix = match &site.directory_template {
        None => true,
        Some(v) => v.first().map(|s| s.as_str()) != Some("{scrapmf_root}"),
    };
    if needs_dir_fix {
        site.directory_template = Some(vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{username}".to_string(),
            "{subcategory}".to_string(),
        ]);
    }

    let body = toml::to_string_pretty(&site).context("serialize migrated site")?;
    let header = r#"# scrapmf — site config (1 file per site) — MIGRATED to profile/network/account/content structure
# File: ~/.config/scrapmf/sites/<name>.toml (0o600, dir 0o700)
# Tree: {scrapmf_root}(profile)/{category}(network)/{username}(account)/...
# Highlights: .../highlights/{post_id}{highlight_title:?_//}/{media_id}.{ext} — NO date, NO num
# Stories: .../stories/{year}/{month-lowercase}/{post_id}_{num}.{ext} — ephemeral 24h
# Posts/Reels: .../posts|reels/{post_id}_{date:%Y-%m-%d}_{num:02d}.{ext}
# {scrapmf_root} injected at runtime via extractor.keywords (profile name; fallback "default")
# Migrated from previous version
# See header in newly created sites/instagram.toml for full docs

"#;
    let content = format!("{header}{body}");
    write_config_file(path, &content)?;
    Ok(true)
}

/// Reorder one directory array from old [user, category, ...rest] to
/// new [scrapmf_root, category, user, ...rest]. Returns true if changed.
pub(super) fn reorder_dir_array(arr: &[toml::Value]) -> Option<Vec<toml::Value>> {
    let strs: Vec<Option<&str>> = arr.iter().map(|v| v.as_str()).collect();
    let first_user = matches!(
        strs.first().copied().flatten(),
        Some("{username}") | Some("{user}")
    );
    if !first_user || arr.first() == Some(&toml::Value::String("{scrapmf_root}".into())) {
        return None;
    }
    let mut out = vec![toml::Value::String("{scrapmf_root}".to_string())];
    // category second (if present right after)
    if strs.get(1).copied().flatten() == Some("{category}") {
        out.push(arr[1].clone());
        out.push(arr[0].clone());
        out.extend_from_slice(&arr[2..]);
    } else {
        out.push(arr[0].clone());
        out.extend_from_slice(&arr[1..]);
    }
    Some(out)
}

/// Migrate any non-instagram site file to the scrapmf_root-first directory order.
/// Only touches directory arrays whose first element is {username}/{user};
/// preserves everything else. Returns true if rewritten.
pub(super) fn migrate_site_root_order(path: &Path) -> anyhow::Result<bool> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    // SAFETY: never rewrite empty files — an empty parse yields a default Site
    // (no identity), which would wipe user config via the rate_limit injection.
    if raw.trim().is_empty() {
        return Ok(false);
    }
    let Some(mut site) = parse_site(path, &raw, "site-root-order") else {
        return Ok(false);
    };
    // Instagram has its own dedicated migration — skip here.
    let is_instagram = site.site.as_deref() == Some("instagram")
        || site
            .pattern
            .as_ref()
            .is_some_and(|p| p.contains("instagram"))
        || site.extractor.keys().any(|k| k.starts_with("instagram"));
    if is_instagram {
        return Ok(false);
    }

    let mut changed = false;
    if let Some(dirs) = &mut site.directory_template
        && let Some(new_arr) = reorder_dir_array(
            &dirs
                .iter()
                .map(|s| toml::Value::String(s.clone()))
                .collect::<Vec<_>>(),
        )
    {
        *dirs = new_arr
            .into_iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        changed = true;
    }
    for (k, v) in site.extractor.iter_mut() {
        if let toml::Value::Table(map) = v {
            if let Some(toml::Value::Array(arr)) = map.get("directory")
                && let Some(new_arr) = reorder_dir_array(arr)
            {
                map.insert("directory".to_string(), toml::Value::Array(new_arr));
                changed = true;
            }
            let _ = k;
        }
    }
    // Rate-limit protection: non-instagram sites without [rate_limit] get the
    // proven instagram values (tiktok is aggressive with bot detection).
    if site.rate_limit.is_none() {
        site.rate_limit = Some(RateLimit {
            sleep: Some("3-6".to_string()),
            sleep_request: Some("8-15".to_string()),
            sleep_429: Some(120),
            limit_rate: None,
        });
        changed = true;
    }
    // TikTok posts: flat/extension-based directory → post_type conditional; disable audio
    if let Some(toml::Value::Table(posts)) = site.extractor.get_mut("tiktok:posts") {
        let dir_ok = matches!(posts.get("directory"), Some(toml::Value::Table(_)))
            && posts
                .get("directory")
                .and_then(|d| d.get("post_type == 'image'"))
                .is_some();
        let needs_audio_off = posts.get("audio") != Some(&toml::Value::Boolean(false));
        if !dir_ok || needs_audio_off {
            if !dir_ok {
                posts.insert(
                    "directory".to_string(),
                    tiktok_posts_conditional_directory(),
                );
            }
            posts.insert("audio".to_string(), toml::Value::Boolean(false));
            changed = true;
        }
    }
    if let Some(toml::Value::Table(stories)) = site.extractor.get_mut("tiktok:stories")
        && stories.get("audio") != Some(&toml::Value::Boolean(false))
    {
        stories.insert("audio".to_string(), toml::Value::Boolean(false));
        changed = true;
    }
    // TikTok avatar filename with {_now} date → migrate to {user}_{file_id}
    if let Some(toml::Value::Table(avatar)) = site.extractor.get_mut("tiktok:avatar")
        && let Some(toml::Value::String(fname)) = avatar.get("filename")
        && fname.contains("{_now")
    {
        avatar.insert(
            "filename".to_string(),
            toml::Value::String("{user}_{file_id}.{extension}".to_string()),
        );
        changed = true;
    }
    // Twitter media: broken {type} conditional directory → static videos dir.
    // The photos/videos split happens via scrapmf's two-pass file-filter runs.
    if let Some(toml::Value::Table(media)) = site.extractor.get_mut("twitter:media") {
        let broken_conditional = matches!(media.get("directory"), Some(toml::Value::Table(_)))
            && media
                .get("directory")
                .and_then(|d| d.get("type == 'photo'"))
                .is_some();
        if broken_conditional {
            media.insert(
                "directory".to_string(),
                toml::Value::Array(vec![
                    toml::Value::String("{scrapmf_root}".to_string()),
                    toml::Value::String("{category}".to_string()),
                    toml::Value::String("{user[name]}".to_string()),
                    toml::Value::String("videos".to_string()),
                ]),
            );
            changed = true;
        }
    }
    if !changed {
        return Ok(false);
    }
    tracing::warn!(path = %path.display(), "migrating site directories to scrapmf_root-first order");

    let body = toml::to_string_pretty(&site).context("serialize migrated site")?;
    let header = "# scrapmf — site config — MIGRATED to profile/network/account/content structure\n# Directory arrays now start with {scrapmf_root} (injected at runtime via extractor.keywords)\n# Migrated from previous version\n\n";
    let content = format!("{header}{body}\n");
    write_config_file(path, &content)?;
    Ok(true)
}

/// Date-first filenames (user preference): alphabetical order equals
/// chronological order.
///
/// - Instagram posts/reels (global `filename_template`; stories/highlights
///   have their own scoped overrides and are NOT touched):
///   `{post_id}_{date}_{num}` → `{date}_{post_id}_{num}`
/// - TikTok posts videos/photos (`tiktok:posts.filename`; stories untouched):
///   `{id}_{date}{num}` → `{date}_{id}{num}`
///
/// gallery-dl never parses ids from filenames — it computes the target path
/// from metadata and skips if it already exists — so this rename cannot cause
/// overwrites. Files downloaded with the old format are simply re-fetched
/// once under the new name.
pub fn migrate_filename_date_first(path: &Path) -> anyhow::Result<bool> {
    const IG_OLD: &str = "{post_id}_{date:%Y-%m-%d}_{num:02d}.{extension}";
    const IG_NEW: &str = "{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}";
    const TT_OLD: &str = "{id}_{date:%Y-%m-%d}{num:?_//>02}.{extension}";
    const TT_NEW: &str = "{date:%Y-%m-%d}_{id}{num:?_//>02}.{extension}";

    let raw = std::fs::read_to_string(path)?;
    if !raw.contains(IG_OLD) && !raw.contains(TT_OLD) {
        return Ok(false);
    }
    let Some(mut site) = parse_site(path, &raw, "date-first-filename") else {
        return Ok(false);
    };

    let is_instagram = site.site.as_deref() == Some("instagram")
        || site
            .pattern
            .as_ref()
            .is_some_and(|p| p.contains("instagram"))
        || site.extractor.keys().any(|k| k.starts_with("instagram"));
    let mut changed = false;

    // Instagram: global filename_template drives posts AND reels only —
    // stories/highlights/avatar override it per-sub-extractor, so they keep
    // their own naming regardless of this rewrite.
    if is_instagram && site.filename_template.as_deref() == Some(IG_OLD) {
        site.filename_template = Some(IG_NEW.to_string());
        changed = true;
    }

    // TikTok: only tiktok:posts (videos/photos). Stories keep their scoped
    // filename untouched, per scope decision.
    let is_tiktok = site.site.as_deref() == Some("tiktok")
        || site.pattern.as_ref().is_some_and(|p| p.contains("tiktok"))
        || site.extractor.keys().any(|k| k.starts_with("tiktok"));
    if is_tiktok
        && let Some(toml::Value::Table(posts)) = site.extractor.get_mut("tiktok:posts")
        && posts.get("filename").and_then(|v| v.as_str()) == Some(TT_OLD)
    {
        posts.insert(
            "filename".to_string(),
            toml::Value::String(TT_NEW.to_string()),
        );
        changed = true;
    }

    if !changed {
        return Ok(false);
    }
    tracing::warn!(
        path = %path.display(),
        "migrating filenames to date-first order (alphabetical == chronological)"
    );
    let body = toml::to_string_pretty(&site).context("serialize migrated site")?;
    let header = "# scrapmf — site config — MIGRATED to date-first filenames\n# Posts/reels/videos/photos now start with {date} so alphabetical order equals\n# chronological order. Already-downloaded files with the old name are re-fetched\n# once under the new name (gallery-dl never overwrites; existence is checked by\n# computed path, not parsed filenames).\n# Migrated from previous version\n\n";
    let content = format!("{header}{body}\n");
    write_config_file(path, &content)?;
    Ok(true)
}

/// Apply [`migrate_filename_date_first`] to every sites/*.toml.
pub fn migrate_all_sites_filenames() -> anyhow::Result<usize> {
    let Some(dir) = sites_dir() else {
        return Ok(0);
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut migrated = 0;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|ext| ext == "toml")
            && let Ok(true) = migrate_filename_date_first(&p)
        {
            migrated += 1;
        }
    }
    Ok(migrated)
}

/// Migrate all sites/*.toml: instagram highlights/stories rule + root-order for other sites.
pub fn migrate_all_sites_highlights() -> anyhow::Result<usize> {
    let Some(dir) = sites_dir() else {
        return Ok(0);
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut migrated = 0;
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|ext| ext == "toml") {
            let mut did = false;
            if let Ok(true) = migrate_highlights_if_needed(&p) {
                did = true;
            }
            if let Ok(true) = migrate_site_root_order(&p) {
                did = true;
            }
            if did {
                migrated += 1;
            }
        }
    }
    Ok(migrated)
}

/// Migrate an existing `sites/tiktok.toml`.
///
/// Injects download-robustness flags (`--retries 10`, `--http-timeout 120`)
/// into `extra_args` when missing. A file that does not parse is left alone:
/// rewriting it from defaults would erase the user's settings and nothing is
/// kept to recover from. Comments in the original file are regenerated from the
/// standard header (same policy as other site migrations).
pub(super) fn migrate_tiktok_robustness(path: &Path) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(path)?;
    let Some(mut site) = parse_site(path, &raw, "tiktok-robustness") else {
        return Ok(());
    };

    const ROBUSTNESS_ARGS: &[&str] = &["--retries", "10", "--http-timeout", "120"];
    let flags_missing = !ROBUSTNESS_ARGS
        .iter()
        .all(|a| site.extra_args.iter().any(|x| x == a));
    // Stale doc header: older texts suggested ytdl=true as a benign fallback
    // or simply lacked the carousel warning; yt-dlp silently drops photo
    // carousels, so the file must be regenerated with the warning even if the
    // flags are already correct.
    let stale_docs =
        raw.contains("Fallback option (not enabled here)") || !raw.contains("FALLBACK WARNING");
    if !flags_missing && !stale_docs {
        return Ok(());
    }
    if flags_missing {
        for a in ROBUSTNESS_ARGS {
            if !site.extra_args.iter().any(|x| x == a) {
                site.extra_args.push((*a).to_string());
            }
        }
    }

    let body = toml::to_string_pretty(&site).context("serialize migrated tiktok site")?;
    let header = "# scrapmf — site config — tiktok — MIGRATED\n# Added: --retries 10, --http-timeout 120 (large MP4s from an expiring CDN truncate\n# with gallery-dl defaults of 4 retries / 30s timeout).\n# FALLBACK WARNING: do not enable -o ytdl=true for TikTok — yt-dlp cannot download\n# photo carousels (saves only the background audio) and every slideshow would be lost.\n# Migrated from previous version\n\n";
    let content = format!("{header}{body}\n");
    write_config_file(path, &content)?;
    tracing::info!(
        path = %path.display(),
        "tiktok.toml migrated with --retries 10 --http-timeout 120"
    );
    Ok(())
}

/// One-time migration: `config.toml` historically could contain inline
/// `[sites.*]`, `[presets.*]` and `[profiles.*]` tables. The canonical
/// layout is `sites/*.toml` / `profiles/*.toml` (+ legacy `presets/`).
/// This migrates any inline tables into separate files (without
/// overwriting existing ones) and rewrites `config.toml` to contain only
/// `[general]` (+ `[backend]` if set). Idempotent.
pub fn migrate_inline_config_to_files() -> anyhow::Result<usize> {
    let Some(cfg_path) = super::config_path() else {
        return Ok(0);
    };
    if !cfg_path.is_file() {
        return Ok(0);
    }
    let raw = match std::fs::read_to_string(&cfg_path) {
        Ok(s) => s,
        Err(_) => return Ok(0),
    };
    // Fast path: no inline tables present
    if !raw.contains("[sites.") && !raw.contains("[presets.") && !raw.contains("[profiles.") {
        return Ok(0);
    }
    let cfg: super::Config = match toml::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %cfg_path.display(), error = %e, "inline config unparseable — skipping inline→files migration");
            return Ok(0);
        }
    };
    if cfg.sites.is_empty() && cfg.presets.is_empty() && cfg.profiles.is_empty() {
        return Ok(0);
    }
    let mut migrated = 0usize;

    // Sites → sites/*.toml
    if !cfg.sites.is_empty()
        && let Some(dir) = super::sites_dir()
    {
        let _ = std::fs::create_dir_all(&dir);
        for (name, site) in &cfg.sites {
            let dest = dir.join(format!("{name}.toml"));
            if dest.exists() {
                continue;
            }
            let body = match toml::to_string_pretty(site) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(name = %name, error = %e, "could not serialize site for migration");
                    continue;
                }
            };
            let header = format!(
                "# scrapmf — site config — migrated from config.toml\n# File: ~/.config/scrapmf/sites/{name}.toml (0o600, dir 0o700)\n\n"
            );
            let content = format!("{header}{body}\n");
            if super::fs::write_config_file(&dest, &content).is_ok() {
                // Ensure dir perms
                super::fs::restrict_perms(&dir, true);
                migrated += 1;
                tracing::info!(path = %dest.display(), "migrated inline site to separate file");
            }
        }
    }
    // Presets → presets/*.toml (legacy, but conserve)
    if !cfg.presets.is_empty()
        && let Some(dir) = super::presets_dir()
    {
        let _ = std::fs::create_dir_all(&dir);
        for (name, preset) in &cfg.presets {
            let dest = dir.join(format!("{name}.toml"));
            if dest.exists() {
                continue;
            }
            if let Ok(body) = toml::to_string_pretty(preset) {
                let header = format!(
                    "# scrapmf — preset — migrated from config.toml\n# File: ~/.config/scrapmf/presets/{name}.toml\n\n"
                );
                let content = format!("{header}{body}\n");
                if super::fs::write_config_file(&dest, &content).is_ok() {
                    migrated += 1;
                }
            }
        }
    }
    // Profiles → profiles/*.toml
    if !cfg.profiles.is_empty()
        && let Some(dir) = super::profiles_dir()
    {
        let _ = std::fs::create_dir_all(&dir);
        for (name, profile) in &cfg.profiles {
            let dest = dir.join(format!("{name}.toml"));
            if dest.exists() {
                continue;
            }
            if let Ok(body) = toml::to_string_pretty(profile) {
                let header = format!(
                    "# scrapmf — profile — migrated from config.toml\n# File: ~/.config/scrapmf/profiles/{name}.toml\n\n"
                );
                let content = format!("{header}{body}\n");
                if super::fs::write_config_file(&dest, &content).is_ok() {
                    migrated += 1;
                }
            }
        }
    }

    if migrated == 0 {
        return Ok(0);
    }

    // Rewrite config.toml with only general + backend (sites/presets/profiles are now skip_serializing)
    let cleaned = super::Config {
        general: cfg.general.clone(),
        backend: cfg.backend.clone(),
        ..Default::default()
    };
    let body = toml::to_string_pretty(&cleaned).context("serialize cleaned config")?;
    let header = r#"# scrapmf — main config
# XDG: ~/.config/scrapmf/config.toml (0o600, dir 0o700)
# This file is the global defaults. Site and profile files override it.
#
# [general]
#   output_dir = "~/scrapmf"                    # base output dir (HOME/scrapmf; CLI --output overrides; tilde ~/ expanded)
#   archive = true                              # download archive (dedup per-account)
# See: sites/*.toml for per-site options

"#;
    let content = format!("{header}{body}");
    super::fs::write_config_file(&cfg_path, &content)?;
    tracing::info!(migrated, path = %cfg_path.display(), "migrated inline config to separate files and cleaned config.toml");
    Ok(migrated)
}

/// One-time content migration: user site/profile TOMLs written before the
/// scrapmf rename reference `{scarpmf_root}` in their directory templates;
/// the binary now injects `scrapmf_root`. Rewrites affected files.
/// Returns the number of files updated.
pub fn migrate_legacy_placeholders() -> anyhow::Result<usize> {
    let mut count = 0usize;
    for dir in [super::sites_dir(), super::profiles_dir()]
        .into_iter()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let raw = match std::fs::read_to_string(&path) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if !raw.contains("{scarpmf_root}") {
                    continue;
                }
                let new = raw.replace("{scarpmf_root}", "{scrapmf_root}");
                match write_config_file(&path, &new) {
                    Ok(()) => count += 1,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "placeholder migration failed")
                    }
                }
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod legacy_migration_tests {
    use super::migrate_filename_date_first;
    use super::migrate_highlights_if_needed;
    use super::parse_site;
    use crate::config::migrate_one;
    use std::path::Path;
    use tempfile::TempDir;

    /// Instagram: global filename_template (posts+reels) migrates to
    /// date-first; scoped stories/highlights overrides are untouched.
    #[test]
    fn instagram_posts_reels_go_date_first() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("instagram.toml");
        std::fs::write(
            &path,
            r#"filename_template = "{post_id}_{date:%Y-%m-%d}_{num:02d}.{extension}"

[extractor."instagram:highlights"]
filename = "{media_id}.{extension}"

[extractor."instagram:stories"]
filename = "{post_id}_{num}.{extension}"
"#,
        )
        .expect("seed config");

        assert!(
            migrate_filename_date_first(&path).expect("migration runs"),
            "old instagram filename must migrate"
        );
        let migrated = std::fs::read_to_string(&path).expect("read back");
        assert!(
            migrated.contains(
                r#"filename_template = "{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}""#
            )
        );
        // scoped overrides must survive verbatim
        assert!(migrated.contains(r#"filename = "{media_id}.{extension}""#));
        assert!(migrated.contains(r#"filename = "{post_id}_{num}.{extension}""#));

        // idempotent
        assert!(!migrate_filename_date_first(&path).expect("second run"));
    }

    /// TikTok: only tiktok:posts (videos/photos) migrates; stories keeps its
    /// own scoped filename.
    #[test]
    fn tiktok_posts_go_date_first_but_stories_do_not() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("tiktok.toml");
        std::fs::write(
            &path,
            r#"[extractor."tiktok:posts"]
filename = "{id}_{date:%Y-%m-%d}{num:?_//>02}.{extension}"

[extractor."tiktok:stories"]
filename = "{id}_{date:%Y-%m-%d}{num:?_//>02}.{extension}"
"#,
        )
        .expect("seed config");

        assert!(
            migrate_filename_date_first(&path).expect("migration runs"),
            "old tiktok posts filename must migrate"
        );
        let migrated = std::fs::read_to_string(&path).expect("read back");
        assert!(migrated.contains(r#"[extractor."tiktok:posts"]"#));
        let old_count = migrated.matches("{id}_{date:%Y-%m-%d}").count();
        assert_eq!(
            old_count, 1,
            "only stories should keep the old format:\n{migrated}"
        );
        assert!(migrated.contains(r#"filename = "{date:%Y-%m-%d}_{id}{num:?_//>02}.{extension}""#));

        assert!(!migrate_filename_date_first(&path).expect("second run"));
    }

    #[test]
    fn date_first_migration_noop_on_new_format() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("instagram.toml");
        std::fs::write(
            &path,
            r#"filename_template = "{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}"
"#,
        )
        .expect("seed config");
        assert!(!migrate_filename_date_first(&path).expect("no-op"));
    }

    /// Every retired highlight-title generation must migrate to the current
    /// canonical segment and then stay stable (idempotent).
    const CANONICAL: &str = "{post_id}{highlight_title:?_//}";

    fn seed_highlights_config(title_segment: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("instagram.toml");
        let cfg = format!(
            r#"extra_args = []

[extractor."instagram:highlights"]
include = ["highlights"]
directory = [
    "{{scrapmf_root}}",
    "{{category}}",
    "{{username}}",
    "highlights",
    "{title_segment}",
]
filename = "{{media_id}}.{{extension}}"
"#
        );
        std::fs::write(&path, cfg).expect("seed config");
        (dir, path)
    }

    fn assert_migrates_to_canonical(path: &Path, label: &str) {
        assert!(
            migrate_highlights_if_needed(path).expect("migration runs"),
            "{label}: must be detected as needing migration"
        );
        let migrated = std::fs::read_to_string(path).expect("read back");
        assert!(
            migrated.contains(CANONICAL),
            "{label}: canonical segment missing:\n{migrated}"
        );
        // Idempotency: the output of pass 1 IS the current canonical config,
        // so a second pass must be a no-op.
        assert!(
            !migrate_highlights_if_needed(path).expect("second run"),
            "{label}: migrated config must not migrate again"
        );
    }

    #[test]
    fn highlights_plain_title_migrates() {
        let (_dir, path) = seed_highlights_config("{post_id}_{highlight_title}");
        assert_migrates_to_canonical(&path, "plain title");
    }

    /// Regression: the BROKEN one-slash conditional spec (`!g:?_/}` — shipped
    /// briefly and made gallery-dl abort with `DirectoryFormatError:
    /// ValueError: not enough values to unpack (expected 3, got 2)`).
    #[test]
    fn highlights_broken_conditional_spec_is_repaired() {
        let (_dir, path) = seed_highlights_config("{post_id}{highlight_title!g:?_/}");
        assert_migrates_to_canonical(&path, "broken one-slash spec");
        let repaired = std::fs::read_to_string(&path).expect("read back");
        assert!(
            !repaired.contains("{post_id}{highlight_title!g:?_/}"),
            "broken one-slash spec must be gone"
        );
    }

    /// The retired slugified form erased emoji-only highlight titles
    /// entirely; raw names are wanted, so it migrates too.
    #[test]
    fn highlights_retired_slugified_form_migrates() {
        let (_dir, path) = seed_highlights_config("{post_id}{highlight_title!g:?_//}");
        assert_migrates_to_canonical(&path, "retired slugified form");
        let migrated = std::fs::read_to_string(&path).expect("read back");
        assert!(!migrated.contains("!g"), "!g conversion must be gone");
    }

    // ── Data-loss regressions ──────────────────────────────────────────────
    //
    // Every test below asserts the same invariant: a file this migration
    // cannot parse is left byte-for-byte alone. The file it used to destroy is
    // the one this migration was supposed to fix.

    /// The exact shape that erased user config: a site file that fails to
    /// parse AND carries the marker strings the old raw-text heuristic looked
    /// for. The heuristic set `needs = true`, and the rebuild then called
    /// `toml::from_str(&raw).unwrap_or_default()` — same parser, same input,
    /// so it failed too and returned an empty `Site` that got written over
    /// everything. `write_atomic` renames, so the original was simply gone.
    #[test]
    fn unparseable_site_with_markers_is_untouched() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("instagram.toml");
        // Valid TOML except `sleep = 3` where RateLimit.sleep is a String.
        // Contains both marker strings the deleted heuristic searched for.
        let cfg = r#"site = "instagram"

[rate_limit]
sleep = 3

[extractor."instagram:highlights"]
filename = "{media_id}_{date}.{extension}"
directory = ["{scrapmf_root}", "{category}", "{username}"]
"#;
        std::fs::write(&path, cfg).expect("seed");
        let before = std::fs::read_to_string(&path).expect("reread");

        assert!(
            !migrate_highlights_if_needed(&path).expect("migration runs"),
            "an unparseable file must not be reported as migrated"
        );

        let after = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            before, after,
            "unparseable site config was rewritten — user settings destroyed"
        );
    }

    /// A bare type error is the realistic way a user breaks their own config by
    /// hand: `sleep = 3` instead of `sleep = "3"`, or `sleep_429 = "120"`
    /// instead of a number. No corruption required, only a typo — and serde
    /// rejects the whole file, not just the field.
    #[test]
    fn type_error_in_rate_limit_leaves_file_intact() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("instagram.toml");
        let cfg = r#"site = "instagram"
pattern = "instagram.com"

[rate_limit]
sleep_request = 5

[extractor."instagram:highlights"]
filename = "{media_id}_{date:%Y-%m-%d}.{extension}"
"#;
        std::fs::write(&path, cfg).expect("seed");
        let before = std::fs::read_to_string(&path).expect("reread");

        let _ = migrate_highlights_if_needed(&path).expect("migration runs");

        assert_eq!(
            before,
            std::fs::read_to_string(&path).expect("read back"),
            "a type error must not cost the user their config"
        );
    }

    /// A file that parses but carries no identity would rebuild into a default
    /// `Site` too. Skipping it is the same rule, one step earlier.
    #[test]
    fn identity_less_site_is_untouched() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("instagram.toml");
        // Parses cleanly; site/pattern/extractor are all absent.
        let cfg = "extra_args = []\n";
        std::fs::write(&path, cfg).expect("seed");
        let before = std::fs::read_to_string(&path).expect("reread");

        let _ = migrate_highlights_if_needed(&path).expect("migration runs");

        assert_eq!(
            before,
            std::fs::read_to_string(&path).expect("read back"),
            "an identity-less file must not be rebuilt into defaults"
        );
    }

    /// `parse_site` is the single gate every migration now goes through, so it
    /// gets the coverage the four inline copies never had.
    #[test]
    fn parse_site_rejects_unparseable_and_identity_less() {
        let path = Path::new("/nonexistent/instagram.toml");
        assert!(parse_site(path, "sleep = 3", "test").is_none());
        assert!(parse_site(path, "", "test").is_none());
        assert!(parse_site(path, "extra_args = []\n", "test").is_none());
        assert!(parse_site(path, "site = \"instagram\"\n", "test").is_some());
    }

    #[test]
    fn migrates_legacy_dir_when_new_missing() {
        let base = TempDir::new().expect("tempdir");
        let old = base.path().join("scarpmf");
        let new = base.path().join("scrapmf");
        std::fs::create_dir_all(old.join("sites")).expect("seed");
        std::fs::write(old.join("config.toml"), "x = 1").expect("seed file");

        migrate_one(&old, &new);

        assert!(!old.exists(), "legacy dir should be moved, not copied");
        assert!(new.join("config.toml").is_file());
        assert!(new.join("sites").is_dir());
    }

    #[test]
    fn no_touch_when_new_already_exists() {
        let base = TempDir::new().expect("tempdir");
        let old = base.path().join("scarpmf");
        let new = base.path().join("scrapmf");
        std::fs::create_dir_all(&old).expect("seed old");
        std::fs::write(old.join("old.toml"), "o").expect("seed");
        std::fs::create_dir_all(&new).expect("seed new");
        std::fs::write(new.join("new.toml"), "n").expect("seed");

        migrate_one(&old, &new);

        // Both survive untouched
        assert!(old.exists());
        assert!(Path::new(&new).join("new.toml").is_file());
        assert!(!Path::new(&new).join("old.toml").exists());
    }

    #[test]
    fn no_op_when_nothing_exists() {
        let base = TempDir::new().expect("tempdir");
        migrate_one(&base.path().join("ghost"), &base.path().join("target"));
        assert!(!base.path().join("target").exists());
    }
}
