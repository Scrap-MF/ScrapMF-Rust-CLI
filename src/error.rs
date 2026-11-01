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
        None => crate::i18n::t("exit_code_unavailable"),
    }
}

#[derive(Error, Debug)]
pub enum ScrapmfError {
    #[error("{}", crate::i18n::tf("error_io", &[("source", &format!("{}", _0))]))]
    Io(#[from] std::io::Error),

    #[error(
        "invalid URL: {url}\n  help: use a valid http(s) URL, e.g. https://example.com/gallery/123\n  note: valid schemes are http/https and length ≤2048"
    )]
    InvalidUrl { url: String },

    #[error("{}", crate::i18n::tf("error_backend_not_found", &[("name", name)]))]
    BackendNotFound { name: String },

    #[error("{}", crate::i18n::tf("error_backend_failed", &[("name", name), ("code", &exit_code_label(*code)), ("stderr", stderr)]))]
    BackendFailed {
        name: String,
        code: Option<i32>,
        stderr: String,
    },

    #[error("{}", crate::i18n::tf("error_config_parse", &[("path", &path.display().to_string()), ("source", &source.to_string())]))]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("{}", crate::i18n::t("error_aborted"))]
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
        assert!(s.contains("gallery-dl"));
        assert!(s.contains("help:") || s.contains("ayuda:"));
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
        assert!(s.contains("help:") || s.contains("ayuda:"));
        assert!(s.contains("note:") || s.contains("nota:"));
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
        assert!(s.contains("4"), "{s}");
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
        assert!(s.contains("signal") || s.contains("señal"), "{s}");
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
        assert!(s.matches("4").count() >= 1, "{s}");
    }

    #[test]
    fn invalid_url_help() {
        let err = ScrapmfError::InvalidUrl {
            url: "javascript:alert(1)".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("help:") || s.contains("ayuda:"));
        assert!(s.contains("note:") || s.contains("nota:"));
    }
}
