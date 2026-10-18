//! Browser channel registry — the single source of truth for locating browser
//! cookie databases and the OS keyring entries that decrypt them.
//!
//! Before this module the path list was duplicated in three places and the
//! copies had drifted: `config::cookies::chromium_paths` ignored
//! `XDG_CONFIG_HOME` and knew nothing about Brave Origin, while
//! `providers::browser` knew about Brave Origin and respected XDG, and
//! `commands::doctor` hardcoded `Brave-Browser` yet again. A user who migrated
//! to Brave Origin was silently pointed at the abandoned `Brave-Browser`
//! profile because that directory still existed. Everything now reads from
//! [`channels()`].
//!
//! Two facts drive the layout:
//!
//! * Browsers ship under several channels (`Brave-Browser`, `Brave-Origin`,
//!   `-Beta`, `-Nightly`, …) and each uses a *different* profile directory, so
//!   a channel is an explicit choice rather than a first-found guess.
//! * Chromium-family cookie values are `v11`-encrypted with a key held in the
//!   desktop keyring. The KWallet folder is **per product** (`Brave Keys`,
//!   `Chrome Keys`, …) and *not* the shared `Chromium Keys` — reading the
//!   wrong folder silently yields no key at all, which surfaces as
//!   "no cookies could be decrypted".

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One installable browser channel.
pub struct BrowserChannel {
    /// Stable key used by the CLI, config and cookie profiles.
    pub id: &'static str,
    /// Human label for menus and diagnostics.
    pub display: &'static str,
    /// Executable names to probe, most specific first.
    pub bins: &'static [&'static str],
    /// Profile roots relative to the config home, in priority order.
    pub rel_dirs: &'static [&'static str],
    /// Flatpak application id, for the sandboxed config home.
    pub flatpak_id: Option<&'static str>,
    /// Snap name, for the snap config home.
    pub snap_name: Option<&'static str>,
    /// Whether the cookie DB lives in a per-profile subdirectory.
    pub per_profile: bool,
    /// KWallet folders to try, most likely first.
    pub kwallet_folders: &'static [&'static str],
    /// KWallet entry name holding the safe-storage key.
    pub kwallet_key: &'static str,
    /// Secret Service `application` attribute candidates.
    pub secret_apps: &'static [&'static str],
}

const FIREFOX_BINS: &[&str] = &["firefox", "firefox-esr", "firefox-nightly"];
// Brave ships under many names, and each build owns its own profile directory.
// A channel counts as installed only when ITS binary and ITS cookie database
// both turn up, so the bin list must be per-channel rather than shared: with one
// shared list, `brave-origin-stable` satisfied the probe for plain `brave`, and
// the leftover profile directory of an uninstalled browser then looked like a
// working install. The de-branded Origin builds are the sharpest case — the
// system package installs `brave-origin-stable` and none of the branded names.
const BRAVE_BINS: &[&str] = &["brave", "brave-browser", "brave-browser-stable"];
const BRAVE_ORIGIN_BINS: &[&str] = &["brave-origin", "brave-origin-stable"];
const BRAVE_BETA_BINS: &[&str] = &["brave-beta", "brave-browser-beta"];
const BRAVE_NIGHTLY_BINS: &[&str] = &["brave-nightly", "brave-browser-nightly"];
const BRAVE_ORIGIN_BETA_BINS: &[&str] = &["brave-origin-beta"];
const BRAVE_ORIGIN_NIGHTLY_BINS: &[&str] = &["brave-origin-nightly"];

// Brave ships the same engine under many channel names. Each one keeps its own
// profile directory, but the KWallet entry is a single per-product record, so
// all of them share the folder/key pair and both `brave` and `brave-origin`
// are probed as Secret Service attributes (Brave Origin is a de-branded build
// and the attribute it registers has changed across releases).
const BRAVE_KWALLET_FOLDERS: &[&str] = &["Brave Keys", "Chromium Keys"];
const BRAVE_SECRET_APPS: &[&str] = &["brave", "brave-origin"];

macro_rules! brave_channel {
    ($id:literal, $display:literal, $rel:literal, $bins:expr) => {
        BrowserChannel {
            id: $id,
            display: $display,
            bins: $bins,
            rel_dirs: &[concat!("BraveSoftware/", $rel)],
            flatpak_id: Some("com.brave.Browser"),
            snap_name: Some("brave"),
            per_profile: true,
            kwallet_folders: BRAVE_KWALLET_FOLDERS,
            kwallet_key: "Brave Safe Storage",
            secret_apps: BRAVE_SECRET_APPS,
        }
    };
}

pub const CHANNELS: &[BrowserChannel] = &[
    brave_channel!("brave", "Brave", "Brave-Browser", BRAVE_BINS),
    brave_channel!(
        "brave-origin",
        "Brave Origin",
        "Brave-Origin",
        BRAVE_ORIGIN_BINS
    ),
    brave_channel!(
        "brave-beta",
        "Brave Beta",
        "Brave-Browser-Beta",
        BRAVE_BETA_BINS
    ),
    brave_channel!(
        "brave-nightly",
        "Brave Nightly",
        "Brave-Browser-Nightly",
        BRAVE_NIGHTLY_BINS
    ),
    brave_channel!(
        "brave-origin-beta",
        "Brave Origin Beta",
        "Brave-Origin-Beta",
        BRAVE_ORIGIN_BETA_BINS
    ),
    brave_channel!(
        "brave-origin-nightly",
        "Brave Origin Nightly",
        "Brave-Origin-Nightly",
        BRAVE_ORIGIN_NIGHTLY_BINS
    ),
    BrowserChannel {
        id: "chrome",
        display: "Chrome",
        bins: &["google-chrome", "google-chrome-stable", "chrome"],
        rel_dirs: &["google-chrome"],
        flatpak_id: Some("com.google.Chrome"),
        snap_name: Some("chrome"),
        per_profile: true,
        kwallet_folders: &["Chrome Keys", "Chromium Keys"],
        kwallet_key: "Chrome Safe Storage",
        secret_apps: &["chrome"],
    },
    BrowserChannel {
        id: "chromium",
        display: "Chromium",
        bins: &["chromium", "chromium-browser"],
        rel_dirs: &["chromium"],
        flatpak_id: Some("org.chromium.Chromium"),
        snap_name: Some("chromium"),
        per_profile: true,
        kwallet_folders: &["Chromium Keys"],
        kwallet_key: "Chromium Safe Storage",
        secret_apps: &["chromium"],
    },
    BrowserChannel {
        id: "edge",
        display: "Edge",
        bins: &[
            "microsoft-edge",
            "microsoft-edge-stable",
            "microsoft-edge-beta",
            "microsoft-edge-dev",
        ],
        rel_dirs: &["microsoft-edge"],
        flatpak_id: Some("com.microsoft.Edge"),
        snap_name: None,
        per_profile: true,
        kwallet_folders: &["Chrome Keys", "Chromium Keys"],
        kwallet_key: "Microsoft Edge Safe Storage",
        secret_apps: &["microsoft-edge"],
    },
    BrowserChannel {
        id: "vivaldi",
        display: "Vivaldi",
        bins: &["vivaldi", "vivaldi-stable"],
        rel_dirs: &["vivaldi"],
        flatpak_id: Some("com.vivaldi.Vivaldi"),
        snap_name: Some("vivaldi"),
        per_profile: true,
        kwallet_folders: &["Chrome Keys", "Chromium Keys"],
        kwallet_key: "Vivaldi Safe Storage",
        secret_apps: &["vivaldi-stable"],
    },
    BrowserChannel {
        id: "opera",
        display: "Opera",
        bins: &["opera", "opera-stable"],
        rel_dirs: &["opera"],
        flatpak_id: Some("com.opera.Opera"),
        snap_name: None,
        // Opera keeps its cookie DB at the profile root, not per profile.
        per_profile: false,
        kwallet_folders: &["Chromium Keys"],
        kwallet_key: "Opera Safe Storage",
        secret_apps: &["opera"],
    },
    BrowserChannel {
        id: "firefox",
        display: "Firefox",
        bins: FIREFOX_BINS,
        rel_dirs: &["mozilla/firefox"],
        flatpak_id: Some("org.mozilla.firefox"),
        snap_name: Some("firefox"),
        // Firefox is a separate code path (plaintext SQLite, no keyring).
        per_profile: true,
        kwallet_folders: &[],
        kwallet_key: "",
        secret_apps: &[],
    },
];

/// All channels that use the Chromium v10/v11 keyring encryption.
pub fn chromium_channels() -> impl Iterator<Item = &'static BrowserChannel> {
    CHANNELS.iter().filter(|c| c.id != "firefox")
}

pub fn find_channel(id: &str) -> Option<&'static BrowserChannel> {
    let id = id.to_lowercase();
    CHANNELS.iter().find(|c| c.id == id)
}

/// Resolve a channel from a user-supplied label, tolerating case and spaces
/// ("Brave Origin" → `brave-origin`).
pub fn resolve_channel(input: &str) -> Option<&'static BrowserChannel> {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let want = norm(input);
    CHANNELS
        .iter()
        .find(|c| norm(c.id) == want || norm(c.display) == want)
}

/// Config home, honouring `XDG_CONFIG_HOME`. This is what `dirs::config_dir`
/// does; it is wrapped here so every caller agrees on the definition.
pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("~/.config"))
}

/// Absolute $HOME, or None. Used for the sandboxed (Flatpak/snap) config homes
/// that are *not* under the XDG config dir.
pub fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// A candidate profile root and which packaging supplied it.
pub type Root = (PathBuf, &'static str);

/// Candidate profile roots for a channel, in priority order: native install
/// first, then Flatpak, then snap.
///
/// Pure: the roots are passed in rather than read from the environment so this
/// stays testable without mutating `XDG_CONFIG_HOME` (which would need `unsafe`
/// on Rust 2024, and this crate forbids unsafe).
pub fn roots_for(ch: &BrowserChannel, chome: &Path, home: Option<&Path>) -> Vec<Root> {
    let mut out = Vec::new();
    for rel in ch.rel_dirs {
        out.push((chome.join(rel), "native"));
        if let Some(id) = ch.flatpak_id
            && let Some(home) = home
        {
            out.push((
                home.join(".var/app").join(id).join("config").join(rel),
                "flatpak",
            ));
        }
        if let Some(snap) = ch.snap_name
            && let Some(home) = home
        {
            out.push((
                home.join("snap")
                    .join(snap)
                    .join("common")
                    .join(".config")
                    .join(rel),
                "snap",
            ));
        }
    }
    out
}

/// Candidate profile roots for a channel, resolved from the environment.
pub fn profile_roots(ch: &BrowserChannel) -> Vec<Root> {
    roots_for(ch, &config_home(), home().as_deref())
}

/// KWallet client binaries, newest generation first.
///
/// `kwallet-query` is the legacy KDE 4 tool and is absent from a modern Plasma 6
/// install, which ships `kwalletctl6`. Probing only the legacy name made KWallet
/// support look permanently broken on current KDE.
pub const KWALLET_CLIENTS: &[&str] = &[
    "kwalletctl6",
    "kwallet-query6",
    "kwalletctl5",
    "kwallet-query",
];

/// Candidate binaries, whether or not installed.
pub fn kwallet_clients() -> &'static [&'static str] {
    KWALLET_CLIENTS
}

/// A located cookie database plus the profile it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieDb {
    pub path: PathBuf,
    /// Profile directory name (`Default`, `Profile 2`, …). Empty when the
    /// channel keeps the DB at the profile root.
    pub profile: String,
    /// Which packaging supplied it: `native`, `flatpak` or `snap`.
    pub origin: &'static str,
}

/// The most recently modified file named `filename` under `root`, with the name
/// of the directory that contained it. Prefers the newest write time so a
/// freshly-used profile wins over a stale `Default`.
fn newest_cookie_db(root: &Path, filename: &str) -> Option<(PathBuf, String)> {
    let mut best: Option<(PathBuf, String, SystemTime)> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == filename)
                && let Ok(md) = std::fs::metadata(&p)
                && md.len() > 0
            {
                let mtime = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let profile = p
                    .parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if best.as_ref().is_none_or(|(_, _, t)| mtime > *t) {
                    best = Some((p, profile, mtime));
                }
            }
        }
    }
    best.map(|(p, profile, _)| (p, profile))
}

/// A non-empty regular file. Chromium creates a zero-length `Default/Cookies`
/// before the first run, and that placeholder must not shadow a populated
/// secondary profile.
fn is_populated_db(path: &Path) -> bool {
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
}

/// Locate a cookie DB among `roots`. Pure — see [`roots_for`].
pub fn find_cookie_db_in(roots: &[Root], per_profile: bool, filename: &str) -> Option<CookieDb> {
    for (root, origin) in roots {
        if !root.is_dir() {
            continue;
        }
        if per_profile {
            let default_db = root.join("Default").join(filename);
            if is_populated_db(&default_db) {
                return Some(CookieDb {
                    path: default_db,
                    profile: "Default".to_string(),
                    origin,
                });
            }
        } else {
            let db = root.join(filename);
            if is_populated_db(&db) {
                return Some(CookieDb {
                    path: db,
                    profile: String::new(),
                    origin,
                });
            }
        }
        // No Default/<db>: fall back to the most recently written profile, so a
        // user whose session lives in "Profile 2" is not told their cookies are
        // missing.
        if let Some((path, profile)) = newest_cookie_db(root, filename) {
            return Some(CookieDb {
                path,
                profile,
                origin,
            });
        }
    }
    None
}

/// Locate a channel's cookie database. `Default/<db>` is preferred when
/// present, otherwise the most recently modified profile wins.
pub fn find_cookie_db(ch: &BrowserChannel, filename: &str) -> Option<CookieDb> {
    find_cookie_db_in(&profile_roots(ch), ch.per_profile, filename)
}

/// All channels that actually have a cookie database on this machine.
pub fn available_channels(filename: &str) -> Vec<(&'static BrowserChannel, CookieDb)> {
    CHANNELS
        .iter()
        .filter_map(|c| find_cookie_db(c, filename).map(|db| (c, db)))
        .collect()
}

/// Whether an executable for a channel is on `$PATH`.
pub fn binary_for(ch: &BrowserChannel) -> Option<PathBuf> {
    ch.bins.iter().find_map(|b| which::which(b).ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn temp_root() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("tempdir")
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, b"x").expect("write");
    }

    #[test]
    fn brave_origin_is_a_first_class_channel() {
        let ch = find_channel("brave-origin").expect("channel must exist");
        assert_eq!(ch.display, "Brave Origin");
        assert_eq!(ch.rel_dirs, &["BraveSoftware/Brave-Origin"]);
    }

    /// The regression that started all of this: Brave Origin existed on disk
    /// but the capture path only ever looked at Brave-Browser.
    #[test]
    fn origin_and_stable_are_distinct_roots() {
        let stable = find_channel("brave").expect("stable");
        let origin = find_channel("brave-origin").expect("origin");
        assert_ne!(stable.rel_dirs, origin.rel_dirs);
    }

    #[test]
    fn kwallet_folder_is_per_product_not_shared_chromium() {
        // Reading "Chromium Keys" for Brave is the bug that made decryption
        // fail on KDE: the folder is per product.
        for id in ["brave", "brave-origin", "brave-beta"] {
            let ch = find_channel(id).expect("channel");
            assert_eq!(
                ch.kwallet_folders.first().copied(),
                Some("Brave Keys"),
                "{id} must try 'Brave Keys' first"
            );
            assert!(
                ch.kwallet_folders.contains(&"Chromium Keys"),
                "{id} must still fall back to 'Chromium Keys'"
            );
        }
        let chrome = find_channel("chrome").expect("channel");
        assert_eq!(chrome.kwallet_folders.first().copied(), Some("Chrome Keys"));
        let chromium = find_channel("chromium").expect("channel");
        assert_eq!(
            chromium.kwallet_folders.first().copied(),
            Some("Chromium Keys")
        );
    }

    #[test]
    fn origin_probes_both_secret_service_attributes() {
        let ch = find_channel("brave-origin").expect("channel");
        assert!(ch.secret_apps.contains(&"brave"));
        assert!(ch.secret_apps.contains(&"brave-origin"));
    }

    #[test]
    fn opera_is_not_per_profile() {
        let ch = find_channel("opera").expect("channel");
        assert!(!ch.per_profile);
    }

    #[test]
    fn resolve_channel_tolerates_labels_and_case() {
        assert_eq!(
            resolve_channel("Brave Origin").map(|c| c.id),
            Some("brave-origin")
        );
        assert_eq!(
            resolve_channel("braveorigin").map(|c| c.id),
            Some("brave-origin")
        );
        assert_eq!(resolve_channel("  FIREFOX ").map(|c| c.id), Some("firefox"));
        assert!(resolve_channel("netscape").is_none());
    }

    #[test]
    fn every_channel_has_a_display_and_bins() {
        for c in CHANNELS {
            assert!(!c.display.is_empty(), "{} has no display", c.id);
            assert!(!c.bins.is_empty(), "{} has no bins", c.id);
            assert!(!c.rel_dirs.is_empty(), "{} has no rel_dirs", c.id);
            if c.id != "firefox" {
                assert!(!c.kwallet_key.is_empty(), "{} has no kwallet key", c.id);
            }
        }
    }

    #[test]
    fn channel_ids_are_unique() {
        let mut ids: Vec<&str> = CHANNELS.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate channel id in registry");
    }

    /// `profile_roots` must expand a channel into native + flatpak + snap, all
    /// of which can hold a real cookie DB.
    #[test]
    fn profile_roots_expand_packaging_variants() {
        let ch = find_channel("brave-origin").expect("channel");
        let chome = PathBuf::from("/home/tester/.config");
        let roots = roots_for(ch, &chome, Some(Path::new("/home/tester")));
        let s: Vec<String> = roots
            .iter()
            .map(|(p, o)| format!("{o}:{}", p.display()))
            .collect();
        assert!(
            s.iter()
                .any(|p| p == "native:/home/tester/.config/BraveSoftware/Brave-Origin"),
            "native root missing: {s:?}"
        );
        assert!(
            s.iter()
                .any(|p| p == "flatpak:/home/tester/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Origin"),
            "flatpak root missing: {s:?}"
        );
        assert!(
            s.iter()
                .any(|p| p
                    == "snap:/home/tester/snap/brave/common/.config/BraveSoftware/Brave-Origin"),
            "snap root missing: {s:?}"
        );
    }

    #[test]
    fn find_cookie_db_prefers_default_profile() {
        let dir = temp_root();
        let root = dir.path().join("BraveSoftware/Brave-Browser");
        touch(&root.join("Default/Cookies"));
        touch(&root.join("Profile 3/Cookies"));

        let db = find_cookie_db_in(&[(root, "native")], true, "Cookies").expect("cookie db");
        assert_eq!(db.profile, "Default");
        assert!(db.path.ends_with("Default/Cookies"));
    }

    #[test]
    fn find_cookie_db_falls_back_to_newest_non_default_profile() {
        let dir = temp_root();
        let root = dir.path().join("BraveSoftware/Brave-Origin");
        // No Default/ — only a secondary profile, which must still be found.
        touch(&root.join("Profile 7/Cookies"));

        let db = find_cookie_db_in(&[(root, "native")], true, "Cookies")
            .expect("cookie db must be found without Default");
        assert_eq!(db.profile, "Profile 7");
    }

    #[test]
    fn find_cookie_db_reports_none_when_absent() {
        let dir = temp_root();
        let roots = vec![(dir.path().join("nope"), "native")];
        assert!(find_cookie_db_in(&roots, true, "Cookies").is_none());
    }

    /// A zero-length `Default/Cookies` is what Chromium creates before first
    /// use. It must not shadow a populated secondary profile.
    #[test]
    fn empty_default_does_not_shadow_a_populated_profile() {
        let dir = temp_root();
        let root = dir.path().join("BraveSoftware/Brave-Browser");
        std::fs::create_dir_all(root.join("Default")).expect("mkdir");
        std::fs::write(root.join("Default/Cookies"), b"").expect("write");
        touch(&root.join("Profile 1/Cookies"));

        let db = find_cookie_db_in(&[(root, "native")], true, "Cookies")
            .expect("should fall through to the populated profile");
        assert_eq!(db.profile, "Profile 1");
    }

    /// The exact bug that blocked a Brave Origin user: the Origin profile had
    /// cookies but the search stopped at the abandoned Brave-Browser profile.
    #[test]
    fn origin_is_found_even_when_a_stale_stable_profile_exists() {
        let dir = temp_root();
        let stable = dir.path().join("BraveSoftware/Brave-Browser");
        let origin = dir.path().join("BraveSoftware/Brave-Origin");
        touch(&stable.join("Default/Cookies"));
        touch(&origin.join("Default/Cookies"));

        // Querying the Origin channel must return the Origin path, not the
        // stable one that happens to sort first on disk.
        let chome = dir.path();
        let roots = roots_for(find_channel("brave-origin").expect("channel"), chome, None);
        let db = find_cookie_db_in(&roots, true, "Cookies").expect("db");
        assert!(
            db.path.ends_with("Brave-Origin/Default/Cookies"),
            "resolved the wrong profile: {}",
            db.path.display()
        );
    }

    #[test]
    fn non_per_profile_channel_reads_root_level_db() {
        let dir = temp_root();
        let root = dir.path().join("opera");
        touch(&root.join("Cookies"));
        let db = find_cookie_db_in(&[(root, "native")], false, "Cookies").expect("db");
        assert_eq!(db.profile, "");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod channel_bin_tests {
    use super::{CHANNELS, find_channel};

    /// A Brave Origin install publishes `brave-origin-stable` and none of the
    /// branded names. With those missing from the probe list the channel
    /// reported "not installed" while its profile directory was right there,
    /// which is how `doctor` and the browser picker both hid it.
    #[test]
    fn every_published_brave_name_is_probed_by_some_channel() {
        for expected in [
            "brave",
            "brave-browser",
            "brave-browser-stable",
            "brave-beta",
            "brave-browser-beta",
            "brave-nightly",
            "brave-browser-nightly",
            "brave-origin",
            "brave-origin-stable",
            "brave-origin-beta",
            "brave-origin-nightly",
        ] {
            assert!(
                CHANNELS
                    .iter()
                    .filter(|c| c.id.starts_with("brave"))
                    .any(|c| c.bins.contains(&expected)),
                "no Brave channel probes {expected}"
            );
        }
    }

    /// The regression this replaced: one shared bin list let
    /// `brave-origin-stable` satisfy the probe for plain `brave`, so an
    /// uninstalled browser whose profile directory survived looked installed —
    /// and selecting it sent a session that had been dead for two weeks.
    #[test]
    fn a_brave_channel_never_probes_another_channels_binary() {
        let branded = find_channel("brave").expect("channel exists");
        let origin = find_channel("brave-origin").expect("channel exists");
        assert!(
            !branded.bins.contains(&"brave-origin-stable"),
            "plain Brave must not be satisfied by the Origin binary"
        );
        assert!(
            !origin.bins.contains(&"brave-browser-stable"),
            "Brave Origin must not be satisfied by the branded binary"
        );
    }

    /// The de-branded builds keep their own profile directories, so the shared
    /// bin list must not collapse them: each channel still resolves its own.
    #[test]
    fn brave_channels_keep_distinct_profile_dirs() {
        let origin = find_channel("brave-origin").expect("channel exists");
        let branded = find_channel("brave").expect("channel exists");
        assert!(origin.rel_dirs[0].ends_with("Brave-Origin"));
        assert!(branded.rel_dirs[0].ends_with("Brave-Browser"));
        assert_ne!(origin.rel_dirs, branded.rel_dirs);
    }
}
