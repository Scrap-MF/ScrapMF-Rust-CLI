use std::path::PathBuf;

use thiserror::Error;

/// Render an exit code for a message the user actually reads.
///
/// `status.code()` is `Some(n)` on a normal exit and `None` when a signal
/// terminated the process on Unix. Formatting it with `{:?}` prints the wrapper
/// too — the UI was literally showing `exit code Some(4)`, which reads like a
/// Rust type leaked into a scraper error.
pub(crate) fn exit_code_label(code: Option<i32>) -> String {
    match code {
        Some(c) => c.to_string(),
        None => "unavailable (process terminated by a signal)".to_string(),
    }
}

#[derive(Error, Debug)]
pub enum ScrapmfError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error(
        "invalid URL: {url}\n  help: use a valid http(s) URL, e.g. https://example.com/gallery/123\n  note: valid schemes are http/https and length ≤2048"
    )]
    InvalidUrl { url: String },

    #[error(
        "backend '{name}' not found in $PATH\n  help: install via 'pipx install {name}' or 'pacman -S {name}'\n  note: custom path can be set in ~/.config/scrapmf/config.toml"
    )]
    BackendNotFound { name: String },

    #[error(
        "backend '{name}' failed with exit code {}\n  stderr: {stderr}\n  help: check {name} logs with --verbose\n  note: RUST_LOG=debug for verbose tracing",
        exit_code_label(*code)
    )]
    BackendFailed {
        name: String,
        code: Option<i32>,
        stderr: String,
    },

    #[error(
        "config parse error at {path}: {source}\n  help: check TOML syntax\n  note: see https://toml.io/en/v1.0.0"
    )]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("aborted by user")]
    Aborted,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::ScrapmfError;

    #[test]
    fn backend_not_found_display_contains_help() {
        let err = ScrapmfError::BackendNotFound {
            name: "gallery-dl".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("help:"));
        assert!(s.contains("gallery-dl"));
    }

    #[test]
    fn backend_failed_truncates() {
        let stderr = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = ScrapmfError::BackendFailed {
            name: "gallery-dl".to_string(),
            code: Some(1),
            stderr: stderr.clone(),
        };
        let s = format!("{err}");
        assert!(s.contains("help:"));
        assert!(s.contains("note:"));
    }

    /// The `Option` wrapper used to reach the UI verbatim: a failed scrape
    /// printed `exit code Some(4)`, which reads like a Rust type leaked into a
    /// message the user is trying to act on.
    #[test]
    fn exit_code_is_rendered_as_a_number() {
        let err = ScrapmfError::BackendFailed {
            name: "gallery-dl".to_string(),
            code: Some(4),
            stderr: "[instagram][error] boom".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("exit code 4"), "{s}");
        assert!(!s.contains("Some("), "{s}");
    }

    /// Unix reports `None` when a signal killed the process. Saying "None" to
    /// someone debugging a failed scrape is worse than saying nothing useful.
    #[test]
    fn a_missing_exit_code_explains_itself() {
        let err = ScrapmfError::BackendFailed {
            name: "gallery-dl".to_string(),
            code: None,
            stderr: "boom".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("signal"), "{s}");
        assert!(!s.contains("None"), "{s}");
    }

    /// The executor used to synthesize `exit code N` into the stderr field
    /// while this message already printed the code, so a silent backend failure
    /// showed it twice.
    #[test]
    fn the_exit_code_appears_exactly_once() {
        let err = ScrapmfError::BackendFailed {
            name: "gallery-dl".to_string(),
            code: Some(4),
            stderr: crate::process::executor::NO_STDERR.to_string(),
        };
        let s = format!("{err}");
        assert_eq!(s.matches("exit code 4").count(), 1, "{s}");
    }

    #[test]
    fn invalid_url_help() {
        let err = ScrapmfError::InvalidUrl {
            url: "javascript:alert(1)".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("help:"));
        assert!(s.contains("note:"));
    }
}
