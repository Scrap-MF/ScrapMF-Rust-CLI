use std::ffi::OsString;
use std::path::PathBuf;

use super::{Provider, ScrapeRequest};

/// Escape a string for embedding inside a gallery-dl `-o key=[...]` JSON value.
///
/// Double quotes and ASCII control characters are escaped (a literal newline
/// or tab inside a TOML multi-line string would otherwise produce JSON that
/// gallery-dl's strict decoder rejects with a cryptic error). Backslashes are
/// intentionally left as-is: sequences like `\f` must survive so gallery-dl's
/// strict-JSON decode converts them to the form-feed control char used by its
/// f-string formatter prefix (`\fF ...` → FStringFormatter). Escaping them
/// would break the formatter prefix.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Convert a toml::Value to its JSON representation for `-o` values.
/// gallery-dl parses option values with strict JSON — raw TOML Display
/// output (`key = value`) would fail parsing.
fn toml_value_to_json(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{}\"", json_escape(s)),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(dt) => format!("\"{}\"", json_escape(&dt.to_string())),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(toml_value_to_json).collect();
            format!("[{}]", items.join(","))
        }
        toml::Value::Table(map) => {
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, val)| format!("\"{}\":{}", json_escape(k), toml_value_to_json(val)))
                .collect();
            format!("{{{}}}", pairs.join(","))
        }
    }
}

pub struct GalleryDl;

impl GalleryDl {
    /// Resolved binary path (bundled pinned copy by default).
    pub fn binary() -> anyhow::Result<PathBuf> {
        crate::application::backend::gallery_dl_executable()
    }
}

impl Provider for GalleryDl {
    fn name(&self) -> &str {
        "gallery-dl"
    }

    fn is_available(&self) -> bool {
        crate::application::backend::gallery_dl_executable().is_ok()
    }

    fn version(&self) -> anyhow::Result<String> {
        // Checked execution: fails clearly if the binary exits non-zero
        let exe = Self::binary()?;
        let output = crate::process::Executor::run_capturing(
            &exe.to_string_lossy(),
            &[std::ffi::OsString::from("--version")],
        )?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn build_args(&self, req: &ScrapeRequest) -> anyhow::Result<Vec<OsString>> {
        let mut args = Vec::with_capacity(32 + req.extractor_options.len() * 2);
        // Cookies handling (anti-bot) — file and browser
        if let Some(ref file) = req.cookies_file {
            args.push(OsString::from("--cookies"));
            args.push(file.as_os_str().to_owned());
        }
        if let Some(ref browser) = req.cookies_from_browser {
            args.push(OsString::from("--cookies-from-browser"));
            args.push(OsString::from(browser));
        }
        // Archive — only if explicitly set (removed by default to avoid skipping posts)
        if let Some(ref archive) = req.archive {
            args.push(OsString::from("--download-archive"));
            args.push(archive.as_os_str().to_owned());
        }
        // Without archive, gallery-dl skips existing files by default (unless --no-skip)
        // Deterministic filenames ({date}_{post_id}/{id}) ensure idempotency without SQLite
        // Rate limiting: sleep, sleep-request, sleep-429 (support ranges like "3-6")
        if let Some(ref rl) = req.rate_limit {
            if let Some(ref s) = rl.sleep {
                args.push(OsString::from("--sleep"));
                args.push(OsString::from(s));
            }
            if let Some(ref sr) = rl.sleep_request {
                args.push(OsString::from("--sleep-request"));
                args.push(OsString::from(sr));
            }
            if let Some(s429) = rl.sleep_429 {
                args.push(OsString::from("--sleep-429"));
                args.push(OsString::from(s429.to_string()));
            }
            if let Some(ref lr) = rl.limit_rate {
                args.push(OsString::from("--limit-rate"));
                args.push(OsString::from(lr));
            }
        }
        if let Some(out) = &req.output {
            let expanded = crate::config::expand_output_dir(out);
            args.push(OsString::from("--destination"));
            args.push(expanded.as_os_str().to_owned());
        }
        // Filename and directory templates (stable IDs + date)
        // Use extractor.<site>.filename/directory when preset (site) is known, so that
        // extractor-specific overrides (e.g. extractor.instagram.highlights) can correctly
        // override the general instagram template. Top-level directory/filename would otherwise
        // take precedence and prevent highlights subfolders (example_user/highlights/...) from being created.
        if let Some(ref tmpl) = req.filename_template {
            let key = if let Some(ref site) = req.preset {
                format!("extractor.{site}.filename")
            } else {
                "filename".to_string()
            };
            args.push(OsString::from("-o"));
            args.push(OsString::from(format!("{key}={tmpl}")));
        }
        if let Some(ref dirs) = req.directory_template {
            // directory = ["{username}", "{category}"] etc. Serialize as gallery-dl expects
            let arr_str = dirs
                .iter()
                .map(|s| format!("\"{}\"", json_escape(s)))
                .collect::<Vec<_>>()
                .join(",");
            let key = if let Some(ref site) = req.preset {
                format!("extractor.{site}.directory")
            } else {
                "directory".to_string()
            };
            args.push(OsString::from("-o"));
            args.push(OsString::from(format!("{key}=[{arr_str}]")));
        }
        // Path sanitization — use gallery-dl native mechanism
        // Ensure restricted filenames (sanitize highlight_title etc.)
        let has_restrict = req.extra_args.iter().any(|a| a.contains("restrict"));
        if !has_restrict {
            args.push(OsString::from("--restrict-filenames"));
            args.push(OsString::from("auto"));
        }
        // Inject profile root keyword: directory templates reference {scrapmf_root}
        // as first segment (profile/network/account/content structure).
        // Prefer the explicit profile name (profile flows + quick where root
        // is the username). For direct CLI scrapes without a profile, derive
        // the username from the URL so the tree shows <username> instead of
        // the generic "default" placeholder.
        let scrapmf_root = crate::application::scraper::scrapmf_root(req);
        // Nombre nuevo + legacy: configs de usuarios pre-renombre usan
        // {scarpmf_root} en sus plantillas y deben seguir resolviendo.
        // REGRESSION NOTE: each -o needs its OWN flag. The legacy alias below
        // used to be pushed without "-o", so argparse read it as the URL
        // positional and rejected the real URL with "unrecognized arguments"
        // — every scrape failed with exit code 2.
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "extractor.keywords.scrapmf_root={scrapmf_root}"
        )));
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "extractor.keywords.scarpmf_root={scrapmf_root}"
        )));
        // Extractor options via -o key=value (supports nested tables for sub-extractors)
        // gallery-dl expects extractor.<category>.<subcategory>.<key>, e.g. extractor.instagram.highlights.directory
        // _cfgpath = ("extractor", category, subcategory) per extractor/common.py
        for (k, v) in &req.extractor_options {
            // k is like "instagram:highlights" or "instagram" — split on ':' to get nested path
            let prefixed_k = if k.contains(':') {
                let dotted = k.replace(':', ".");
                format!("extractor.{dotted}")
            } else {
                format!("extractor.{k}")
            };
            match v {
                toml::Value::Table(map) => {
                    for (subk, subv) in map {
                        let val_str = match subv {
                            toml::Value::String(s) => format!("{prefixed_k}.{subk}={s}"),
                            toml::Value::Array(arr) => {
                                let arr_str = arr
                                    .iter()
                                    .map(|val| match val {
                                        toml::Value::String(s) => format!("\"{}\"", json_escape(s)),
                                        _ => val.to_string(),
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",");
                                format!("{prefixed_k}.{subk}=[{arr_str}]")
                            }
                            _ => format!("{prefixed_k}.{subk}={}", toml_value_to_json(subv)),
                        };
                        args.push(OsString::from("-o"));
                        args.push(OsString::from(val_str));
                    }
                }
                toml::Value::String(s) => {
                    args.push(OsString::from("-o"));
                    args.push(OsString::from(format!("{prefixed_k}={s}")));
                }
                toml::Value::Array(arr) => {
                    let arr_str = arr
                        .iter()
                        .map(|val| match val {
                            toml::Value::String(s) => format!("\"{}\"", json_escape(s)),
                            _ => val.to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    args.push(OsString::from("-o"));
                    args.push(OsString::from(format!("{prefixed_k}=[{arr_str}]")));
                }
                _ => {
                    args.push(OsString::from("-o"));
                    args.push(OsString::from(format!("{prefixed_k}={v}")));
                }
            }
        }
        for extra in &req.extra_args {
            args.push(OsString::from(extra));
        }
        // Extra -o overrides from scrapmf's own logic (e.g. twitter two-pass
        // photos/videos split). Emitted after site options so they win.
        for (k, v) in &req.extra_extractor_opts {
            args.push(OsString::from("-o"));
            args.push(OsString::from(format!("{k}={v}")));
        }
        args.push(OsString::from(&req.url));
        Ok(args)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{GalleryDl, json_escape, toml_value_to_json};
    use crate::application::ScrapeRequest;
    use crate::providers::Provider;
    use std::path::PathBuf;

    #[test]
    fn json_escape_quotes_and_control_chars() {
        assert_eq!(json_escape("plain"), "plain");
        assert_eq!(json_escape("say \"hi\""), "say \\\"hi\\\"");
        // Control characters that would break strict-JSON parsing
        assert_eq!(json_escape("line1\nline2"), "line1\\nline2");
        assert_eq!(json_escape("a\tb"), "a\\tb");
        assert_eq!(json_escape("cr\r"), "cr\\r");
        assert_eq!(json_escape("\u{01}"), "\\u0001");
        // UTF-8 passes through untouched
        assert_eq!(json_escape("español ✓ 日本語"), "español ✓ 日本語");
    }

    /// Structural invariant: every `-o` VALUE must directly follow its own
    /// `-o` flag, and the URL must be the LAST argument. Regression guard for
    /// the scarpmf_root legacy alias that used to be pushed without "-o" —
    /// argparse then read it as the URL positional and every scrape died with
    /// "unrecognized arguments: <url>" (exit code 2).
    #[test]
    fn every_option_value_has_its_own_o_flag_and_url_is_last() {
        use crate::providers::Provider;
        let req = ScrapeRequest {
            url: "https://example.com/gallery".to_string(),
            output: None,
            preset: None,
            extra_args: vec![],
            cookies_from_browser: None,
            cookies_file: None,
            archive: None,
            rate_limit: None,
            extractor_options: Default::default(),
            filename_template: None,
            directory_template: None,
            extra_urls: vec![],
            profile_name: Some("example".to_string()),
            extra_extractor_opts: vec![],

            ..Default::default()
        };
        let args = GalleryDl.build_args(&req).unwrap();
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for (i, arg) in strs.iter().enumerate().skip(1) {
            if arg.starts_with("extractor.") && !arg.contains('=') {
                continue; // bare keys are not emitted by build_args today
            }
            if arg.contains('=') && arg.starts_with("extractor.") {
                assert_eq!(
                    strs[i - 1],
                    "-o",
                    "option '{arg}' at index {i} lacks its own '-o' flag"
                );
            }
        }
        assert!(
            strs.last().is_some_and(|last| last.contains("https://")),
            "URL must be the final argv element, got {strs:?}"
        );
    }

    #[test]
    fn json_escape_preserves_fstring_prefix_backslashes() {
        // The documented contract: backslash sequences must survive verbatim
        // because gallery-dl uses them for its f-string formatter prefix.
        assert_eq!(
            json_escape("\\fF {post_id}"),
            "\\fF {post_id}",
            "escaping backslashes would break the f-string prefix"
        );
    }

    #[test]
    fn toml_value_to_json_nested_structures() {
        use toml::Value;
        let arr = Value::Array(vec![Value::String("a\"b".into()), Value::Integer(3)]);
        assert_eq!(toml_value_to_json(&arr), "[\"a\\\"b\",3]");
        let mut map = toml::map::Map::new();
        map.insert("k".into(), Value::String("x\ny".into()));
        map.insert("n".into(), Value::Boolean(true));
        let out = toml_value_to_json(&Value::Table(map));
        // Table ordering is alphabetical in toml::Map: k before n
        assert_eq!(out, "{\"k\":\"x\\ny\",\"n\":true}");
    }
    fn req(url: &str) -> ScrapeRequest {
        ScrapeRequest {
            url: url.to_string(),
            output: None,
            preset: None,
            extra_args: Vec::new(),
            cookies_from_browser: None,
            cookies_file: None,
            archive: None,
            rate_limit: None,
            extractor_options: std::collections::HashMap::new(),
            filename_template: None,
            directory_template: None,
            extra_urls: Vec::new(),
            profile_name: None,
            extra_extractor_opts: Vec::new(),
            no_archive: false,
            profile_pic_only: false,
        }
    }

    #[test]
    fn build_args_no_output() {
        let g = GalleryDl;
        let r = req("https://example.com");
        let args = g.build_args(&r).expect("build");
        assert_eq!(
            args.last().unwrap().to_string_lossy(),
            "https://example.com"
        );
        assert!(!args.iter().any(|a| a == "--destination"));
    }

    #[test]
    fn build_args_with_output() {
        let g = GalleryDl;
        let mut r = req("https://example.com");
        r.output = Some(PathBuf::from("/tmp/out"));
        let args = g.build_args(&r).expect("build");
        assert!(args.iter().any(|a| a == "--destination"));
        assert!(args.iter().any(|a| a == "/tmp/out"));
    }

    #[test]
    fn build_args_with_extra_and_cookies() {
        let g = GalleryDl;
        let mut r = req("https://example.com");
        r.extra_args = vec!["--sleep".to_string(), "1".to_string()];
        r.cookies_from_browser = Some("firefox".to_string());
        r.cookies_file = Some(PathBuf::from("/tmp/cookies.txt"));
        let args = g.build_args(&r).expect("build");
        assert!(args.iter().any(|a| a == "--cookies"));
        assert!(args.iter().any(|a| a == "--cookies-from-browser"));
        assert!(args.iter().any(|a| a == "firefox"));
        assert!(args.iter().any(|a| a == "--sleep"));
    }

    #[test]
    fn build_args_order() {
        let g = GalleryDl;
        let mut r = req("https://example.com");
        r.output = Some(PathBuf::from("/tmp/out"));
        r.cookies_from_browser = Some("firefox".to_string());
        let args = g.build_args(&r).expect("build");
        // cookies before destination before url
        let pos_cookies = args
            .iter()
            .position(|a| a == "--cookies-from-browser")
            .unwrap();
        let pos_dest = args.iter().position(|a| a == "--destination").unwrap();
        let pos_url = args
            .iter()
            .position(|a| a == "https://example.com")
            .unwrap();
        assert!(pos_cookies < pos_dest);
        assert!(pos_dest < pos_url);
    }

    #[test]
    fn build_args_injects_profile_root_keyword() {
        let g = GalleryDl;
        let mut r = req("https://example.com");
        r.profile_name = Some("example".to_string());
        let args = g.build_args(&r).expect("build");
        assert!(
            args.iter()
                .any(|a| a.to_string_lossy() == "extractor.keywords.scrapmf_root=example")
        );
    }

    #[test]
    fn build_args_injects_default_root_without_profile() {
        let g = GalleryDl;
        let args = g.build_args(&req("https://example.com")).expect("build");
        // example.com has no known site_account, so fallback remains "default"
        assert!(
            args.iter()
                .any(|a| a.to_string_lossy() == "extractor.keywords.scrapmf_root=default")
        );
    }

    #[test]
    fn build_args_derives_username_when_no_profile_but_known_site() {
        let g = GalleryDl;
        let mut r = req("https://www.instagram.com/someuser/");
        r.profile_name = None;
        let args = g.build_args(&r).expect("build");
        assert!(
            args.iter()
                .any(|a| a.to_string_lossy() == "extractor.keywords.scrapmf_root=someuser"),
            "known site without explicit profile should use URL username, not default"
        );
    }
}
