use std::ffi::OsString;
use std::path::PathBuf;

use super::{Provider, ScrapeRequest};

/// First line of `--help` that carries text, or `"threadstractor"`.
///
/// Separate from the process call so it is testable without the binary.
fn version_line(help_out: &str) -> String {
    help_out
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("threadstractor")
        .to_string()
}

/// Provider for Threads via the `threadstractor` Python package.
/// Mirrors gallery-dl provider but calls the threadstractor binary
/// which handles `post_id` naming, `date` sorting and anti rate-limit.
pub struct Threadstractor;

impl Threadstractor {
    pub fn binary() -> anyhow::Result<PathBuf> {
        // Resolution order (mirrors gallery-dl backend): env override >
        // managed plugin venv > PATH. Error carries actionable hints.
        crate::plugins::resolve_binary()
    }

    fn binary_with_fallback() -> anyhow::Result<(String, Vec<OsString>)> {
        let path = Self::binary()?;
        Ok((path.to_string_lossy().into_owned(), vec![]))
    }
}

impl Provider for Threadstractor {
    fn name(&self) -> &str {
        "threadstractormf"
    }

    fn is_available(&self) -> bool {
        crate::plugins::threads_enabled()
    }

    fn version(&self) -> anyhow::Result<String> {
        let (bin, prefix) = Self::binary_with_fallback()?;

        // Prefer `--version`. It was added after the pin moved to v1.2.0, so the
        // fallback is not dead code: an older install answers with an error.
        let mut vargs = prefix.clone();
        vargs.push(OsString::from("--version"));
        if let Ok(out) = crate::process::Executor::run_capturing(&bin, &vargs) {
            let v = String::from_utf8_lossy(&out.stdout);
            let line = v.lines().map(str::trim).find(|l| !l.is_empty());
            if let Some(line) = line {
                return Ok(line.to_string());
            }
        }

        let mut args = prefix;
        args.push(OsString::from("--help"));
        let output = crate::process::Executor::run_capturing(&bin, &args)?;
        // The help output is a `rich` panel that opens with a blank line of
        // padding, so the first line is whitespace. Take the first line with
        // something on it instead of the first line.
        let out = String::from_utf8_lossy(&output.stdout);
        Ok(version_line(&out))
    }

    #[allow(clippy::collapsible_if)]
    fn build_args(&self, req: &ScrapeRequest) -> anyhow::Result<Vec<OsString>> {
        let mut args = Vec::with_capacity(16);

        // Cookies (same as gallery-dl)
        if let Some(ref file) = req.cookies_file {
            args.push(OsString::from("--cookies"));
            args.push(file.as_os_str().to_owned());
        }
        if let Some(ref browser) = req.cookies_from_browser {
            args.push(OsString::from("--cookies-from-browser"));
            args.push(OsString::from(browser));
        }

        if req.profile_pic_only {
            args.push(OsString::from("--profile-pic-only"));
        }

        // Archive: the plugin grew its own ledger (`--archive`, v1.2.0). scrapmf
        // keeps ownership of the canonical record and bridges the two — see
        // `crate::application::threads_archive` — so enabling it here is what
        // makes the plugin consult and update its ledger.
        if req.archive.is_some() {
            args.push(OsString::from("--archive"));
        }

        // Rate limit -> threadstractor flags
        if let Some(ref rl) = req.rate_limit {
            if let Some(ref s) = rl.sleep {
                // sleep "3-6" range -> take lower bound for cooldown ms
                if let Some(ms) = parse_sleep_to_ms(s) {
                    args.push(OsString::from("--cooldown"));
                    args.push(OsString::from(ms.to_string()));
                }
            }
            if let Some(ref sr) = rl.sleep_request {
                if let Some(ms) = parse_sleep_to_ms(sr) {
                    // map sleep_request to rps ~ 1000/ms
                    let rps = if ms > 0 { 1000.0 / ms as f64 } else { 0.5 };
                    args.push(OsString::from("--rps"));
                    args.push(OsString::from(format!("{rps:.2}")));
                }
            }
            if let Some(s429) = rl.sleep_429 {
                // batch cooldown approx
                let ms = s429 as u64 * 1000;
                args.push(OsString::from("--batch-cooldown"));
                args.push(OsString::from(ms.to_string()));
            }
            if let Some(ref lr) = rl.limit_rate {
                // not yet mapped, ignore
                let _ = lr;
            }
        }

        if let Some(out) = &req.output {
            let expanded = crate::config::expand_output_dir(out);
            args.push(OsString::from("--dest"));
            args.push(expanded.as_os_str().to_owned());
        }

        // Filename / directory templates: threadstractor supports templating via
        // --filename-template and --directory-template (f-string style).
        // Resolve {scrapmf_root}/{scarpmf_root} literals before passing to
        // the Python binary. Profile requests must not fall back to "default":
        // quick uses the username as root, profile uses the profile name.
        // This mirrors gallery-dl's extractor.keywords.scrapmf_root injection
        // but as a literal replacement so threadstractor never sees the placeholder.
        let resolved_root = req
            .profile_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                // Fallback for direct CLI scrapes without a profile: derive
                // username from the URL so quick still shows <username> and
                // never "default".
                crate::application::archive::site_account_from_url(&req.url)
                    .map(|(_, account)| account)
            });
        let resolve_dirs = |dirs: &[String]| -> String {
            dirs.iter()
                .map(|s| match s.as_str() {
                    "{scrapmf_root}" | "{scarpmf_root}" => resolved_root
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join("/")
        };
        if req.profile_pic_only {
            args.push(OsString::from("--filename-template"));
            args.push(OsString::from("{username}_profile.{extension}"));
            let use_caller = req
                .directory_template
                .as_ref()
                .is_some_and(|d| d.iter().any(|s| s.contains("profile")));
            if use_caller {
                if let Some(ref dirs) = req.directory_template {
                    let joined = resolve_dirs(dirs);
                    args.push(OsString::from("--directory-template"));
                    args.push(OsString::from(joined));
                }
            } else {
                // No caller dirs → build literal root + profile suffix
                let root = resolved_root
                    .clone()
                    .unwrap_or_else(|| "default".to_string());
                args.push(OsString::from("--directory-template"));
                args.push(OsString::from(format!("{root}/profile")));
            }
        } else {
            if let Some(ref tmpl) = req.filename_template {
                args.push(OsString::from("--filename-template"));
                args.push(OsString::from(tmpl));
            }
            if let Some(ref dirs) = req.directory_template {
                let joined = resolve_dirs(dirs);
                args.push(OsString::from("--directory-template"));
                args.push(OsString::from(joined));
            }
        }

        // Extractor options for threads: allow overriding filename_template via -o
        // For now, extractor_options are ignored except filename/directory which are already handled.

        // Extra args (allow-list validated) — filter gallery-dl-only flags
        let mut skip_next = false;
        for extra in &req.extra_args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if extra == "--restrict-filenames" {
                skip_next = true; // skip its value (auto)
                continue;
            }
            // gallery-dl flag that threadstractor doesn't need (already handled via rate_limit mapping)
            if extra == "--sleep"
                || extra == "--sleep-request"
                || extra == "--sleep-429"
                || extra == "--limit-rate"
            {
                skip_next = true;
                continue;
            }
            args.push(OsString::from(extra));
        }

        // Target URL / @username — threadstractor accepts @user or URL
        args.push(OsString::from(&req.url));
        Ok(args)
    }
}

fn parse_sleep_to_ms(s: &str) -> Option<u64> {
    // "3-6" -> take average or lower bound (we use lower for safety)
    // "5" -> 5000
    let first = s.split('-').next()?.trim();
    let secs: f64 = first.parse().ok()?;
    Some((secs * 1000.0) as u64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{Threadstractor, version_line};
    use crate::application::ScrapeRequest;
    use crate::providers::Provider;

    #[test]
    fn version_line_skips_the_padding_line() {
        // The plugin's `--help` is a `rich` panel that starts with a line of
        // spaces. Reading the first line verbatim yielded an empty version.
        let help = "                    \n Usage: threadstractormf [OPTIONS]\n";
        assert_eq!(version_line(help), "Usage: threadstractormf [OPTIONS]");
        assert_eq!(version_line("\n\n  \n"), "threadstractor");
    }

    #[test]
    fn archive_flag_follows_the_request() {
        let mut r = req("https://www.threads.net/@user/media");
        assert!(
            !Threadstractor
                .build_args(&r)
                .unwrap()
                .iter()
                .any(|a| a == "--archive"),
            "no archive requested, no ledger written"
        );
        r.archive = Some(std::path::PathBuf::from("/tmp/cache.sqlite"));
        assert!(
            Threadstractor
                .build_args(&r)
                .unwrap()
                .iter()
                .any(|a| a == "--archive"),
            "archive requested, the plugin must keep its ledger"
        );
    }

    fn req(url: &str) -> ScrapeRequest {
        ScrapeRequest {
            url: url.to_string(),
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
            profile_name: None,
            extra_extractor_opts: vec![],
            no_archive: false,
            profile_pic_only: false,
        }
    }

    #[test]
    fn build_args_with_templates() {
        let mut r = req("https://www.threads.com/@user");
        r.filename_template = Some("{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}".to_string());
        r.directory_template = Some(vec!["{scrapmf_root}".to_string(), "{category}".to_string()]);
        let args = Threadstractor.build_args(&r).unwrap();
        assert!(args.iter().any(|a| a == "--filename-template"));
        assert!(args.iter().any(|a| a.to_string_lossy().contains("{date:")));
        assert!(args.iter().any(|a| a == "--directory-template"));
    }

    #[test]
    fn build_args_no_templates() {
        let r = req("https://www.threads.com/@user");
        let args = Threadstractor.build_args(&r).unwrap();
        assert!(!args.iter().any(|a| a == "--filename-template"));
    }

    #[test]
    fn build_args_resolves_scrapmf_root_to_profile_name() {
        let mut r = req("https://www.threads.com/@someuser");
        r.profile_name = Some("myprofile".to_string());
        r.directory_template = Some(vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{username}".to_string(),
            "posts".to_string(),
        ]);
        let args = Threadstractor.build_args(&r).unwrap();
        let joined = args
            .windows(2)
            .find(|w| w[0] == "--directory-template")
            .map(|w| w[1].to_string_lossy().into_owned())
            .unwrap_or_default();
        assert!(
            joined.starts_with("myprofile/"),
            "profile root must be literal profile name, got {joined}"
        );
        assert!(!joined.contains("{scrapmf_root}"));
        assert!(!joined.contains("default"));
    }

    #[test]
    fn build_args_quick_resolves_root_to_username_literal() {
        let mut r = req("https://www.threads.com/@someuser");
        r.profile_name = Some("someuser".to_string());
        r.directory_template = Some(vec!["{scrapmf_root}".to_string(), "photos".to_string()]);
        let args = Threadstractor.build_args(&r).unwrap();
        let joined = args
            .windows(2)
            .find(|w| w[0] == "--directory-template")
            .map(|w| w[1].to_string_lossy().into_owned())
            .unwrap_or_default();
        assert_eq!(joined, "someuser/photos");
    }

    #[test]
    fn build_args_no_profile_fallback_is_username_from_url_not_default() {
        let mut r = req("https://www.threads.com/@deriveduser");
        // no profile_name set → fallback to URL account
        r.directory_template = Some(vec!["{scrapmf_root}".to_string(), "posts".to_string()]);
        let args = Threadstractor.build_args(&r).unwrap();
        let joined = args
            .windows(2)
            .find(|w| w[0] == "--directory-template")
            .map(|w| w[1].to_string_lossy().into_owned())
            .unwrap_or_default();
        assert_eq!(joined, "deriveduser/posts");
        assert!(!joined.contains("default"));
    }
}
