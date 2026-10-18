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

const ENTRIES: &[(Action, &str)] = &[
    (Action::Scrape, "Download content"),
    (Action::Configuration, "Configuration"),
    (Action::Plugins, "Plugins"),
    (Action::Doctor, "Doctor"),
    (Action::Exit, "Exit"),
];

fn action_of(idx: usize) -> Action {
    ENTRIES[idx.min(ENTRIES.len() - 1)].0
}

/// Detail lines for the right pane. Live data where it is cheap.
fn detail_lines(action: Action) -> Vec<String> {
    match action {
        Action::Scrape => {
            let sites = super::scrape_flow::site_options_with_fallbacks(&[]);
            let mut lines = vec![
                "Run scrapes against accounts or URLs.".to_string(),
                String::new(),
                "Detected sites:".to_string(),
            ];
            lines.extend(sites.iter().map(|s| format!("  · {s}")));
            lines
        }
        Action::Configuration => {
            let cfg = crate::config::load().unwrap_or_default();
            vec![
                "Edit how scrapmf behaves.".to_string(),
                String::new(),
                format!(
                    "output dir : {}",
                    crate::config::expand_output_dir(&cfg.general.output_dir).display()
                ),
                format!(
                    "archive    : {}",
                    if cfg.general.archive { "on" } else { "off" }
                ),
                format!(
                    "plugins    : threads {}",
                    match crate::plugins::threads_state() {
                        crate::plugins::PluginState::Enabled(v) => format!("enabled ({v})"),
                        crate::plugins::PluginState::Disabled => "disabled".to_string(),
                        crate::plugins::PluginState::NotInstalled => "not installed".to_string(),
                    }
                ),
                String::new(),
                "Sites and profiles are managed inside.".to_string(),
            ]
        }
        Action::Plugins => {
            let mut lines = vec![
                "Optional site providers installed into scrapmf's".to_string(),
                "own managed environment (never system Python).".to_string(),
                String::new(),
            ];
            for def in crate::plugins::REGISTRY {
                let status = match def.id {
                    "threads" => match crate::plugins::threads_state() {
                        crate::plugins::PluginState::Enabled(v) => format!("enabled ({v})"),
                        crate::plugins::PluginState::Disabled => {
                            "disabled (files kept)".to_string()
                        }
                        crate::plugins::PluginState::NotInstalled => "not installed".to_string(),
                    },
                    "termux-scan" => match crate::plugins::termux_scan_state() {
                        crate::plugins::PluginState::Enabled(v) => format!("enabled ({v})"),
                        crate::plugins::PluginState::Disabled => "disabled".to_string(),
                        crate::plugins::PluginState::NotInstalled => "not installed".to_string(),
                    },
                    _ => "not installed".to_string(),
                };
                lines.push(format!("  · {} by {}: {}", def.title, def.vendor, status));
            }
            lines
        }
        Action::Doctor => vec![
            "Health check for backends and system:".to_string(),
            String::new(),
            "  · gallery-dl backend + pinned version".to_string(),
            "  · plugin availability".to_string(),
            "  · browser cookie DBs".to_string(),
            "  · download archive stats".to_string(),
            "  · temp dir writability".to_string(),
        ],
        Action::Exit => vec!["Leave scrapmf.".to_string()],
    }
}

/// Open the home browser. Returns the chosen action, or `None` when the user
/// quits (`q`/`Esc`/Ctrl-C). The alternate screen is entered/exited inside
/// the shared browser.
pub(crate) fn pick() -> Option<Action> {
    let title = format!("SCRAPMF v{}", env!("CARGO_PKG_VERSION"));
    let mut b = Browser::new(title).mode(Mode::Single);
    for (_, label) in ENTRIES {
        let action = action_of(
            ENTRIES
                .iter()
                .position(|(_, l)| l == label)
                .unwrap_or_default(),
        );
        b = b.entry(*label, detail_lines(action));
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
        assert_eq!(ENTRIES.len(), 5);
        assert_eq!(ENTRIES[0].1, "Download content");
        assert_eq!(ENTRIES.last().unwrap().1, "Exit");
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
