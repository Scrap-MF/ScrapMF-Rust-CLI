//! Shared two-pane browser — the ranger/yazi-style navigator used by every
//! menu in scrapmf (home, scrape flows, configuration tree, plugins).
//!
//! Left pane: entries with a cursor (and checkboxes in Multi mode). Right
//! pane: live details of the highlighted entry. Footer: key hints only —
//! the caller's box title owns the identity (version, etc.).
//!
//! No mouse capture on purpose: text selection stays available.

use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph},
};

use crate::ui::dashboard::TerminalGuard;

/// One navigable row with its right-pane description.
#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub details: Vec<String>,
}

impl Entry {
    pub fn new(label: impl Into<String>, details: Vec<String>) -> Self {
        Self {
            label: label.into(),
            details,
        }
    }
}

/// Selection semantics of the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// ↑↓ move · Enter picks the highlighted entry.
    Single,
    /// ↑↓ move cursor · Space toggles under it · `a` all/none · Enter
    /// confirms returning every checked index.
    Multi,
}

/// Result of running a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Picked(usize),
    Toggled(Vec<usize>),
    /// The screen could not be used at all (no TTY, or too few entries).
    Quit,
    /// Esc / q — go back to the previous question, keeping earlier answers.
    Back,
    /// Ctrl+C — abandon the whole flow.
    Abandoned,
}

/// Fluent builder — see [`Browser::run`].
#[derive(Default)]
pub struct Browser {
    title: String,
    entries: Vec<Entry>,
    mode: Option<Mode>,
    checked: Vec<usize>,
    hint: Option<String>,
    /// Row the cursor starts on. A wizard that steps back wants to land on
    /// the answer the user just gave instead of the top of the list.
    cursor: Option<usize>,
    /// Lines above the entries — the subject of the menu. A caller that
    /// printed these with `println!` before opening the chrome lost them: the
    /// alternate screen covered them the moment it drew.
    header: Vec<String>,
}

impl Browser {
    /// Set the lines shown above the entries.
    pub fn header(mut self, lines: Vec<String>) -> Self {
        self.header = lines;
        self
    }

    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn entry(mut self, label: impl Into<String>, details: Vec<String>) -> Self {
        self.entries.push(Entry::new(label, details));
        self
    }

    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Key hint rendered under the list, so a screen that steps back on Esc
    /// says so instead of leaving the behaviour to be guessed.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Pre-checked indices (Multi only; ignored otherwise).
    // Used by the content-kinds / cookie-networks browsers (next phase).
    #[allow(dead_code)]
    pub fn checked(mut self, idxs: &[usize]) -> Self {
        self.checked = idxs.to_vec();
        self
    }

    /// Start the cursor on `idx`, clamped to the list. So a caller that steps
    /// back can re-open the question on the answer it already has.
    pub fn cursor_at(mut self, idx: usize) -> Self {
        self.cursor = Some(idx);
        self
    }

    pub fn run(self) -> Outcome {
        let mode = self.mode.unwrap_or(Mode::Single);
        run_browser(
            &self.title,
            &self.header,
            &self.entries,
            mode,
            &self.checked,
            self.hint.as_deref(),
            self.cursor.unwrap_or(0),
        )
    }
}

// ─── Pure helpers (unit-tested) ─────────────────────────────────────────────

/// Toggle `checked[idx]` in place.
fn toggle_at(checked: &mut [bool], idx: usize) {
    if let Some(v) = checked.get_mut(idx) {
        *v = !*v;
    }
}

/// Flip every flag toward "all on" when anything is off, otherwise all off.
/// Returns the resulting state.
fn toggle_all(checked: &mut [bool]) -> bool {
    let any_off = checked.iter().any(|v| !v);
    for v in checked.iter_mut() {
        *v = any_off;
    }
    any_off
}

/// Left gutter for one rendered row: cursor marker + checkbox (Multi only).
fn row_prefix(mode: Mode, is_cursor: bool, is_checked: bool) -> String {
    let cursor = if is_cursor { "▶" } else { " " };
    match mode {
        Mode::Single => format!("{cursor} "),
        Mode::Multi => {
            let check = if is_checked { "[x]" } else { "[ ]" };
            format!("{cursor}{check} ")
        }
    }
}

// ─── TUI ────────────────────────────────────────────────────────────────────

/// Decide what leaving the browser means, given how the loop ended.
///
/// Split out from the event loop so every combination is testable without a
/// TTY. This logic is the difference between "my selection was accepted" and
/// "the flow was abandoned", so it must not be reachable only by pressing real
/// keys.
fn outcome_for(
    confirmed: bool,
    went_back: bool,
    abandoned: bool,
    mode: Mode,
    cursor: usize,
    checked: &[bool],
) -> Outcome {
    if abandoned {
        return Outcome::Abandoned;
    }
    if went_back {
        return Outcome::Back;
    }
    if confirmed {
        return match mode {
            Mode::Single => Outcome::Picked(cursor),
            Mode::Multi => {
                let picked: Vec<usize> = checked
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.then_some(i))
                    .collect();
                // Enter on the highlighted row means "just this one", the same
                // as a single-select. An empty check list used to read as
                // "chose none", which every caller treats as aborting the run,
                // so Enter looked broken and Space was the only way in.
                if picked.is_empty() {
                    Outcome::Toggled(vec![cursor])
                } else {
                    Outcome::Toggled(picked)
                }
            }
        };
    }
    // No key ever settled the loop (no TTY, empty list): nothing was chosen.
    match mode {
        Mode::Single => Outcome::Quit,
        Mode::Multi => Outcome::Toggled(vec![]),
    }
}

/// Where the cursor starts, given what the caller asked for.
///
/// A caller that steps back re-opens the question on the answer it already
/// has, so it asks for a row index. That index can be stale by the time the
/// menu opens, and an empty list has no row at all — either way it must land
/// somewhere valid rather than index past the entries.
fn clamp_cursor(requested: usize, len: usize) -> usize {
    requested.min(len.saturating_sub(1))
}

fn run_browser(
    title: &str,
    header: &[String],
    entries: &[Entry],
    mode: Mode,
    prechecked: &[usize],
    hint: Option<&str>,
    initial_cursor: usize,
) -> Outcome {
    // A cancelled multi-select is an *empty* selection, not a quit: pressing
    // Esc while picking kinds means "I chose none", which is a legitimate
    // answer the caller must be able to distinguish from leaving the screen.
    let fallback = || match mode {
        Mode::Single => Outcome::Quit,
        Mode::Multi => Outcome::Toggled(vec![]),
    };
    if entries.is_empty() {
        return fallback();
    }

    let guard = match TerminalGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: cannot enter TUI mode: {e}");
            return fallback();
        }
    };

    let mut cursor: usize = clamp_cursor(initial_cursor, entries.len());
    let mut checked: Vec<bool> = vec![false; entries.len()];
    for i in prechecked {
        if let Some(v) = checked.get_mut(*i) {
            *v = true;
        }
    }
    // Loop ends exactly once, via one of these:
    let mut confirmed = false;
    // Esc and Ctrl+C both leave the screen, but they mean different things to
    // the caller: Esc steps back to the previous question, Ctrl+C abandons the
    // whole flow. `Browser::run` therefore has to tell them apart, so the
    // distinction is tracked separately instead of collapsing into `cancelled`.
    let mut went_back = false;
    let mut abandoned = false;

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let Ok(mut terminal) = ratatui::Terminal::new(backend) else {
        drop(guard);
        return fallback();
    };

    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    while !confirmed && !went_back && !abandoned {
        let _ = terminal.draw(|f| {
            let header_len = if header.is_empty() {
                0
            } else {
                (header.len() as u16 + 1).min(6)
            };
            let chunks = Layout::vertical([
                Constraint::Length(header_len),
                Constraint::Percentage(52),
                Constraint::Percentage(38),
                Constraint::Length(1),
            ])
            .split(f.area());

            if !header.is_empty() {
                let text: Vec<Line> = header.iter().map(|h| Line::raw(h.clone())).collect();
                let p = Paragraph::new(text).block(
                    Block::bordered()
                        .border_set(ratatui::symbols::border::ROUNDED)
                        .title(" "),
                );
                f.render_widget(p, chunks[0]);
            }

            let rows: Vec<Line> = entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let is_cursor = i == cursor;
                    let style = if is_cursor {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = row_prefix(mode, is_cursor, checked[i]);
                    Line::styled(format!("{prefix}{}", e.label), style)
                })
                .collect();
            // Fixed chrome: always SCRAPMF vX.Y.Z, context appended as " ─ {context}"
            let chrome = {
                let ver = env!("CARGO_PKG_VERSION");
                let base = format!("SCRAPMF v{ver}");
                if title.is_empty() || title == base || title.starts_with("SCRAPMF v") {
                    format!(" {title} ")
                } else {
                    format!(" SCRAPMF v{ver} ─ {title} ")
                }
            };
            let nav = Paragraph::new(rows).block(
                Block::bordered()
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(chrome),
            );
            f.render_widget(nav, chunks[1]);

            let detail: Vec<Line> = entries[cursor]
                .details
                .iter()
                .map(|s| Line::from(s.as_str()))
                .collect();
            let details = Paragraph::new(detail).block(
                Block::bordered()
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" details "),
            );
            f.render_widget(details, chunks[2]);

            // An explicit hint from the caller wins, so a screen that steps back
            // on Esc can advertise it; otherwise fall back to the per-mode text.
            let hints = match hint {
                Some(h) => h.to_string(),
                None => match mode {
                    Mode::Single => "↑↓ Navigate · Enter Select · q Cancel",
                    // Mirrors `multi_hint`: Enter picks the highlighted row
                    // when nothing is checked, Space is for picking more.
                    Mode::Multi => {
                        "↑↓ Move · ⏎ Pick This One · Space Adds More · a All/None · q Cancel"
                    }
                }
                .to_string(),
            };
            f.render_widget(Paragraph::new(hints), chunks[3]);
        });

        let has_event =
            crossterm::event::poll(std::time::Duration::from_millis(33)).unwrap_or(false);
        if !has_event {
            continue;
        }
        let Ok(Event::Key(key)) = crossterm::event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl_c = matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Up => cursor = cursor.saturating_sub(1),
            KeyCode::Down => cursor = (cursor + 1).min(entries.len() - 1),
            KeyCode::End => cursor = entries.len() - 1,
            KeyCode::Enter => confirmed = true,
            KeyCode::Char('a' | 'A') if mode == Mode::Multi => {
                toggle_all(&mut checked);
            }
            KeyCode::Char(' ') if mode == Mode::Multi => {
                toggle_at(&mut checked, cursor);
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => went_back = true,
            _ => {}
        }
        if ctrl_c {
            abandoned = true;
        }
    }

    drop(guard);

    outcome_for(confirmed, went_back, abandoned, mode, cursor, &checked)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Enter must accept the selection. This is the regression guard for an
    /// inverted condition that made Enter fall through to the "nothing chosen"
    /// fallback, so every confirmation abandoned the flow — Enter behaved
    /// exactly like Esc.
    #[test]
    fn enter_accepts_the_selection_in_single_mode() {
        assert_eq!(
            outcome_for(true, false, false, Mode::Single, 2, &[false; 3]),
            Outcome::Picked(2)
        );
    }

    #[test]
    fn enter_returns_the_checked_items_in_multi_mode() {
        let checked = [false, true, false, true];
        assert_eq!(
            outcome_for(true, false, false, Mode::Multi, 0, &checked),
            Outcome::Toggled(vec![1, 3])
        );
    }

    #[test]
    fn enter_on_an_unchecked_row_picks_it_in_multi_mode() {
        // Space is only needed for more than one. Without this, Enter with
        // nothing checked returned an empty selection, which every caller
        // treats as aborting the run, so Enter looked like it did nothing.
        assert_eq!(
            outcome_for(true, false, false, Mode::Multi, 1, &[false; 3]),
            Outcome::Toggled(vec![1])
        );
    }

    #[test]
    fn esc_steps_back_without_consuming_the_selection() {
        let checked = [true, false];
        assert_eq!(
            outcome_for(false, true, false, Mode::Single, 1, &checked),
            Outcome::Back
        );
        assert_eq!(
            outcome_for(false, true, false, Mode::Multi, 0, &checked),
            Outcome::Back,
            "Esc is 'go back', not 'choose nothing'"
        );
    }

    #[test]
    fn ctrl_c_abandons_rather_than_stepping_back() {
        assert_eq!(
            outcome_for(false, false, true, Mode::Single, 0, &[false; 2]),
            Outcome::Abandoned
        );
    }

    /// An unsettled loop means the screen was never usable, so no selection
    /// happened — distinct from a confirmed empty multi-select.
    #[test]
    fn unsettled_loop_yields_no_selection() {
        assert_eq!(
            outcome_for(false, false, false, Mode::Single, 1, &[false; 2]),
            Outcome::Quit
        );
        assert_eq!(
            outcome_for(false, false, false, Mode::Multi, 0, &[false; 2]),
            Outcome::Toggled(vec![])
        );
    }

    /// Ctrl+C wins over a simultaneous Back, since abandoning is the stronger
    /// signal and the loop can end on both.
    #[test]
    fn abandon_takes_precedence_over_back() {
        assert_eq!(
            outcome_for(false, true, true, Mode::Single, 0, &[false]),
            Outcome::Abandoned
        );
    }

    #[test]
    fn toggles_are_local_and_all_flips_both_ways() {
        let mut c = vec![false, false, true];
        toggle_at(&mut c, 0);
        assert_eq!(c, vec![true, false, true]);
        assert!(toggle_all(&mut c)); // something was off → all on
        assert_eq!(c, vec![true, true, true]);
        assert!(!toggle_all(&mut c)); // nothing off → all off
        assert_eq!(c, vec![false, false, false]);
    }

    #[test]
    fn toggle_at_ignores_out_of_range() {
        let mut c = vec![true];
        toggle_at(&mut c, 7);
        assert_eq!(c, vec![true]);
    }

    /// A wizard that steps back re-opens on the answer it already has, so the
    /// cursor has to start there — and a stale index must not index past the
    /// list. Checked on the clamp directly: driving the real browser needs a
    /// terminal, which the test runner has no reason to have.
    #[test]
    fn the_initial_cursor_lands_somewhere_valid() {
        assert_eq!(clamp_cursor(1, 3), 1);
        assert_eq!(clamp_cursor(0, 3), 0);
        assert_eq!(clamp_cursor(99, 3), 2, "past the end lands on the last row");
        assert_eq!(clamp_cursor(5, 0), 0, "an empty list has no row");
        assert_eq!(clamp_cursor(5, 1), 0);
    }

    #[test]
    fn prefixes_show_cursor_and_checkbox() {
        assert_eq!(row_prefix(Mode::Single, true, false), "▶ ");
        assert_eq!(row_prefix(Mode::Single, false, false), "  ");
        assert_eq!(row_prefix(Mode::Multi, true, true), "▶[x] ");
        assert_eq!(row_prefix(Mode::Multi, false, true), " [x] ");
        assert_eq!(row_prefix(Mode::Multi, false, false), " [ ] ");
    }
}
