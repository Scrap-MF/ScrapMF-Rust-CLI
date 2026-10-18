//! Live scrape execution dashboard (ratatui, alternate screen).
//!
//! Shows one row per job (label, files counter, state) plus an optional
//! scrolling log feed of gallery-dl stderr lines. Ctrl-C sets the shared
//! abort flag: gallery-dl children are killed and pending jobs are marked
//! cancelled. Nothing is persisted to disk.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const LOG_CAPACITY: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Done(usize), // files downloaded
    Failed(String),
    Cancelled,
}

/// Progress within one job's sub-extractor queue — full plan up front plus
/// the index of the step currently running (1-based; 0 = not started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepsState {
    pub names: Vec<String>,
    pub current: usize,
}

#[derive(Debug, Clone)]
pub struct JobRow {
    pub label: String,
    pub files: usize,
    pub state: JobState,
    pub steps: Option<StepsState>,
}

/// Shared dashboard state — mutated by scrape hooks from the worker thread,
/// read by the render loop.
#[derive(Debug, Default)]
pub struct DashboardState {
    jobs: Vec<JobRow>,
    logs: VecDeque<String>,
}

impl DashboardState {
    pub fn new(labels: impl IntoIterator<Item = String>) -> Self {
        Self {
            jobs: labels
                .into_iter()
                .map(|label| JobRow {
                    label,
                    files: 0,
                    state: JobState::Pending,
                    steps: None,
                })
                .collect(),
            logs: VecDeque::new(),
        }
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        if self.logs.len() == LOG_CAPACITY {
            self.logs.pop_front();
        }
        self.logs.push_back(line.into());
    }

    pub fn set_running(&mut self, idx: usize) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.state = JobState::Running;
        }
    }

    /// Load the full sub-extractor plan for a job (checklist up front).
    pub fn set_steps(&mut self, idx: usize, names: Vec<String>) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.steps = Some(StepsState { names, current: 0 });
        }
    }

    /// Mark the step currently running (1-based).
    pub fn set_step(&mut self, idx: usize, current: usize) {
        if let Some(job) = self.jobs.get_mut(idx)
            && let Some(steps) = job.steps.as_mut()
        {
            steps.current = current;
        }
    }

    pub fn add_file(&mut self, idx: usize) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.files += 1;
        }
    }

    /// Mark job done (with file count) or failed with a message.
    pub fn finish_ok(&mut self, idx: usize) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.state = JobState::Done(job.files);
        }
    }

    pub fn finish_failed(&mut self, idx: usize, message: impl Into<String>) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.state = JobState::Failed(message.into());
        }
    }

    /// Cancel the running job plus every pending one (Ctrl-C path).
    pub fn cancel_from(&mut self, idx: usize) {
        for job in self.jobs.iter_mut().skip(idx) {
            if matches!(job.state, JobState::Pending | JobState::Running) {
                job.state = JobState::Cancelled;
            }
        }
    }
}

/// Slice of `logs` visible in a panel of `rows` content lines, scrolled
/// `scroll` lines up from the tail (0 = follow the latest line).
fn log_window<'a>(logs: &'a [&'a String], scroll: usize, rows: usize) -> &'a [&'a String] {
    let len = logs.len();
    let rows = rows.max(1);
    let max_off = len.saturating_sub(rows);
    let off = scroll.min(max_off);
    let end = len - off;
    let start = end.saturating_sub(rows);
    &logs[start..end]
}

/// One rendered job row (pure — unit-testable).
fn job_line(job: &JobRow, index: usize, total: usize) -> String {
    let files = if matches!(job.state, JobState::Running | JobState::Done(_)) {
        format!(" · {} archivos", job.files)
    } else {
        String::new()
    };
    format!("[{}/{}] {}{files}", index + 1, total, job.label)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepMark {
    Done,
    Active,
    Pending,
}

/// One rendered checklist row for a sub-step.
fn step_line(name: &str, index: usize, total: usize, current: usize) -> (String, StepMark) {
    let mark = if index < current {
        StepMark::Done
    } else if index == current {
        StepMark::Active
    } else {
        StepMark::Pending
    };
    let text = match mark {
        StepMark::Done => format!("✔ [{index}/{total}] {name}"),
        StepMark::Active => format!("▶ [{index}/{total}] {name}"),
        StepMark::Pending => format!("  [{index}/{total}] {name}"),
    };
    (text, mark)
}

/// Terminal guard restoring the normal terminal on drop (panic-safe).
pub(crate) struct TerminalGuard;

impl TerminalGuard {
    pub(crate) fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn draw(
    f: &mut ratatui::Frame,
    state: &DashboardState,
    show_logs: bool,
    log_scroll: usize,
    finished: bool,
    cancelled: bool,
) {
    use ratatui::{
        layout::{Constraint, Layout},
        style::{Color, Modifier, Style},
        text::Line,
        widgets::{Block, Paragraph, Wrap},
    };

    let chunks: Vec<ratatui::layout::Rect> = if show_logs {
        // Jobs left (~40%), logs right (~60%), full height each.
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(f.area())
            .to_vec()
    } else {
        Layout::vertical([Constraint::Percentage(100)])
            .split(f.area())
            .to_vec()
    };

    // Jobs panel (left) — header row + one checklist row per sub-process
    let mut lines: Vec<Line> = Vec::new();
    for (i, job) in state.jobs.iter().enumerate() {
        let (icon, style) = match &job.state {
            JobState::Pending => (" ", Style::default().fg(Color::DarkGray)),
            JobState::Running => ("▶", Style::default().fg(Color::Cyan)),
            JobState::Done(_) => ("✔", Style::default().fg(Color::Green)),
            JobState::Failed(_) => ("✖", Style::default().fg(Color::Red)),
            JobState::Cancelled => ("⚠", Style::default().fg(Color::Yellow)),
        };
        let detail = match &job.state {
            JobState::Failed(msg) => format!(" — {msg}"),
            _ => String::new(),
        };
        lines.push(Line::styled(
            format!("{icon} {}{detail}", job_line(job, i, state.jobs.len())),
            style,
        ));
        if let Some(steps) = &job.steps {
            for (n, name) in steps.names.iter().enumerate() {
                let (text, mark) = step_line(name, n + 1, steps.names.len(), steps.current);
                let style = match mark {
                    StepMark::Done => Style::default().fg(Color::Green),
                    StepMark::Active => Style::default().fg(Color::Cyan),
                    StepMark::Pending => Style::default().fg(Color::DarkGray),
                };
                lines.push(Line::styled(format!("   {text}"), style));
            }
        }
    }
    if finished {
        let (msg, color) = if cancelled {
            ("⚠ Cancelled — press Enter to go back", Color::Yellow)
        } else {
            ("✔ Done — press Enter to go back", Color::Green)
        };
        lines.push(Line::styled(
            msg,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    } else {
        lines.push(Line::styled(
            "Ctrl-C: abortar",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ));
    }
    let jobs_block = Paragraph::new(lines)
        .block(
            Block::bordered()
                .border_set(ratatui::symbols::border::ROUNDED)
                .title(" scrapmf "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(jobs_block, chunks[0]);

    // Log panel (right, batch flow only) — color-coded by line prefix,
    // scrollable with keyboard (↑↓ / PgUp/PgDn).
    if show_logs {
        let rows = usize::from(chunks[1].height.saturating_sub(2)).max(1);
        let all: Vec<&String> = state.logs.iter().collect();
        let window = log_window(&all, log_scroll, rows);
        let log_lines: Vec<Line> = window
            .iter()
            .map(|l| {
                let raw = l.as_str();
                if raw.starts_with('✔') {
                    Line::styled(format!("· {raw}"), Style::default().fg(Color::Green))
                } else if raw.contains("[warning]") || raw.starts_with('⚠') {
                    Line::styled(format!("· {raw}"), Style::default().fg(Color::Yellow))
                } else if raw.contains("[error]") || raw.contains("failed:") {
                    Line::styled(format!("· {raw}"), Style::default().fg(Color::Red))
                } else if raw.contains("retrying") {
                    Line::styled(format!("· {raw}"), Style::default().fg(Color::Yellow))
                } else {
                    Line::from(format!("· {raw}"))
                }
            })
            .collect();
        let title = if log_scroll > 0 {
            " log (↑↓ scroll · siga ↓) "
        } else {
            " log (↑↓ scroll) "
        };
        let log_block = Paragraph::new(log_lines)
            .block(
                Block::bordered()
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(title),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(log_block, chunks[1]);
    }
}

/// Run the blocking TUI dashboard while `worker` executes the batch.
///
/// The worker closure receives nothing; it interacts through the shared
/// `state` (hooks mutate it) and must respect `abort` (checked between
/// reads by the executor; on abort children are killed).
///
/// When the user presses Ctrl-C in the dashboard the abort flag is also set.
/// After the worker returns the dashboard does NOT close: it keeps rendering
/// a finished state ("Done") until the user presses Enter (or Ctrl-C),
/// so results can be inspected before returning to the menu. While running,
/// ↑↓ / PgUp / PgDn scroll the log panel (no mouse capture — text selection
/// stays available).
pub fn run_dashboard<F, R>(
    state: Arc<Mutex<DashboardState>>,
    abort: Arc<AtomicBool>,
    show_logs: bool,
    worker: F,
) -> R
where
    F: FnOnce() -> R,
{
    // Reset any stale abort request from a previous run
    abort.store(false, Ordering::Relaxed);

    let guard = match TerminalGuard::enter() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("error: cannot enter TUI mode: {e}");
            return worker();
        }
    };

    let finished = Arc::new(AtomicBool::new(false));
    let exit_req = Arc::new(AtomicBool::new(false));
    let render_finished = finished.clone();
    let render_exit = exit_req.clone();
    let render_abort = abort.clone();
    let handle = std::thread::spawn(move || {
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        let mut terminal = match ratatui::Terminal::new(backend) {
            Ok(t) => t,
            Err(_) => return,
        };
        let mut log_scroll: usize = 0;
        loop {
            if render_exit.load(Ordering::Relaxed) {
                break;
            }
            // NOTE: an abort must NOT kill this render loop — the finished
            // phase below waits for exit_req, and only this loop reads the
            // keyboard. Breaking here would freeze the app with no way out.
            let mut snapshot_scroll = log_scroll;
            let _ = terminal.draw(|f| {
                let Ok(snapshot) = state.lock() else {
                    return;
                };
                // Clamp scroll against current content length and panel size.
                let rows = usize::from(f.area().height.saturating_sub(2)).max(1);
                let max_off = snapshot.logs.len().saturating_sub(rows);
                log_scroll = log_scroll.min(max_off);
                snapshot_scroll = log_scroll;
                draw(
                    f,
                    &snapshot,
                    show_logs,
                    snapshot_scroll,
                    render_finished.load(Ordering::Relaxed),
                    // "Cancelled" only in the finished phase (abort + done)
                    render_finished.load(Ordering::Relaxed) && render_abort.load(Ordering::Relaxed),
                );
            });
            let _ = snapshot_scroll; // (kept for clarity; clamp applied above)
            // Poll keyboard: Ctrl-C sets abort; arrows/PgUp/PgDn scroll logs;
            // Enter leaves once finished.
            let has_event =
                crossterm::event::poll(std::time::Duration::from_millis(33)).unwrap_or(false);
            if has_event
                && let Ok(ev) = crossterm::event::read()
                && let crossterm::event::Event::Key(key) = ev
            {
                use crossterm::event::KeyCode;
                let ctrl_c = matches!(key.code, KeyCode::Char('c' | 'C'))
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL);
                if ctrl_c {
                    render_abort.store(true, Ordering::Relaxed);
                    if render_finished.load(Ordering::Relaxed) {
                        render_exit.store(true, Ordering::Relaxed);
                    }
                    continue;
                }
                if key.kind == crossterm::event::KeyEventKind::Press {
                    match key.code {
                        KeyCode::Up => log_scroll = log_scroll.saturating_add(1),
                        KeyCode::Down => log_scroll = log_scroll.saturating_sub(1),
                        KeyCode::PageUp => log_scroll = log_scroll.saturating_add(10),
                        KeyCode::PageDown => log_scroll = log_scroll.saturating_sub(10),
                        KeyCode::End => log_scroll = 0,
                        KeyCode::Enter if render_finished.load(Ordering::Relaxed) => {
                            render_exit.store(true, Ordering::Relaxed)
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    let result = worker();

    // Finished: keep the dashboard up until Enter/Ctrl-C instead of kicking
    // the user back to the main menu.
    finished.store(true, Ordering::Relaxed);
    while !exit_req.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(33));
    }

    // Stop the render thread and wait for it before restoring the terminal
    abort.store(true, Ordering::Relaxed);
    let _ = handle.join();
    drop(guard);
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn job(state: JobState) -> JobRow {
        JobRow {
            label: "instagram:example_user · posts".into(),
            files: 0,
            state,
            steps: None,
        }
    }

    #[test]
    fn log_window_follows_tail_and_scrolls() {
        let v: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let refs: Vec<&String> = v.iter().collect();
        // follow tail: last 3 lines
        let w = log_window(&refs, 0, 3);
        assert_eq!(
            w.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["7", "8", "9"]
        );
        // scrolled 2 up: 5..8
        let w = log_window(&refs, 2, 3);
        assert_eq!(
            w.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["5", "6", "7"]
        );
        // scroll beyond top clamps
        let w = log_window(&refs, 99, 3);
        assert_eq!(
            w.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["0", "1", "2"]
        );
        // more rows than content clamps to whole content
        let w = log_window(&refs, 0, 50);
        assert_eq!(w.len(), 10);
    }

    #[test]
    fn job_line_keeps_header_simple() {
        let mut j = job(JobState::Running);
        j.files = 14;
        let line = job_line(&j, 0, 1);
        assert!(line.contains("14 archivos"), "got {line}");
        // sub-process names live in the checklist rows, not the header
        assert!(!line.contains("stories"), "got {line}");
    }

    #[test]
    fn step_line_marks_done_active_pending() {
        // current=3 → steps 1-2 done, 3 active, 4+ pending
        assert!(step_line("posts", 1, 4, 3).0.starts_with("✔ [1/4] posts"));
        assert!(step_line("reels", 2, 4, 3).0.starts_with("✔ [2/4] reels"));
        assert!(
            step_line("highlights", 3, 4, 3)
                .0
                .starts_with("▶ [3/4] highlights")
        );
        assert_eq!(
            step_line("stories", 4, 4, 3),
            ("  [4/4] stories".to_string(), StepMark::Pending)
        );
    }
}
