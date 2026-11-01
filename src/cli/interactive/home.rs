//! Ranger/yazi-style home screen built on the shared [`crate::cli::interact
//! ive::browser`] — left pane = navigation, right pane = live details of the
//! highlighted entry.
//!
//! Enter leaves the alternate screen and dispatches to the existing inquire
//! flows; returning from a flow re-opens this browser. No mouse capture —
//! text selection stays available.

use crate::cli::interactive::browser::{Browser, Mode, Outcome};

/// Top-level entries of the home browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Scrape,
    Configuration,
    Plugins,
    Doctor,
    Exit,
}

fn entries() -> Vec<(Action, String)> {
    vec![
        (Action::Scrape, crate::i18n::t("download_content")),
        (Action::Configuration, crate::i18n::t("configuration")),
        (Action::Plugins, crate::i18n::t("plugins")),
        (Action::Doctor, crate::i18n::t("doctor")),
        (Action::Exit, crate::i18n::t("exit")),
    ]
}

fn action_of(idx: usize) -> Action {
    let e = entries();
    e[idx.min(e.len() - 1)].0
}

/// Detail lines for the right pane. Live data where it is cheap.
fn detail_lines(action: Action) -> Vec<String> {
    match action {
        Action::Scrape => {
            let sites = super::scrape_flow::site_options_with_fallbacks(&[]);
            let mut lines = vec![
                crate::i18n::t("run_scrapes").to_string(),
                String::new(),
                crate::i18n::t("detected_sites").to_string(),
            ];
            lines.extend(sites.iter().map(|s| format!("  · {s}")));
            lines
        }
        Action::Configuration => {
            let cfg = crate::config::load().unwrap_or_default();
            vec![
                crate::i18n::t("edit_how").to_string(),
                String::new(),
                crate::i18n::t("output_dir_label").replace(
                    "{}",
                    &crate::config::expand_output_dir(&cfg.general.output_dir)
                        .display()
                        .to_string(),
                ),
                format!(
                    "archive    : {}",
                    if cfg.general.archive {
                        crate::i18n::t("on")
                    } else {
                        crate::i18n::t("off")
                    }
                ),
                format!(
                    "plugins    : threads {}",
                    match crate::plugins::threads_state() {
                        crate::plugins::PluginState::Enabled(v) => format!("enabled ({v})"),
                        crate::plugins::PluginState::Disabled =>
                            crate::i18n::t("disabled").to_string(),
                        crate::plugins::PluginState::NotInstalled =>
                            crate::i18n::t("not_installed").to_string(),
                    }
                ),
                String::new(),
                crate::i18n::t("sites_profiles_managed").to_string(),
            ]
        }
        Action::Plugins => {
            let mut lines = vec![
                crate::i18n::t("optional_site_providers").to_string(),
                crate::i18n::t("own_managed_env").to_string(),
                String::new(),
            ];
            for def in crate::plugins::REGISTRY {
                let status = match def.id {
                    "threads" => match crate::plugins::threads_state() {
                        crate::plugins::PluginState::Enabled(v) => format!("enabled ({v})"),
                        crate::plugins::PluginState::Disabled => {
                            crate::i18n::t("disabled_files_kept").to_string()
                        }
                        crate::plugins::PluginState::NotInstalled => {
                            crate::i18n::t("not_installed").to_string()
                        }
                    },
                    "termux-scan" => match crate::plugins::termux_scan_state() {
                        crate::plugins::PluginState::Enabled(v) => format!("enabled ({v})"),
                        crate::plugins::PluginState::Disabled => {
                            crate::i18n::t("disabled").to_string()
                        }
                        crate::plugins::PluginState::NotInstalled => {
                            crate::i18n::t("not_installed").to_string()
                        }
                    },
                    _ => crate::i18n::t("not_installed").to_string(),
                };
                lines.push(format!("  · {} by {}: {}", def.title, def.vendor, status));
            }
            lines
        }
        Action::Doctor => vec![
            crate::i18n::t("health_check").to_string(),
            String::new(),
            crate::i18n::t("doctor_detail_backend").to_string(),
            crate::i18n::t("doctor_detail_plugins").to_string(),
            crate::i18n::t("doctor_detail_browsers").to_string(),
            crate::i18n::t("doctor_detail_archive").to_string(),
            crate::i18n::t("doctor_detail_tempdir").to_string(),
        ],
        Action::Exit => vec![crate::i18n::t("leave_scrapmf").to_string()],
    }
}

/// Open the home browser. Returns the chosen action, or `None` when the user
/// quits (`q`/`Esc`/Ctrl-C). The alternate screen is entered/exited inside
/// the shared browser.
pub(crate) fn pick() -> Option<Action> {
    let title = format!("SCRAPMF v{}", env!("CARGO_PKG_VERSION"));
    let mut b = Browser::new(title).mode(Mode::Single);
    for (action, label) in entries() {
        b = b.entry(label, detail_lines(action));
    }
    match b.run() {
        Outcome::Picked(i) => Some(action_of(i)),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_stable_and_complete() {
        let e = entries();
        assert_eq!(e.len(), 5);
        assert_eq!(e[0].1, crate::i18n::t("download_content"));
        assert_eq!(e.last().unwrap().1, crate::i18n::t("exit"));
    }

    #[test]
    fn action_of_clamps_index() {
        assert_eq!(action_of(0), Action::Scrape);
        assert_eq!(action_of(99), Action::Exit);
    }

    #[test]
    fn details_never_empty_for_any_action() {
        for a in [
            Action::Scrape,
            Action::Configuration,
            Action::Plugins,
            Action::Doctor,
            Action::Exit,
        ] {
            assert!(!detail_lines(a).is_empty());
        }
    }

    #[test]
    fn pick_maps_index_to_action() {
        // pick() itself needs a TTY; verify the mapping helper instead.
        assert_eq!(action_of(1), Action::Configuration);
        assert_eq!(action_of(3), Action::Doctor);
    }
}
