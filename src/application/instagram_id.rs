//! Resolve the numeric account id behind an Instagram profile URL and write it
//! to `Instagram_id.txt` at the account root.
//!
//! # Why this cannot just be another content kind
//!
//! gallery-dl *can* resolve the account — `InstagramInfoExtractor` is bound to
//! `https://www.instagram.com/USER/info/` — but its `items()` yields
//! `(Message.Directory, "", user.copy())`. That is directory metadata, never a
//! downloadable file, so pointing a normal scrape at that URL writes **zero**
//! files and still exits reporting success:
//!
//! ```text
//! $ gallery-dl "https://www.instagram.com/USER/info/"
//! archivos creados: 0
//! ```
//!
//! `-g` is no help either: a Directory message carries no URL to print. The
//! only output that carries the id is `--write-info-json`, which writes a full
//! profile dump. So this module runs gallery-dl for that dump into a scratch
//! directory, reads `id` back, and writes the one-line file itself.
//!
//! # Why scrapmf computes the path instead of letting gallery-dl
//!
//! The destination is the account root — the sibling of `posts/`, `reels/` and
//! friends — not a `{subcategory}` directory, and gallery-dl would have no
//! reason to put it there. Every piece of the path is already known here
//! (`output`, `profile_name`, the site id and the account handle), so the
//! layout rule is reproduced rather than the second file silently landing in a
//! place nobody looks.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

use crate::application::scraper::ScrapeRequest;
use crate::providers::Provider;
use crate::providers::gallery_dl::GalleryDl;

/// File name written at the account root.
pub const ID_FILENAME: &str = "Instagram_id.txt";

/// True for `https://www.instagram.com/USER/info/`.
///
/// Deliberately narrow: `/info/` is the only path where this applies, and
/// mistaking some other URL for it would divert a real scrape into an id-only
/// run.
pub fn is_info_url(url: &str) -> bool {
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
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or(path)
        .trim_matches('/');
    let mut parts = path.split('/');
    let (Some(account), Some("info")) = (parts.next(), parts.next()) else {
        return false;
    };
    // Exactly USER/info — a deeper path is a different sub-extractor.
    parts.next().is_none() && !account.is_empty() && !account.contains(':')
}

/// The account handle an `/info/` URL points at, if it is one.
pub fn account_from_info_url(url: &str) -> Option<String> {
    is_info_url(url).then(|| {
        url.split("instagram.com/")
            .nth(1)
            .and_then(|r| r.split(['?', '#']).next())
            .map(|p| p.trim_matches('/'))
            .unwrap_or_default()
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string()
    })
}

/// The subset of gallery-dl's `info.json` that is actually read. Deserializing
/// the whole dump would couple this to dozens of Instagram fields that change
/// without notice; ignoring unknown keys keeps it to the one that matters.
#[derive(Debug, Deserialize)]
struct UserInfo {
    id: String,
}

/// A self-cleaning scratch directory under the system temp dir.
///
/// `tempfile` is a dev-dependency only, and pulling it into the binary for one
/// directory was not worth a dependency. The name carries the pid so two
/// concurrent scrapes cannot collide, and `Drop` removes it even on the error
/// paths.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("scrapmf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Ask gallery-dl for the account behind `url` and return its numeric id.
///
/// The session, rate limits and filename handling all come from the normal
/// builder, so a private account resolves with the same credentials a media
/// scrape would use.
pub fn resolve(req: &ScrapeRequest, url: &str) -> anyhow::Result<String> {
    let exe = GalleryDl::binary().context("gallery-dl backend is not installed")?;
    let scratch = Scratch::new("insta-id").context("could not create a scratch directory")?;

    // Redirect by repointing the request, not by appending a second `-D`.
    // `build_args` already emits `--destination` from `req.output`, and
    // argparse keeps the first of two destination flags — appending lost the
    // race and dropped a full profile dump into the real output tree.
    let mut scratch_req = req.clone();
    scratch_req.output = Some(scratch.path().to_path_buf());

    let mut args = GalleryDl
        .build_args(&scratch_req)
        .context("could not build gallery-dl arguments")?;
    args.push(std::ffi::OsString::from("--write-info-json"));
    let output = crate::process::Executor::run_capturing(&exe.to_string_lossy(), &args)
        .with_context(|| format!("gallery-dl could not resolve {url}"))?;
    if !output.status.success() {
        // The run can fail *and* still have written the dump — a 429 partway
        // through does exactly that. Prefer the file when it is there.
        if find_info_json(scratch.path()).is_none() {
            anyhow::bail!(
                "gallery-dl exited {} resolving {url}\n  {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    let json = find_info_json(scratch.path())
        .context("gallery-dl wrote no info.json — the account may be unavailable")?;
    let raw = std::fs::read_to_string(&json)
        .with_context(|| format!("could not read {}", json.display()))?;
    let user: UserInfo = serde_json::from_str(&raw)
        .with_context(|| format!("{} did not contain an account id", json.display()))?;

    let id = user.id.trim().to_string();
    anyhow::ensure!(
        !id.is_empty(),
        "{} contained an empty account id",
        json.display()
    );
    Ok(id)
}

/// Where the id file belongs for `account`.
///
/// Mirrors how gallery-dl is told what `{scrapmf_root}` is: the explicit profile
/// name, else the account from the URL, else a generic `default`.
pub fn target_path(req: &ScrapeRequest, account: &str) -> PathBuf {
    let root = crate::application::scraper::scrapmf_root(req);
    let base = match &req.output {
        Some(out) => crate::config::expand_output_dir(out),
        None => {
            let cfg = crate::config::load().unwrap_or_default();
            crate::config::expand_output_dir(&cfg.general.output_dir)
        }
    };

    // Derived from `req.directory_template` rather than re-stating the layout,
    // because that template is not one fixed shape: a profile scrape keeps
    // `[root, category, username, subcategory]` while a quick scrape flattens
    // it to `[root, subcategory]`. The only tree that actually exists here is
    // the flattened one — `~/scrapmf/<account>/posts/` — so hardcoding the
    // profile shape filed the id at `~/scrapmf/<account>/instagram/<account>/`,
    // a directory nothing else ever creates.
    let segments = req
        .directory_template
        .as_ref()
        .map(|dirs| {
            // Everything from `{subcategory}` down describes the media kind,
            // not the account: the id belongs beside `posts/`, not inside one.
            let upto = dirs
                .iter()
                .position(|s| s.contains("{subcategory}"))
                .unwrap_or(dirs.len());
            dirs[..upto].to_vec()
        })
        // No template configured: fall back to the profile-shaped tree.
        .unwrap_or_else(|| {
            vec![
                "{scrapmf_root}".to_string(),
                "instagram".to_string(),
                "{username}".to_string(),
            ]
        });

    let mut path = base;
    for seg in segments {
        let seg = seg
            .replace("{scrapmf_root}", &root)
            .replace("{scarpmf_root}", &root)
            .replace("{category}", "instagram")
            .replace("{username}", account)
            .replace("{user}", account)
            .replace("{user[name]}", account);
        if !seg.is_empty() {
            path = path.join(seg);
        }
    }
    path.join(ID_FILENAME)
}

/// Write the id to `path`, creating the account directory if needed.
pub fn write_id(path: &Path, id: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
    }
    std::fs::write(path, format!("{id}\n"))
        .with_context(|| format!("could not write {}", path.display()))
}

/// The scratch run nests under `<scratch>/instagram/<account>/`; a recursive walk
/// avoids depending on that exact shape.
fn find_info_json(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: usize) -> Option<PathBuf> {
        if depth > 4 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        let mut fallback = None;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path, depth + 1) {
                    return Some(found);
                }
            } else if path.file_name().is_some_and(|n| n == "info.json") {
                return Some(path);
            } else if fallback.is_none() {
                fallback = Some(path);
            }
        }
        fallback
    }
    walk(root, 0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn info_urls_are_recognized() {
        assert!(is_info_url("https://www.instagram.com/example_user/info/"));
        assert!(is_info_url("https://instagram.com/example_user/info"));
        assert!(is_info_url(
            "http://www.instagram.com/example_user/info/?hl=en"
        ));
    }

    /// The cost of a false positive is high: a real media scrape would be
    /// diverted into an id-only run and download nothing.
    #[test]
    fn other_instagram_paths_are_not_info_urls() {
        for url in [
            "https://www.instagram.com/example_user/",
            "https://www.instagram.com/example_user/reels/",
            "https://www.instagram.com/stories/example_user/",
            "https://www.instagram.com/example_user/highlights/123/",
            "https://www.instagram.com/example_user/info/extra/",
            "https://www.instagram.com/id:6883230550/info/",
            "https://www.instagram.com//info/",
            "https://www.tiktok.com/@someone/info/",
            "not a url",
        ] {
            assert!(!is_info_url(url), "must not match: {url}");
        }
    }

    #[test]
    fn account_is_read_from_the_info_url() {
        assert_eq!(
            account_from_info_url("https://www.instagram.com/example_user/info/").as_deref(),
            Some("example_user")
        );
        assert_eq!(account_from_info_url("https://www.instagram.com/x/"), None);
    }

    #[test]
    fn id_is_parsed_from_info_json() {
        let user: UserInfo =
            serde_json::from_str(r#"{"id":"6883230550","username":"x","is_private":false}"#)
                .expect("parse");
        assert_eq!(user.id, "6883230550");
    }

    /// A profile dump without an id must not become an empty file — the whole
    /// point of the file is that it carries the id.
    #[test]
    fn json_without_an_id_is_rejected() {
        let parsed: Result<UserInfo, _> = serde_json::from_str(r#"{"username":"x"}"#);
        assert!(parsed.is_err());
    }

    fn base_req(output: &str, profile: Option<&str>) -> ScrapeRequest {
        ScrapeRequest {
            url: "https://www.instagram.com/demo_account/info/".to_string(),
            output: Some(PathBuf::from(output)),
            profile_name: profile.map(str::to_string),
            preset: Some("instagram".to_string()),
            ..Default::default()
        }
    }

    fn profile_tree() -> Vec<String> {
        vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{username}".to_string(),
            "{subcategory}".to_string(),
        ]
    }

    /// A profile scrape keeps the network and account segments.
    #[test]
    fn target_path_uses_the_profile_name() {
        let mut req = base_req("/tmp/out", Some("miprofile"));
        req.directory_template = Some(profile_tree());
        assert_eq!(
            target_path(&req, "demo_account"),
            PathBuf::from("/tmp/out/miprofile/instagram/demo_account/Instagram_id.txt")
        );
    }

    /// A quick scrape flattens to `[root, subcategory]`, which is the tree that
    /// actually exists on this machine — `~/scrapmf/<account>/posts/`. The id
    /// has to land there or it is stranded in a directory nothing creates.
    #[test]
    fn target_path_follows_the_flattened_quick_tree() {
        let mut req = base_req("/tmp/out", None);
        req.directory_template = Some(vec![
            "{scrapmf_root}".to_string(),
            "{subcategory}".to_string(),
        ]);
        assert_eq!(
            target_path(&req, "demo_account"),
            PathBuf::from("/tmp/out/demo_account/Instagram_id.txt")
        );
    }

    /// Same shape, but rooted at the profile name.
    #[test]
    fn target_path_lands_beside_the_media() {
        let mut req = base_req("/tmp/out", Some("miprofile"));
        req.directory_template = Some(vec![
            "{scrapmf_root}".to_string(),
            "{subcategory}".to_string(),
        ]);
        let id_dir = target_path(&req, "demo_account")
            .parent()
            .expect("has a parent")
            .to_path_buf();
        let posts = PathBuf::from("/tmp/out/miprofile/posts");
        assert_eq!(
            id_dir,
            posts.parent().expect("posts has a parent"),
            "the id belongs AT the account root, beside posts/ — not inside it"
        );
    }

    /// With no template at all, fall back to the profile-shaped tree rather
    /// than guessing.
    #[test]
    fn target_path_without_a_template_uses_the_profile_shape() {
        let req = base_req("/tmp/out", None);
        assert_eq!(
            target_path(&req, "demo_account"),
            PathBuf::from("/tmp/out/demo_account/instagram/demo_account/Instagram_id.txt")
        );
    }

    #[test]
    fn target_path_expands_a_tilde_output() {
        let req = base_req("~/scrapmf", Some("miprofile"));
        let p = target_path(&req, "demo_account");
        assert!(!p.to_string_lossy().starts_with('~'), "{p:?}");
        assert!(
            p.ends_with("miprofile/instagram/demo_account/Instagram_id.txt"),
            "{p:?}"
        );
    }

    /// The file is a bare id, so anything extra breaks the thing people paste
    /// this into.
    #[test]
    fn written_file_holds_only_the_id() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("nested").join(ID_FILENAME);
        write_id(&path, "6883230550").expect("write");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "6883230550\n"
        );
    }

    #[test]
    fn info_json_is_found_whatever_the_nesting() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let nested = dir.path().join("instagram").join("someone");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let want = nested.join("info.json");
        std::fs::write(&want, "{}").expect("write");
        assert_eq!(find_info_json(dir.path()), Some(want));
    }

    #[test]
    fn an_empty_scratch_dir_yields_nothing() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(find_info_json(dir.path()), None);
    }
}
