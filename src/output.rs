//! Styled terminal output helpers (anstyle + anstream).
//!
//! Colors are automatically disabled when the target is not a TTY or
//! `NO_COLOR` is set (handled by `anstream`).

use anstyle::{AnsiColor, Color, Style};

pub fn print_success(msg: &str) {
    let style = Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Green)))
        .bold();
    anstream::println!("{}{}{}", style.render(), msg, Style::new().render());
}

pub fn print_error(msg: &str) {
    let style = Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Red)))
        .bold();
    anstream::eprintln!("{}error:{} {msg}", style.render(), Style::new().render());
}

pub fn print_info(msg: &str) {
    anstream::println!("{msg}");
}

/// Warning: a check that did not fail outright but needs attention (e.g. a
/// missing D-Bus session, which blocks cookie decryption without aborting).
pub fn print_warn(msg: &str) {
    let style = Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
        .bold();
    anstream::println!("{}warning:{} {msg}", style.render(), Style::new().render());
}

pub fn print_help(msg: &str) {
    let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
    anstream::println!("{}{}{}", style.render(), msg, Style::new().render());
}

pub fn print_note(msg: &str) {
    let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
    anstream::eprintln!("{}note:{} {msg}", style.render(), Style::new().render());
}
