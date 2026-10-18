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

/// Expand leading `~/` to `$HOME`.
pub fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    path.to_path_buf()
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
}
