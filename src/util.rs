use std::path::{Path, PathBuf};

/// Sanitize a name component for filesystem use.
/// Keeps ASCII alphanumeric + `-`/`_` (and optionally `.`), replaces others with `_`,
/// truncates to `max_len`, returns `default` if empty after sanitizing.
pub fn sanitize_component(name: &str, max_len: usize, default: &str) -> String {
    let cleaned: String = name
        .chars()
        .take(max_len)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        default.to_string()
    } else {
        cleaned
    }
}

/// Variant that also allows `.` (used for runlog).
pub fn sanitize_component_with_dot(name: &str, max_len: usize, default: &str) -> String {
    let cleaned: String = name
        .chars()
        .take(max_len)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        default.to_string()
    } else {
        cleaned
    }
}

/// Expand a leading `~` or `$HOME` reference to the user's home directory.
///
/// Both forms are accepted because both are natural to type and the config is
/// hand-edited on the phone: `~/scrapmf` and `$HOME/scrapmf` mean the same
/// place, and only one of them expanding silently created a literal directory
/// named `$HOME` relative to the working directory.
///
/// `$HOME` is only expanded in leading position (`$HOME`, `$HOME/rest`,
/// `${HOME}/rest`); a variable anywhere else, or one that is not set, is left
/// exactly as written rather than guessed at.
pub fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let Some(home) = dirs::home_dir() else {
        return path.to_path_buf();
    };
    if let Some(stripped) = s.strip_prefix("~/") {
        return home.join(stripped);
    }
    if s == "~" {
        return home;
    }
    let rest = if let Some(rest) = s.strip_prefix("${HOME}/") {
        rest
    } else if let Some(rest) = s.strip_prefix("$HOME/") {
        rest
    } else if s == "$HOME" || s == "${HOME}" {
        return home;
    } else {
        return path.to_path_buf();
    };
    home.join(rest)
}

#[cfg(test)]
mod tests {
    use super::{expand_tilde, sanitize_component, sanitize_component_with_dot};
    use std::path::Path;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_component("instagram", 64, "misc"), "instagram");
        assert_eq!(
            sanitize_component("../etc/passwd", 64, "misc"),
            "___etc_passwd"
        );
        assert_eq!(sanitize_component("", 64, "misc"), "misc");
        assert_eq!(sanitize_component(&"a".repeat(100), 64, "misc").len(), 64);
    }

    #[test]
    fn sanitize_with_dot() {
        assert_eq!(
            sanitize_component_with_dot("weird:name/with spaces", 32, "job"),
            "weird_name_with_spaces"
        );
        assert_eq!(sanitize_component_with_dot("", 32, "job"), "job");
    }

    #[test]
    fn expand_tilde_home() {
        let p = expand_tilde(Path::new("~/scrapmf"));
        // If HOME is set, should not start with ~
        assert!(!p.to_string_lossy().starts_with("~/") || p == Path::new("~/scrapmf"));
    }

    /// `$HOME/...` is what a hand-edited Android config naturally ends up
    /// containing, and it used to survive expansion untouched — producing a
    /// directory literally named `$HOME`.
    #[test]
    #[allow(clippy::expect_used)]
    fn expands_home_env_forms() {
        let home = dirs::home_dir().expect("home dir");
        for raw in ["$HOME/scrapmf", "${HOME}/scrapmf"] {
            assert_eq!(
                expand_tilde(Path::new(raw)),
                home.join("scrapmf"),
                "{raw} should expand to the home directory"
            );
        }
        assert_eq!(expand_tilde(Path::new("$HOME")), home);
        assert_eq!(expand_tilde(Path::new("${HOME}")), home);
        assert_eq!(expand_tilde(Path::new("~")), home);
    }

    #[test]
    fn leaves_everything_else_alone() {
        for raw in [
            "/abs/path",
            "relative/path",
            "$OTHER/x",
            "a/$HOME/b",
            "HOME/x",
        ] {
            assert_eq!(expand_tilde(Path::new(raw)), Path::new(raw), "{raw}");
        }
    }
}
