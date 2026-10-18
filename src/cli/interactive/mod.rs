use inquire::Text;
use std::io::Write;

pub(super) fn clear_screen() {
    print!("\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();
    // Header eliminated — all menus now live inside the Browser box
    // `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`. No ▄▅▆▇ printed here.
}

/// Decorated select that always renders inside the Browser box
/// `╭ SCRAPMF vX.Y.Z ─ {prompt} ─╮`. Implements the same `.prompt()` API
/// as `inquire::Select` so all call sites automatically inherit the chrome
/// without needing `clear_screen` or `theme::render_config`.
pub struct DecoratedSelect<T> {
    prompt: String,
    details: Vec<String>,
    options: Vec<T>,
}

impl<T: std::fmt::Display + Clone> DecoratedSelect<T> {
    pub fn without_filtering(self) -> Self {
        self
    }
    pub fn with_help_message(self, _msg: &str) -> Self {
        self
    }
    pub fn with_render_config(self, _cfg: inquire::ui::RenderConfig<'static>) -> Self {
        self
    }
    pub fn with_default(self, _def: &T) -> Self {
        self
    }
    pub fn prompt(self) -> Result<T, inquire::InquireError> {
        let opts: Vec<(String, Vec<String>)> = self
            .options
            .iter()
            .map(|o| (o.to_string(), Vec::new()))
            .collect();
        let title = self.prompt.clone();
        let details = self.details;
        match crate::cli::interactive::menu::pick_single_details(&title, details, opts) {
            crate::cli::interactive::menu::Step::Value(idx) => Ok(self.options[idx].clone()),
            // Outside a multi-step flow Esc and Ctrl+C both mean "leave".
            crate::cli::interactive::menu::Step::Back
            | crate::cli::interactive::menu::Step::Cancel => {
                Err(inquire::InquireError::OperationCanceled)
            }
        }
    }
}

/// Standard option menu — now always decorated via Browser chrome.
/// Chain `.with_help_message()` / `.with_default()` remains no-op for compat.
pub(super) fn select_menu<T: std::fmt::Display + Clone>(
    prompt: &str,
    options: Vec<T>,
) -> DecoratedSelect<T> {
    DecoratedSelect {
        prompt: prompt.to_string(),
        details: Vec::new(),
        options,
    }
}

/// As [`select_menu`], but with lines shown above the options inside the
/// Browser chrome.
///
/// A `println!` header written before the menu opened is covered as soon as
/// the chrome renders, so the information never reached the screen. Passing it
/// as `details` puts it inside the box, where it stays visible.
pub(super) fn select_menu_with_details<T: std::fmt::Display + Clone>(
    prompt: &str,
    details: Vec<String>,
    options: Vec<T>,
) -> DecoratedSelect<T> {
    DecoratedSelect {
        prompt: prompt.to_string(),
        details,
        options,
    }
}

/// Run interactive prompt. All scrape flows (saved profile, URL(s), quick
/// scrape) run as internal batches via preview_and_execute. The home screen
/// is a ratatui browser; picking an entry drops to the plain terminal for
/// its inquire flow and returns to the browser afterwards.
/// One option of the "Download content" submenu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrapeEntry {
    SavedProfile,
    Urls,
    Quick,
    Tasks,
}

/// Index → entry. `None` past the end, so a stale index cannot run something.
fn scrape_entry_of(idx: usize) -> Option<ScrapeEntry> {
    match idx {
        0 => Some(ScrapeEntry::SavedProfile),
        1 => Some(ScrapeEntry::Urls),
        2 => Some(ScrapeEntry::Quick),
        3 => Some(ScrapeEntry::Tasks),
        _ => None,
    }
}

pub fn run() {
    use home::Action;

    loop {
        let Some(action) = home::pick() else {
            println!("bye");
            return;
        };
        match action {
            Action::Scrape => {
                use browser::{Browser, Outcome};
                // Named rather than positional: this list grows, and a bare
                // `Picked(3)` is the kind of dispatch that rots the moment an
                // entry is inserted in the middle.
                let outcome = Browser::new("Download content")
                    .entry(
                        "Saved profile",
                        vec![
                            "Run a saved scraping profile.".to_string(),
                            String::new(),
                            "Sites, cookies and output rules bundled".to_string(),
                            "for repeatable runs.".to_string(),
                        ],
                    )
                    .entry(
                        "URL(s)",
                        vec![
                            "Paste & go: one or more URLs.".to_string(),
                            "Site auto-detected per URL — no extra prompts.".to_string(),
                        ],
                    )
                    .entry(
                        "Quick scrape",
                        vec![
                            "One account, pick content kinds.".to_string(),
                            "Fastest path: username in, media out.".to_string(),
                        ],
                    )
                    .entry(
                        "Tasks",
                        vec![
                            "Saved batches: content chosen once, reuse it.".to_string(),
                            String::new(),
                            "One or more accounts, each with its own".to_string(),
                            "cookie session. Output under the task name.".to_string(),
                        ],
                    )
                    .run();
                let action = match outcome {
                    Outcome::Picked(i) => scrape_entry_of(i),
                    _ => None,
                };
                match action {
                    Some(ScrapeEntry::SavedProfile) => {
                        prompt_scrape_as_profile();
                        clear_screen();
                    }
                    // Single entry for 1..N URLs: paste, auto-match site by
                    // pattern, run — no per-run prompts ("paste and go").
                    Some(ScrapeEntry::Urls) => {
                        scrape_flow::prompt_scrape_direct_urls();
                        clear_screen();
                    }
                    Some(ScrapeEntry::Quick) => {
                        prompt_quick_scrape();
                        clear_screen();
                    }
                    Some(ScrapeEntry::Tasks) => {
                        tasks::tasks_menu();
                        clear_screen();
                    }
                    None => {}
                }
            }
            Action::Configuration => {
                configuration_submenu();
                clear_screen();
            }
            Action::Plugins => {
                plugins_menu::menu();
                clear_screen();
            }
            Action::Doctor => {
                doctor_view::show();
                clear_screen();
            }
            Action::Exit => {
                println!("bye");
                return;
            }
        }
    }
}

use profiles::prompt_scrape_as_profile;
use scrape_flow::prompt_quick_scrape;
use sites::configuration_submenu;

pub(crate) mod browser;
mod content;
pub(crate) mod doctor_view;
pub(crate) mod home;
pub mod menu;
pub(crate) mod plugins_menu;
mod profiles;
mod scrape_flow;
mod sites;
mod tasks;
pub(super) mod theme;

pub(super) fn ask_nonempty(prompt: &str) -> Option<String> {
    loop {
        let raw = Text::new(prompt).prompt().ok()?;
        let u = raw.trim().to_string();
        if !u.is_empty() {
            return Some(u);
        }
    }
}

/// Guided creation of a new profile: interactively collects accounts
/// (site, username) and returns a Profile ready to serialize.
/// Cookies source is NOT prompted — it lives in sites/*.toml and is
/// inherited via the account > site resolution precedence.
/// Site options come from sites/*.toml with instagram/tiktok fallbacks.
pub(super) fn edit_with_editor(path: &std::path::Path) {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    if which::which(&editor).is_err() {
        eprintln!("error: editor '{editor}' not found in $PATH");
        return;
    }
    match std::process::Command::new(&editor).arg(path).status() {
        Ok(s) if s.success() => {
            // validate
            if let Ok(s) = std::fs::read_to_string(path) {
                // try both Site and Profile parse, one should succeed
                if toml::from_str::<crate::config::Site>(&s).is_ok()
                    || toml::from_str::<crate::config::Profile>(&s).is_ok()
                    || toml::from_str::<crate::config::Config>(&s).is_ok()
                {
                    println!("✔ Saved {}", path.display());
                } else {
                    eprintln!("warn: file saved but TOML parse failed, check syntax");
                }
            }
        }
        Ok(s) => eprintln!("editor exited with {:?}", s.code()),
        Err(e) => eprintln!("error: launch editor {editor}: {e}"),
    }
}

/// Helper to check if we are in interactive TTY context.
pub fn is_interactive() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}
