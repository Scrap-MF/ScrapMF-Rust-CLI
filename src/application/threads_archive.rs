//! Bridge between scrapmf's download archive and the threads plugin's ledger.
//!
//! gallery-dl keeps its own archive table and scrapmf mirrors it into
//! `archive/<site>/<account>.jsonl` by seeding a disposable sqlite cache and
//! draining it back. The threads plugin has no such table — it grew its own
//! append-only ledger at `<dest>/.archive/dedup.jsonl`, keyed by
//! `(post_id, index)`.
//!
//! scrapmf stays the owner of the canonical record, so the two are bridged:
//!
//! * **before** the run, our keys are injected into the plugin's ledger, which
//!   is the only way to make it skip work it would otherwise repeat — it has no
//!   flag to import foreign keys;
//! * **after** the run, its ledger is drained back into ours.
//!
//! The plugin's loader tolerates this: it requires only `post_id` and `index`,
//! ignores every other field, and skips lines it cannot parse. Its own ledger
//! lives inside the media directory, so injecting a key for a file that no
//! longer exists on disk is harmless — the plugin still verifies the file
//! before skipping.

use std::collections::HashSet;
use std::io::{BufRead, BufWriter, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Category prefix, matching gallery-dl's `"<category> <id>"` key shape so the
/// JSONL stays uniform across providers.
const CATEGORY: &str = "threads";

/// Directory the plugin keeps its ledger in, relative to `--dest`.
const LEDGER_DIR: &str = ".archive";
const LEDGER_FILE: &str = "dedup.jsonl";

/// Our archive key for one downloadable item.
///
/// The plugin's granularity is `(post_id, index)` — a carousel post yields
/// several files — so both halves are part of the key.
pub fn key(post_id: &str, index: i64) -> String {
    format!("{CATEGORY} {post_id}_{index}")
}

/// Inverse of [`key`].
///
/// Splits on the first space only, so a post id that itself contains a space
/// still round-trips. Returns `None` for a key belonging to another category,
/// which is what keeps instagram archive entries out of a threads ledger.
pub fn parse_key(k: &str) -> Option<(String, i64)> {
    let (category, id) = k.split_once(' ')?;
    if category != CATEGORY {
        return None;
    }
    // The id is `<post_id>_<index>`; the post id may contain underscores, so
    // split on the last one.
    let (post_id, index) = id.rsplit_once('_')?;
    if post_id.is_empty() {
        return None;
    }
    Some((post_id.to_string(), index.parse().ok()?))
}

/// Where the plugin keeps its ledger for a given `--dest`.
pub fn ledger_path(dest: &Path) -> PathBuf {
    dest.join(LEDGER_DIR).join(LEDGER_FILE)
}

/// Keys the plugin's ledger already holds, as scrapmf keys.
fn existing_keys(ledger: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(fh) = std::fs::File::open(ledger) else {
        return out;
    };
    for line in std::io::BufReader::new(fh).lines().map_while(Result::ok) {
        if let Some(entry) = parse_ledger_line(&line) {
            out.insert(key(&entry.0, entry.1));
        }
    }
    out
}

/// `(post_id, index)` from one ledger line, or `None` if it is not ours to read.
fn parse_ledger_line(line: &str) -> Option<(String, i64)> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let post_id = v.get("post_id")?.as_str()?;
    if post_id.is_empty() {
        return None;
    }
    let index = match v.get("index") {
        None => 1,
        Some(i) => i.as_i64()?,
    };
    Some((post_id.to_string(), index))
}

/// Add our known keys to the plugin's ledger so it can skip them.
///
/// Returns how many lines were appended. Best-effort: a ledger that cannot be
/// written only costs a re-download, never a failed scrape.
pub fn inject(ledger: &Path, keys: &HashSet<String>) -> std::io::Result<usize> {
    let present = existing_keys(ledger);
    let mut fresh: Vec<(String, i64)> = keys
        .iter()
        .filter_map(|k| parse_key(k))
        .filter(|(post_id, index)| !present.contains(&key(post_id, *index)))
        .collect();
    if fresh.is_empty() {
        return Ok(0);
    }
    // Stable order keeps the file diffable between runs.
    fresh.sort();

    if let Some(dir) = ledger.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    // The file handle must outlive the writer, so it is bound separately.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger)?;
    let mut w = BufWriter::new(file);
    // `path` and `t` are deliberately omitted: the plugin's loader only needs
    // `post_id` and `index`, and this record is a claim that the file was
    // downloaded, not a fresh download to be logged.
    for (post_id, index) in &fresh {
        let entry = serde_json::json!({ "post_id": post_id, "index": index });
        writeln!(w, "{}", entry)?;
    }
    w.flush()?;
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(ledger, std::fs::Permissions::from_mode(0o600));
    }
    Ok(fresh.len())
}

/// Read the plugin's ledger back as scrapmf keys.
///
/// Lines the plugin wrote carry `path` and `t`; lines scrapmf injected carry
/// only `post_id` and `index`. Both are accepted, and anything else is skipped.
pub fn drain(ledger: &Path) -> std::io::Result<HashSet<String>> {
    let mut out = HashSet::new();
    let Ok(fh) = std::fs::File::open(ledger) else {
        return Ok(out);
    };
    for line in std::io::BufReader::new(fh).lines().map_while(Result::ok) {
        if let Some((post_id, index)) = parse_ledger_line(&line) {
            out.insert(key(&post_id, index));
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrapmf-threads-archive-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn key_round_trips_including_awkward_ids() {
        assert_eq!(key("DPxyz", 1), "threads DPxyz_1");
        assert_eq!(parse_key("threads DPxyz_1"), Some(("DPxyz".into(), 1)));
        // A post id with underscores must not be split at the wrong place.
        assert_eq!(
            parse_key("threads my_user_profile_2"),
            Some(("my_user_profile".into(), 2))
        );
        // Nor at a space: only the first space separates the category.
        assert_eq!(parse_key("threads odd id_3"), Some(("odd id".into(), 3)));
    }

    #[test]
    fn parse_key_refuses_other_categories_and_junk() {
        // This is what stops an instagram archive from being injected into a
        // threads ledger, where every entry would be a phantom download.
        assert_eq!(parse_key("instagram 3182345678"), None);
        assert_eq!(parse_key("threads"), None);
        assert_eq!(parse_key("threads noindex"), None);
        assert_eq!(parse_key("threads _1"), None);
        assert_eq!(parse_key("threads abc_notanumber"), None);
    }

    #[test]
    fn ledger_lives_under_dest() {
        assert_eq!(
            ledger_path(Path::new("/media/user/photos")),
            Path::new("/media/user/photos/.archive/dedup.jsonl")
        );
    }

    #[test]
    fn inject_then_drain_round_trips() {
        let dir = tmp("round-trip");
        let ledger = ledger_path(&dir);
        let known: HashSet<String> = ["threads AAA_1", "threads BBB_2"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert_eq!(inject(&ledger, &known).unwrap(), 2);
        let drained = drain(&ledger).unwrap();
        assert!(drained.contains("threads AAA_1"));
        assert!(drained.contains("threads BBB_2"));
    }

    #[test]
    fn inject_is_idempotent() {
        let dir = tmp("idempotent");
        let ledger = ledger_path(&dir);
        let known: HashSet<String> = ["threads AAA_1"].iter().map(|s| s.to_string()).collect();

        assert_eq!(inject(&ledger, &known).unwrap(), 1);
        // Re-injecting must not grow the ledger: the plugin dedups on load, but
        // an ever-growing file is a leak nobody notices until it matters.
        assert_eq!(inject(&ledger, &known).unwrap(), 0);
        assert_eq!(inject(&ledger, &known).unwrap(), 0);
        assert_eq!(drain(&ledger).unwrap().len(), 1);
    }

    #[test]
    fn drain_ignores_junk_and_foreign_categories() {
        let dir = tmp("tolerant");
        let ledger = ledger_path(&dir);
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        // What the plugin writes, what scrapmf injects, and damage.
        let body = concat!(
            r#"{"post_id":"AAA","index":1,"path":"/m/AAA_1.jpg","t":1756000000}"#,
            "\n",
            r#"{"post_id":"BBB"}"#,
            "\n",
            "{ not json\n",
            "\n",
            "[1,2,3]\n",
            r#"{"post_id":""}"#,
            "\n",
            r#"{"post_id":"CCC","index":"two"}"#,
        );
        std::fs::write(&ledger, body).unwrap();

        let out = drain(&ledger).unwrap();
        assert_eq!(out.len(), 2, "AAA_1 and BBB_1 (index defaults to 1)");
        assert!(out.contains("threads AAA_1"));
        assert!(out.contains("threads BBB_1"));
    }

    #[test]
    fn inject_creates_the_ledger_directory() {
        let dir = tmp("mkdir");
        let nested = dir.join("photos");
        std::fs::create_dir_all(&nested).unwrap();
        let ledger = ledger_path(&nested);
        assert!(!ledger.exists());

        let known: HashSet<String> = ["threads AAA_1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(inject(&ledger, &known).unwrap(), 1);
        assert!(ledger.exists());
    }
}
