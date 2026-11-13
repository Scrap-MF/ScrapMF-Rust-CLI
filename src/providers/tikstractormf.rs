use std::ffi::OsString;
use std::path::PathBuf;

use super::{Provider, ScrapeRequest};

/// Provider for TikTok via the `tikstractormf` Python package.
pub struct Tikstractormf;

impl Tikstractormf {
    pub fn binary() -> anyhow::Result<PathBuf> {
        crate::plugins::resolve_tiktok_binary()
    }
}

impl Provider for Tikstractormf {
    fn name(&self) -> &str {
        "tikstractormf"
    }

    fn is_available(&self) -> bool {
        crate::plugins::tiktok_enabled()
    }

    fn version(&self) -> anyhow::Result<String> {
        let bin = Self::binary()?;
        let out = crate::process::Executor::run_capturing(
            &bin.to_string_lossy(),
            &[OsString::from("--version")],
        )?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn build_args(&self, req: &ScrapeRequest) -> anyhow::Result<Vec<OsString>> {
        let mut args = Vec::with_capacity(16);

        if let Some(ref file) = req.cookies_file {
            args.push(OsString::from("--cookies"));
            args.push(file.as_os_str().to_owned());
        }
        // Flatpak browsers cannot be read by browser-cookie3 (the plugin's
        // --cookies-from-browser): their runtime Python is not visible to the
        // host. scrapmf captures their cookies into a file upstream and clears
        // cookies_from_browser, so only forward this when it is NOT a Flatpak.
        if let Some(ref browser) = req.cookies_from_browser
            && !crate::config::cookies::is_flatpak_browser(browser)
        {
            args.push(OsString::from("--cookies-from-browser"));
            args.push(OsString::from(browser));
        }

        if req.profile_pic_only {
            args.push(OsString::from("--profile-pic-only"));
        }

        // Stories URL → only stories.
        if req.url.contains("/stories") {
            args.push(OsString::from("--only-stories"));
        }

        if req.archive.is_some() {
            args.push(OsString::from("--archive"));
        }

        if let Some(ref rl) = req.rate_limit {
            if let Some(ref s) = rl.sleep
                && let Some(ms) = parse_sleep_to_ms(s)
            {
                args.push(OsString::from("--cooldown"));
                args.push(OsString::from(ms.to_string()));
            }
            if let Some(ref sr) = rl.sleep_request
                && let Some(ms) = parse_sleep_to_ms(sr)
            {
                let rps = if ms > 0 { 1000.0 / ms as f64 } else { 0.5 };
                args.push(OsString::from("--rps"));
                args.push(OsString::from(format!("{rps:.2}")));
            }
            if let Some(s429) = rl.sleep_429 {
                let ms = s429 as u64 * 1000;
                args.push(OsString::from("--batch-cooldown"));
                args.push(OsString::from(ms.to_string()));
            }
        }

        if let Some(out) = &req.output {
            let expanded = crate::config::expand_output_dir(out);
            args.push(OsString::from("--dest"));
            args.push(expanded.as_os_str().to_owned());
        }

        // Templates: resolve {scrapmf_root}.
        let resolved_root = req
            .profile_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                crate::application::archive::site_account_from_url(&req.url).map(|(_, a)| a)
            });

        if let Some(ref tmpl) = req.filename_template {
            args.push(OsString::from("--filename-template"));
            args.push(OsString::from(tmpl));
        }
        if let Some(ref dirs) = req.directory_template {
            let joined = dirs
                .iter()
                .map(|s| match s.as_str() {
                    "{scrapmf_root}" | "{scarpmf_root}" => resolved_root
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join("/");
            args.push(OsString::from("--directory-template"));
            args.push(OsString::from(joined));
        }

        // Pass through validated extra args, dropping flags the tiktok plugin
        // does not understand (they come from gallery-dl site configs and
        // would otherwise abort the run with "No such option").
        let mut skip_next = false;
        for extra in &req.extra_args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if extra == "--restrict-filenames"
                || extra == "--retries"
                || extra == "--http-timeout"
                || extra == "--force-ipv4"
                || extra == "--proxy"
                || extra == "--user-agent"
                || extra == "--sleep"
                || extra == "--sleep-request"
                || extra == "--sleep-429"
                || extra == "--limit-rate"
                || extra == "--destination"
                || extra == "--get-urls"
            {
                if extra != "--force-ipv4" {
                    skip_next = true;
                }
                continue;
            }
            args.push(OsString::from(extra));
        }

        args.push(OsString::from(&req.url));
        Ok(args)
    }
}

fn parse_sleep_to_ms(s: &str) -> Option<u64> {
    let first = s.split('-').next()?.trim();
    let secs: f64 = first.parse().ok()?;
    Some((secs * 1000.0) as u64)
}
