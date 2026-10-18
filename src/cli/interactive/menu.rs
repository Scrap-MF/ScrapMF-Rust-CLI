//! Unified decorated menu API — every selection goes through `Browser`
//! with the fixed chrome `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`.
//! Adding a new network/site/content kind requires no UI change: just
//! extend `sites::registry` and the menu inherits the chrome.

use crate::cli::interactive::browser::{Browser, Mode, Outcome};

/// Outcome of one question in a multi-step flow.
///
/// `Option` conflated "step back" with "leave": a caller that received `None`
/// could not offer to go back, so Esc anywhere meant starting over. These
/// three states are what a wizard actually needs — confirm, go back one step,
/// or abandon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step<T> {
    /// The user confirmed this question.
    Value(T),
    /// Esc — return to the previous question, preserving earlier answers.
    Back,
    /// Ctrl+C — abandon the flow entirely.
    Cancel,
}

impl<T> Step<T> {
    /// Value if confirmed, otherwise None.
    pub fn value(self) -> Option<T> {
        match self {
            Step::Value(v) => Some(v),
            Step::Back | Step::Cancel => None,
        }
    }

    /// True when the user asked to go back rather than leave.
    pub fn is_back(&self) -> bool {
        matches!(self, Step::Back)
    }

    /// Convert the value, keeping `Back` and `Cancel` untouched. Used where a
    /// question's `T` is not the shape the caller wants, e.g. a `bool` answer
    /// that only matters as "go again or not".
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Step<U> {
        match self {
            Step::Value(v) => Step::Value(f(v)),
            Step::Back => Step::Back,
            Step::Cancel => Step::Cancel,
        }
    }
}

/// Hint line for a multi-select.
///
/// Separate from [`key_hint`] because `Enter` does something different here:
/// it picks the highlighted row unless rows are already checked, so "confirm"
/// alone both hid the `Space` shortcut and described the wrong behaviour.
fn multi_hint(back: bool) -> String {
    let tail = if back { "esc back" } else { "esc cancel" };
    format!("↑↓ move · ⏎ pick this one · space adds more · a all/none · {tail}")
}

/// Hint line describing the keys, shown in prompts that support going back.
pub fn key_hint(back: bool) -> String {
    if back {
        "↑↓ move · ⏎ confirm · esc back · ctrl+c cancel".to_string()
    } else {
        "⏎ confirm · esc cancel".to_string()
    }
}

/// Fixed chrome prefix — always SCRAPMF version.
fn chrome_title(context: &str) -> String {
    let ver = env!("CARGO_PKG_VERSION");
    let base = format!("SCRAPMF v{ver}");
    if context.is_empty() || context == base {
        base
    } else if context.starts_with("SCRAPMF") {
        context.to_string()
    } else {
        format!("{base} ─ {context}")
    }
}

/// Single-select decorated menu (Browser Single). `context` is shown in the
/// border title, e.g. "Download content" → `╭ SCRAPMF v1.7.0 ─ Download content ─╮`.
/// Returns the picked index, `Back` on Esc/q and `Cancel` on Ctrl+C.
pub fn pick_single(context: &str, options: Vec<(String, Vec<String>)>) -> Step<usize> {
    pick_single_inner(context, Vec::new(), options, false, None)
}

/// As [`pick_single`], but with lines rendered above the options.
pub fn pick_single_details(
    context: &str,
    details: Vec<String>,
    options: Vec<(String, Vec<String>)>,
) -> Step<usize> {
    pick_single_inner(context, details, options, false, None)
}

/// As [`pick_single`], but advertises that Esc steps back.
pub fn pick_single_back(context: &str, options: Vec<(String, Vec<String>)>) -> Step<usize> {
    pick_single_inner(context, Vec::new(), options, true, None)
}

/// As [`pick_single_back`], but with the cursor starting on `initial`.
///
/// A wizard that steps back re-opens the question on the answer it already
/// has, instead of dropping the user at the top of the list. `initial` is
/// clamped, so a stale index is harmless.
pub fn pick_single_back_at(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    initial: usize,
) -> Step<usize> {
    pick_single_inner(context, Vec::new(), options, true, Some(initial))
}

fn pick_single_inner(
    context: &str,
    details: Vec<String>,
    options: Vec<(String, Vec<String>)>,
    back: bool,
    cursor: Option<usize>,
) -> Step<usize> {
    if options.is_empty() {
        return Step::Cancel;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).header(details).mode(Mode::Single);
    if let Some(i) = cursor {
        b = b.cursor_at(i);
    }
    if back {
        b = b.hint(key_hint(true));
    }
    for (label, entry_details) in options {
        b = b.entry(label, entry_details);
    }
    match b.run() {
        Outcome::Picked(i) => Step::Value(i),
        Outcome::Back => Step::Back,
        Outcome::Abandoned => Step::Cancel,
        Outcome::Toggled(_) | Outcome::Quit => Step::Cancel,
    }
}

/// Multi-select decorated menu (Browser Multi). Returns the picked indices,
/// `Back` on Esc/q and `Cancel` on Ctrl+C.
pub fn pick_multi(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
) -> Step<Vec<usize>> {
    pick_multi_inner(context, options, prechecked, false)
}

/// As [`pick_multi`], but advertises that Esc steps back.
pub fn pick_multi_back(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
) -> Step<Vec<usize>> {
    pick_multi_inner(context, options, prechecked, true)
}

fn pick_multi_inner(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
    back: bool,
) -> Step<Vec<usize>> {
    if options.is_empty() {
        return Step::Cancel;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).mode(Mode::Multi).checked(prechecked);
    b = b.hint(multi_hint(back));
    for (label, details) in options {
        b = b.entry(label, details);
    }
    match b.run() {
        Outcome::Toggled(v) => Step::Value(v),
        Outcome::Back => Step::Back,
        Outcome::Abandoned => Step::Cancel,
        Outcome::Picked(_) | Outcome::Quit => Step::Cancel,
    }
}

type BoxedBackend = ratatui::backend::CrosstermBackend<std::io::Stdout>;

/// Shared chrome for the boxed prompts: border, title and help line.
///
/// `input_text` and `confirm` render through this so a confirmation looks like
/// the rest of the app instead of dropping into a bare inquire prompt.
struct BoxedPrompt {
    title: String,
    terminal: ratatui::Terminal<BoxedBackend>,
    guard: crate::ui::dashboard::TerminalGuard,
}

impl BoxedPrompt {
    fn enter(context: &str) -> Result<Self, ()> {
        use crate::ui::dashboard::TerminalGuard;
        let guard = TerminalGuard::enter().map_err(|_| ())?;
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        let terminal = ratatui::Terminal::new(backend).map_err(|_| ())?;
        Ok(Self {
            title: chrome_title(context),
            terminal,
            guard,
        })
    }
}

/// Draw `body` inside the bordered box, with `help` pinned at the bottom.
fn draw_boxed(
    title: &str,
    terminal: &mut ratatui::Terminal<BoxedBackend>,
    body: Vec<ratatui::text::Line<'_>>,
    help: &str,
) -> std::io::Result<()> {
    {
        use ratatui::{
            layout::{Constraint, Layout},
            style::{Color, Style},
            widgets::{Block, Paragraph},
        };
        terminal.draw(|f| {
            let area = f.area();
            let block = Block::bordered()
                .border_set(ratatui::symbols::border::ROUNDED)
                .title(format!(" {title} "));
            let inner = block.inner(area);
            f.render_widget(block, area);
            let chunks = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(if help.is_empty() { 0 } else { 1 }),
            ])
            .split(inner);
            f.render_widget(Paragraph::new(body), chunks[0]);
            if !help.is_empty() {
                f.render_widget(
                    Paragraph::new(ratatui::text::Line::styled(
                        help,
                        Style::default().fg(Color::DarkGray),
                    ))
                    .wrap(ratatui::widgets::Wrap { trim: true }),
                    chunks[2],
                );
            }
        })?;
        Ok(())
    }
}

/// What a key press means on a yes/no question.
///
/// Split out from the event loop so every case is testable without a TTY, the
/// same reasoning that `browser::outcome_for` follows.
///
/// `None` means the key is not an answer and the question stays open.
///
/// Enter always confirms and Esc always steps back, on both kinds of question.
/// That is the flow the rest of the app already teaches: Enter proceeds, Esc
/// retreats. A destructive prompt used to invert Enter to "no" on the theory
/// that a stray keypress could delete a profile — but the box is modal, so the
/// user had already chosen "Delete profile" from the menu before the prompt
/// appeared, and the inversion only made the key that confirms every other
/// action silently refuse this one.
fn confirm_key(key: crossterm::event::KeyCode) -> Option<Step<bool>> {
    use crossterm::event::KeyCode;
    match key {
        KeyCode::Enter => Some(Step::Value(true)),
        KeyCode::Char('y' | 'Y') => Some(Step::Value(true)),
        // `n` answers no on both kinds of question. On an ordinary question it
        // is the only way to say no at all — dropping it in favour of Enter
        // left questions like "add another account?" answerable only with yes
        // or Esc, which is no way to decline.
        KeyCode::Char('n' | 'N') => Some(Step::Value(false)),
        KeyCode::Esc => Some(Step::Back),
        _ => None,
    }
}

/// Ask a yes/no question inside the app's chrome.
///
/// `Enter` confirms and `Esc` steps back, on every question. The `destructive`
/// flag no longer changes the key mapping — it only adds "yes" to the hint so
/// a prompt that removes something reads differently from one that adds.
///
/// The prompt is modal: the caller reached it by already choosing the
/// destructive action, so Enter confirms an intent the user has expressed
/// rather than completing a stray keystroke.
pub fn confirm_back(context: &str, prompt: &str, default: bool, destructive: bool) -> Step<bool> {
    confirm_box(context, prompt, &[], default, destructive)
}

/// As [`confirm_back`], but also shows `lines` above the question — used for
/// the scrape preview so the summary and the decision share one box.
pub fn confirm_box(
    context: &str,
    prompt: &str,
    lines: &[String],
    default: bool,
    destructive: bool,
) -> Step<bool> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    let help = if destructive {
        // Enter confirms everywhere in this app, and Esc is the way back, so
        // the hint says exactly that. Marking Enter as "no" here was the
        // opposite of what every other prompt does.
        "⏎ yes · esc back"
    } else {
        "⏎ yes · n no · esc back"
    };

    let mut boxed = match BoxedPrompt::enter(context) {
        Ok(b) => b,
        // No TTY: keep the previous inquire behaviour so scripts still work.
        Err(_) => {
            return match inquire::Confirm::new(prompt).with_default(default).prompt() {
                Ok(v) => Step::Value(v),
                Err(_) => Step::Cancel,
            };
        }
    };
    // Split the borrow: `title` is read while `terminal` is drawn.
    let BoxedPrompt {
        title,
        terminal,
        guard: _guard,
    } = &mut boxed;

    loop {
        let mut body: Vec<ratatui::text::Line<'_>> = Vec::new();
        for line in lines {
            body.push(ratatui::text::Line::raw(line.clone()));
        }
        if !body.is_empty() {
            body.push(ratatui::text::Line::raw(String::new()));
        }
        body.push(ratatui::text::Line::styled(
            format!("◆ {prompt}"),
            ratatui::style::Style::default().fg(ratatui::style::Color::Magenta),
        ));
        if draw_boxed(title, terminal, body, help).is_err() {
            break;
        }

        if !crossterm::event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            continue;
        }
        let Ok(Event::Key(key)) = crossterm::event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        // Ctrl+C is checked before the match so it wins over any character.
        if matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            // `boxed` owns the guard; dropping it here restores the terminal.
            return Step::Cancel;
        }
        let Some(result) = confirm_key(key.code) else {
            continue;
        };
        return result;
    }
    Step::Cancel
}

/// Show a result inside the Browser chrome and wait for acknowledgement.
///
/// Every menu in this app draws through `Browser` or [`BoxedPrompt`], both of
/// which render on the alternate screen. A `println!` issued between two menus
/// therefore lands on the normal buffer and is covered the moment the next box
/// is drawn — so an action that had already succeeded looked like it had done
/// nothing. That is the whole reason a delete reported nothing back.
///
/// Lines are coloured by leading marker, matching the log panel: `✔` green,
/// `✖` red, `ℹ`/`⚠` yellow. Anything else is plain.
///
/// Returns `Back` on Esc, `Cancel` on Ctrl+C, so a caller can tell "dismissed"
/// from "left".
pub fn notice(context: &str, lines: &[String]) -> Step<()> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    let mut boxed = match BoxedPrompt::enter(context) {
        Ok(b) => b,
        // No TTY: fall back to plain lines so scripted runs still see them.
        Err(_) => {
            for line in lines {
                println!("{line}");
            }
            return Step::Value(());
        }
    };
    let BoxedPrompt {
        title,
        terminal,
        guard: _guard,
    } = &mut boxed;

    loop {
        let body: Vec<ratatui::text::Line<'_>> = lines
            .iter()
            .map(|line| {
                use ratatui::style::{Color, Style};
                let style = if line.trim_start().starts_with('✔') {
                    Style::default().fg(Color::Green)
                } else if line.trim_start().starts_with('✖') {
                    Style::default().fg(Color::Red)
                } else if line.trim_start().starts_with(['ℹ', '⚠']) {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                ratatui::text::Line::styled(line.clone(), style)
            })
            .collect();
        if draw_boxed(title, terminal, body, "⏎ or esc dismiss").is_err() {
            break;
        }
        if !crossterm::event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            continue;
        }
        let Ok(Event::Key(key)) = crossterm::event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return Step::Cancel;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ' | 'q') => return Step::Value(()),
            KeyCode::Esc => return Step::Back,
            _ => {}
        }
    }
    Step::Value(())
}

/// Convenience for simple string options without details pane.
pub fn pick_single_labels(context: &str, labels: Vec<String>) -> Option<usize> {
    let opts = labels.into_iter().map(|l| (l, Vec::new())).collect();
    pick_single(context, opts).value()
}

/// Text input inside the Browser box `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`.
/// Returns `Back` on Esc/q, `Cancel` on Ctrl+C, and `Value` on a non-empty
/// submission. `initial` pre-fills the field so stepping back to fix a typo
/// shows what was typed instead of an empty box.
pub fn input_text(context: &str, prompt: &str, placeholder: &str, help: &str) -> Step<String> {
    input_text_inner(context, prompt, placeholder, help, "", false)
}

/// As [`input_text`], pre-filled with `initial` and advertising that Esc goes back.
pub fn input_text_back(
    context: &str,
    prompt: &str,
    placeholder: &str,
    help: &str,
    initial: &str,
) -> Step<String> {
    input_text_inner(context, prompt, placeholder, help, initial, true)
}

fn input_text_inner(
    context: &str,
    prompt: &str,
    placeholder: &str,
    help: &str,
    initial: &str,
    back: bool,
) -> Step<String> {
    use crate::ui::dashboard::TerminalGuard;
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::{
        layout::{Constraint, Layout},
        style::{Color, Style},
        text::Line,
        widgets::{Block, Paragraph},
    };

    let title = chrome_title(context);
    let guard = match TerminalGuard::enter() {
        Ok(g) => g,
        Err(_) => {
            // Fallback to plain inquire if TUI cannot be entered
            return match inquire::Text::new(prompt)
                .with_initial_value(initial)
                .with_placeholder(placeholder)
                .with_help_message(help)
                .prompt()
            {
                Ok(s) => {
                    let t = s.trim().to_string();
                    if t.is_empty() {
                        Step::Back
                    } else {
                        Step::Value(t)
                    }
                }
                Err(_) => Step::Cancel,
            };
        }
    };

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let Ok(mut terminal) = ratatui::Terminal::new(backend) else {
        drop(guard);
        return Step::Cancel;
    };

    let mut input = initial.to_string();
    let mut cursor: usize = input.chars().count();
    let mut confirmed = false;
    let mut went_back = false;
    let mut abandoned = false;

    while !confirmed && !went_back && !abandoned {
        let _ = terminal.draw(|f| {
            let area = f.area();
            let block = Block::bordered()
                .border_set(ratatui::symbols::border::ROUNDED)
                .title(format!(" {title} "));
            let inner = block.inner(area);
            f.render_widget(block, area);

            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);

            // Prompt line inside box
            let prompt_line =
                Line::styled(format!("◆ {prompt}"), Style::default().fg(Color::Magenta));
            f.render_widget(Paragraph::new(vec![prompt_line]), chunks[0]);
            f.render_widget(
                Paragraph::new(Line::styled(
                    "─".repeat(inner.width as usize),
                    Style::default().fg(Color::DarkGray),
                )),
                chunks[1],
            );

            // Input line with cursor — wrap placeholder/input to avoid cutting
            let display = if input.is_empty() && !placeholder.is_empty() {
                Line::styled(
                    format!("  {placeholder}"),
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                let before: String = input.chars().take(cursor).collect();
                let at = input.chars().nth(cursor).unwrap_or(' ');
                let after: String = input.chars().skip(cursor + 1).collect();
                if cursor < input.chars().count() {
                    Line::from(vec![
                        ratatui::text::Span::raw(format!("  {before}")),
                        ratatui::text::Span::styled(
                            at.to_string(),
                            Style::default().bg(Color::White).fg(Color::Black),
                        ),
                        ratatui::text::Span::raw(after),
                    ])
                } else {
                    Line::from(vec![
                        ratatui::text::Span::raw(format!("  {input}")),
                        ratatui::text::Span::styled(
                            " ",
                            Style::default().bg(Color::White).fg(Color::Black),
                        ),
                    ])
                }
            };
            f.render_widget(
                Paragraph::new(vec![display]).wrap(ratatui::widgets::Wrap { trim: true }),
                chunks[2],
            );

            // Help inside box bottom. When the screen supports going back, say
            // so — an Esc that silently jumps to the previous question would
            // otherwise look like the run was cancelled.
            let help_text = if back {
                let mut h = String::new();
                if !help.is_empty() {
                    h.push_str(help);
                    h.push_str("  ·  ");
                }
                h.push_str(&key_hint(true));
                h
            } else {
                help.to_string()
            };
            if !help_text.is_empty() {
                f.render_widget(
                    Paragraph::new(Line::styled(
                        help_text,
                        Style::default().fg(Color::DarkGray),
                    ))
                    .wrap(ratatui::widgets::Wrap { trim: true }),
                    chunks[3],
                );
            }
        });

        if !crossterm::event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
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
        if ctrl_c {
            abandoned = true;
            continue;
        }
        match key.code {
            KeyCode::Enter => confirmed = true,
            KeyCode::Esc => went_back = true,
            KeyCode::Backspace => {
                if cursor > 0 {
                    let mut chars: Vec<char> = input.chars().collect();
                    chars.remove(cursor - 1);
                    input = chars.into_iter().collect();
                    cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let mut chars: Vec<char> = input.chars().collect();
                if cursor < chars.len() {
                    chars.remove(cursor);
                    input = chars.into_iter().collect();
                }
            }
            KeyCode::Left => cursor = cursor.saturating_sub(1),
            KeyCode::Right => {
                let len = input.chars().count();
                if cursor < len {
                    cursor += 1;
                }
            }
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = input.chars().count(),
            KeyCode::Char(c) => {
                let mut chars: Vec<char> = input.chars().collect();
                chars.insert(cursor, c);
                input = chars.into_iter().collect();
                cursor += 1;
            }
            _ => {}
        }
    }

    drop(guard);

    if abandoned {
        return Step::Cancel;
    }
    if went_back {
        return Step::Back;
    }
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        // An empty submission is not an answer; treat it as "step back" so the
        // user is not stranded on a screen with nothing typed.
        Step::Back
    } else {
        Step::Value(trimmed)
    }
}

/// Generic nonempty input inside the box — loops until nonempty or cancel.
pub fn ask_nonempty(context: &str, prompt: &str) -> Option<String> {
    loop {
        let s = input_text(context, prompt, "", "").value()?;
        let t = s.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Step, chrome_title, confirm_key, key_hint};
    use crossterm::event::KeyCode;

    #[test]
    fn step_separates_back_from_cancel() {
        // The whole point: these two used to be the same `None`, which is why Esc
        // anywhere in a flow meant starting over.
        assert_ne!(Step::<u8>::Back, Step::<u8>::Cancel);
        assert!(Step::<u8>::Back.is_back());
        assert!(!Step::<u8>::Cancel.is_back());
        assert!(!Step::Value(1).is_back());
    }

    #[test]
    fn value_carries_the_answer() {
        assert_eq!(Step::Value(7).value(), Some(7));
        assert_eq!(Step::<u8>::Back.value(), None);
        assert_eq!(Step::<u8>::Cancel.value(), None);
    }

    #[test]
    fn hint_advertises_back_only_when_supported() {
        assert!(key_hint(true).contains("esc back"));
        assert!(!key_hint(false).contains("back"));
    }

    // ─── Yes/no confirmation ───────────────────────────────────────────────

    /// Enter is the fast path: one keystroke to continue. This is what the
    /// scrape preview depends on.
    #[test]
    fn enter_confirms_yes_for_an_ordinary_question() {
        assert_eq!(
            confirm_key(crossterm::event::KeyCode::Enter),
            Some(Step::Value(true))
        );
    }

    /// Enter confirms a deletion too. The prompt is modal — the user already
    /// picked "Delete profile" to get here — so Enter carries an intent
    /// already expressed rather than completing a stray keystroke. Inverting
    /// it made the key that confirms every other action refuse this one.
    #[test]
    fn enter_confirms_a_destructive_question() {
        assert_eq!(
            confirm_key(crossterm::event::KeyCode::Enter),
            Some(Step::Value(true))
        );
    }

    #[test]
    fn letters_remain_valid_answers() {
        assert_eq!(confirm_key(KeyCode::Char('y')), Some(Step::Value(true)));
        assert_eq!(confirm_key(KeyCode::Char('Y')), Some(Step::Value(true)));
        assert_eq!(confirm_key(KeyCode::Char('n')), Some(Step::Value(false)));
        assert_eq!(confirm_key(KeyCode::Char('N')), Some(Step::Value(false)));
    }

    #[test]
    fn esc_steps_back_rather_than_answering() {
        assert_eq!(confirm_key(KeyCode::Esc), Some(Step::Back));
    }

    /// Keys that mean nothing here must not answer the question — otherwise a
    /// stray letter could delete a profile.
    #[test]
    fn map_converts_the_value_and_keeps_the_way_out() {
        assert_eq!(Step::Value(2).map(|n| n * 2), Step::Value(4));
        // Backing out must survive the conversion, otherwise the caller loses
        // the only signal that the user wanted to go back.
        assert_eq!(
            Step::<u8>::Back.map(|n| n * 2),
            Step::Back,
            "Back is not a value"
        );
        assert_eq!(Step::<u8>::Cancel.map(|n| n * 2), Step::Cancel);
    }

    #[test]
    fn an_ordinary_question_can_still_be_declined() {
        // Enter answers yes in one keystroke, but a question whose answer is
        // often "no" needs a way to say so that is not Esc — Esc means "step
        // back" everywhere else, and a caller that maps it to `false` is
        // leaning on a side effect rather than a designed answer.
        assert_eq!(confirm_key(KeyCode::Char('n')), Some(Step::Value(false)));
        assert_eq!(confirm_key(KeyCode::Char('N')), Some(Step::Value(false)));
        assert_eq!(confirm_key(KeyCode::Char('y')), Some(Step::Value(true)));
    }

    #[test]
    fn unrelated_keys_are_ignored() {
        assert_eq!(confirm_key(KeyCode::Char('x')), None);
        assert_eq!(confirm_key(KeyCode::Left), None);
        assert_eq!(confirm_key(KeyCode::Tab), None);
    }

    #[test]
    fn chrome_title_formats() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(chrome_title(""), format!("SCRAPMF v{v}"));
        assert_eq!(
            chrome_title("Download content"),
            format!("SCRAPMF v{v} ─ Download content")
        );
        assert_eq!(
            chrome_title(&format!("SCRAPMF v{v}")),
            format!("SCRAPMF v{v}")
        );
    }
}
