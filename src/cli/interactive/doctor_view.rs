//! Full-screen single-box Doctor view — covers the whole terminal until
//! Enter/q/Esc/Ctrl-C. No two-pane split.

use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Paragraph},
};

use crate::commands::doctor::{Level, collect};
use crate::ui::dashboard::TerminalGuard;

pub(crate) fn show() {
    let (lines, _ok) = collect(1);

    let guard = match TerminalGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: cannot enter TUI mode: {e}");
            // Fallback: plain CLI output
            let _ = crate::commands::doctor::run(1);
            return;
        }
    };

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let Ok(mut terminal) = ratatui::Terminal::new(backend) else {
        drop(guard);
        let _ = crate::commands::doctor::run(1);
        return;
    };

    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    loop {
        let _ = terminal.draw(|f| {
            let chunks =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(f.area());

            let styled: Vec<Line> = lines
                .iter()
                .map(|l| {
                    let style = match l.level {
                        Level::Success => Style::default().fg(Color::Green),
                        Level::Info => Style::default().fg(Color::DarkGray),
                        Level::Warn => Style::default().fg(Color::Yellow),
                        Level::Error => Style::default().fg(Color::Red),
                        Level::Help => Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(ratatui::style::Modifier::ITALIC),
                    };
                    // Prefix to mirror CLI output style
                    let prefix = match l.level {
                        Level::Success => "✔ ",
                        Level::Warn => "⚠ ",
                        Level::Error => "✖ ",
                        Level::Info => "ℹ ",
                        Level::Help => "  ",
                    };
                    Line::styled(format!("{prefix}{}", l.text), style)
                })
                .collect();

            // Header + separator are rendered as part of the block title area;
            // keep them inside the single box for full-screen coverage.
            let mut all: Vec<Line> = Vec::new();
            all.push(Line::styled(
                "scrapmf doctor — checking backends and system",
                Style::default().add_modifier(ratatui::style::Modifier::BOLD),
            ));
            all.push(Line::from("─────────────────────────────────────────────"));
            all.extend(styled);
            all.push(Line::from("─────────────────────────────────────────────"));

            let block = Paragraph::new(all).block(
                Block::bordered()
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" Doctor "),
            );
            f.render_widget(block, chunks[0]);
            f.render_widget(Paragraph::new("Enter to go back · q/Esc Cancel"), chunks[1]);
        });

        let has_event =
            crossterm::event::poll(std::time::Duration::from_millis(100)).unwrap_or(false);
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
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q' | 'Q') => break,
            _ => {}
        }
        if ctrl_c {
            break;
        }
    }

    drop(guard);
}
