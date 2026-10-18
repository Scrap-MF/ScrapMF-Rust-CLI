//! Saved content batches: a name, the accounts to run it on, and what each one
//! wants.
//!
//! A task answers *what and for whom*; a profile answers *how*, with per-run
//! content choices and per-site overrides. They are separate on purpose —
//! separate directory, struct, menus and flows — but both speak
//! [`config::Account`], so a cookie session behaves the same in each.
//!
//! Content belongs to the account, not the task: the wizard asks site → account
//! → content per account, so two instagram accounts in one task can want
//! different things. A task may span networks, and the kinds are a *filter*
//! rather than an instruction — each account is validated against what its site
//! actually supports, and one that cannot serve the request is named and
//! skipped instead of silently fetching something else.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::cli::interactive::menu::{self, Step};
use crate::cli::interactive::theme;
use crate::config::{self, Task, TaskAccount};

/// A prompt's answer, keeping the two ways out apart.
///
/// `Option<T>` cannot carry both, and conflating them is what made Ctrl+C a
/// dead key here: every helper returned `None` for "stepped back" *and* for
/// "abandoned", so leaving without saving was the same gesture as going back.
enum Answer<T> {
    Value(T),
    /// Esc — return to the previous question.
    Back,
    /// Ctrl+C — leave the wizard without writing.
    Abort,
}

/// What Esc at the site question should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SiteBack {
    /// Leave the wizard without writing.
    Leave,
    /// Re-open the last committed account.
    Reopen,
}

/// Whether Esc at the site question re-opens the previous account or leaves.
///
/// Extracted because the looping case is a flag transition nobody can see in
/// the control flow: inside a re-open, "step back" used to re-open the very
/// same account, and the user could never get out of the wizard with Esc.
fn site_back_escapes(in_reopen_pass: bool, has_last_added: bool) -> SiteBack {
    // Nothing committed yet, or already inside a re-open where re-opening
    // again would land on the screen we started from.
    if !has_last_added || in_reopen_pass {
        SiteBack::Leave
    } else {
        SiteBack::Reopen
    }
}

/// What a finished wizard did.
enum Created {
    Saved(String),
    /// The user stepped out; nothing was written.
    Cancelled,
    /// The write failed, with the reason. Surfaced loudly: a save that fails
    /// quietly is the one thing the user cannot act on.
    Failed(String),
}

/// What the menu can do with a saved task.
enum TaskAction {
    Run,
    Edit,
    Delete,
    Back,
}

/// List saved tasks and act on one.
pub(super) fn tasks_menu() {
    let cfg = config::load().unwrap_or_default();
    // One-shot line on the menu footer. `println!` between prompts lands on the
    // main screen, which the menu's alternate screen then covers — so the
    // outcome of a wizard is reported where the user is actually looking.
    let mut notice: Option<String> = None;
    loop {
        let tasks = config::load_tasks();
        let mut names: Vec<String> = tasks.keys().cloned().collect();
        names.sort();

        let mut entries: Vec<(String, Vec<String>)> = Vec::new();
        for name in &names {
            let t = &tasks[name];
            let accounts: usize = t.accounts.values().map(|v| v.len()).sum();
            let sites = t.accounts.len();
            entries.push((
                theme::brand_site_label(name),
                vec![
                    format!("{} account(s) across {sites} site(s)", accounts),
                    describe_task(t),
                ],
            ));
        }
        entries.push((
            "Create new task".to_string(),
            vec![
                "Pick the content once, then reuse it.".to_string(),
                String::new(),
                "Each account gets its own content and".to_string(),
                "cookie session.".to_string(),
            ],
        ));
        entries.push(("Back".to_string(), vec![String::new()]));

        let create_idx = names.len();
        let back_idx = names.len() + 1;
        let context = match notice.take() {
            Some(n) => format!("Tasks ─ {n}"),
            None => "Tasks".to_string(),
        };
        let picked = match menu::pick_single(&context, entries) {
            Step::Value(i) => i,
            // Nothing above this menu: Esc leaves.
            Step::Back | Step::Cancel => return,
        };
        match picked {
            i if i == create_idx => {
                let outcome = create_task(None);
                report(&outcome);
                notice = notice_from(&outcome);
            }
            i if i == back_idx => return,
            i => {
                let id = names[i].clone();
                task_menu(&id, &tasks[&id], &cfg, &mut notice);
            }
        }
    }
}

/// What one can do with a single task.
fn task_menu(id: &str, task: &Task, cfg: &config::Config, notice: &mut Option<String>) {
    let action = {
        let entries = vec![
            (
                "Run".to_string(),
                vec![
                    String::new(),
                    "Media lands under the task name.".to_string(),
                ],
            ),
            (
                "Edit".to_string(),
                vec![
                    "Rewalk the wizard with today's accounts.".to_string(),
                    String::new(),
                ],
            ),
            (
                "Delete".to_string(),
                vec!["Removes the saved task.".to_string(), String::new()],
            ),
            ("Back".to_string(), vec![String::new()]),
        ];
        let picked = menu::pick_single(&theme::brand_site_label(id), entries);
        match picked {
            Step::Value(0) => TaskAction::Run,
            Step::Value(1) => TaskAction::Edit,
            Step::Value(2) => TaskAction::Delete,
            Step::Value(_) => TaskAction::Back,
            Step::Back | Step::Cancel => TaskAction::Back,
        }
    };
    match action {
        TaskAction::Run => {
            match run_task(task, cfg) {
                Step::Value(()) => super::clear_screen(),
                // Esc at the preview steps back inside the task rather than
                // out of the list.
                Step::Back | Step::Cancel => {}
            }
        }
        TaskAction::Edit => {
            let outcome = create_task(Some(task.clone()));
            report(&outcome);
            *notice = notice_from(&outcome);
        }
        TaskAction::Delete => {
            // Destructive: Enter keeps it, `y` deletes it. The key is named in
            // the prompt because every *ordinary* question in the app answers
            // yes on Enter, so pressing it here reads as "broken" rather than
            // as "no" — the hint line alone was not enough.
            let sure = menu::confirm_back(
                "Tasks",
                &format!("delete task '{id}'?  (y to confirm)"),
                false,
                true,
            )
            .value()
            .unwrap_or(false);
            if sure && let Some(dir) = config::tasks_dir() {
                let path = dir.join(format!("{id}.toml"));
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        println!("✔ task '{id}' deleted");
                        *notice = Some(format!("deleted '{id}'"));
                    }
                    Err(e) => {
                        let why = format!("could not delete '{id}': {e}");
                        report(&Created::Failed(why.clone()));
                        *notice = Some(why);
                    }
                }
            }
        }
        TaskAction::Back => {}
    }
}

/// The wizard: name, then site → account → content, until the user saves.
///
/// `existing` restarts the account list for [`TaskAction::Edit`], so editing is
/// the same walk rather than a second, divergent flow. `Esc` steps back one
/// question at every level and keeps what was typed, matching every other flow.
fn create_task(existing: Option<Task>) -> Created {
    let editing = existing.is_some();
    let (name, mut accounts) = match existing {
        Some(t) => {
            let id = t.task.clone().unwrap_or_else(|| "task".to_string());
            (id, t.accounts)
        }
        None => match ask_task_name() {
            Answer::Value(n) => (n, std::collections::HashMap::new()),
            Answer::Back | Answer::Abort => return Created::Cancelled,
        },
    };

    let Some(site_opts) =
        Some(crate::cli::interactive::scrape_flow::site_options_with_fallbacks(&[]))
    else {
        crate::output::print_error("no sites configured — add one under Configuration first");
        return Created::Cancelled;
    };
    if site_opts.is_empty() {
        crate::output::print_error("no sites configured — add one under Configuration first");
        return Created::Cancelled;
    }

    // The most recently committed account, and where it sits in its site's
    // list. Esc at the site question re-opens it instead of leaving: a step
    // back must not save, and must not drop work either.
    let mut last_added: Option<(String, TaskAccount, usize)> = None;
    // True on the pass that re-opens `last_added`: the account is already in
    // `accounts`, so committing must replace it rather than append.
    let mut reopening = false;
    // True for the whole of a re-open pass, not just its entry. Without it Esc
    // at the site question re-opened the same account forever, so stepping
    // back never reached the exit.
    let mut escapes_to_picker = false;
    // The committed account a re-open shows directly, skipping every question.
    let mut reopen_account: Option<TaskAccount> = None;
    // Answers from the pass in flight, so a step back re-opens the question on
    // what the user already gave.
    let mut previous_site: Option<String> = None;
    let mut previous_username: Option<String> = None;
    let mut previous_kinds: Vec<String> = Vec::new();

    'wizard: loop {
        // Re-opening skips the site question entirely: the network and the
        // account are already known.
        // `is_reopen` has to outlive the branch: the account is committed at
        // the picker, several questions later. Clearing `reopening` here made
        // the picker think it was adding a new account and would have stacked
        // a second copy of the one being edited.
        let mut is_reopen = false;
        let site = if reopening {
            reopening = false;
            match &last_added {
                Some((s, _, _)) => {
                    is_reopen = true;
                    s.clone()
                }
                None => return Created::Cancelled,
            }
        } else {
            match ask_site(&name, &site_opts, previous_site.as_deref()) {
                Answer::Value(s) => s,
                // Ctrl+C leaves without writing, always.
                Answer::Abort => return Created::Cancelled,
                Answer::Back => {
                    // Inside a re-open, going back again would land on the
                    // picker we came from. Leaving is the only way forward.
                    if site_back_escapes(escapes_to_picker, last_added.is_some()) == SiteBack::Leave
                    {
                        return Created::Cancelled;
                    }
                    // Otherwise re-open the last account: the pass that
                    // follows takes the site from `last_added` itself.
                    let Some((_, a, _)) = last_added.clone() else {
                        return Created::Cancelled;
                    };
                    // Restored so the commit question can be shown as it was:
                    // stepping back one question lands on the picker, not on
                    // the username we just came forward from.
                    previous_username = a.account.username.clone();
                    previous_kinds = a.kinds.clone();
                    reopen_account = Some(a);
                    reopening = true;
                    escapes_to_picker = true;
                    continue 'wizard;
                }
            }
        };
        // A remembered username belongs to the network it was typed for.
        // Carrying it into a different site would offer an instagram handle as
        // the answer for a tiktok prompt.
        if previous_site.as_deref() != Some(site.as_str()) {
            previous_username = None;
            previous_kinds.clear();
        }
        previous_site = Some(site.clone());

        'who: loop {
            // On a re-open all four answers are already known, so only the
            // commit question is worth showing.
            let (username, cookies) = if is_reopen {
                (
                    previous_username.clone().unwrap_or_default(),
                    reopen_account
                        .as_ref()
                        .and_then(|a| a.account.cookie_profile.clone()),
                )
            } else {
                let username = match ask_username(&site, previous_username.as_deref()) {
                    Answer::Value(u) => u,
                    Answer::Abort => return Created::Cancelled,
                    Answer::Back => continue 'wizard,
                };
                previous_username = Some(username.clone());
                let cookies = match pick_cookie_for(&site) {
                    Answer::Value(c) => c,
                    Answer::Abort => return Created::Cancelled,
                    Answer::Back => continue 'who,
                };
                (username, cookies)
            };

            'detail: loop {
                // Captured before the flag is cleared. Clearing it *here* rather
                // than on entry is what makes Esc from the picker land on the
                // content question instead of on the picker again.
                let replace = is_reopen;
                is_reopen = false;

                let account = if replace {
                    reopen_account.clone().unwrap_or_default()
                } else {
                    let kinds = match ask_kinds(&site, &previous_kinds) {
                        Answer::Value(k) => k,
                        Answer::Abort => return Created::Cancelled,
                        Answer::Back => continue 'who,
                    };
                    previous_kinds = kinds.clone();

                    // Held back until the picker decides: committing here is
                    // what made Esc at the picker duplicate the account.
                    let mut a = TaskAccount::default();
                    a.account.username = Some(username.clone());
                    a.account.cookie_profile = cookies.clone();
                    a.kinds = kinds;
                    a
                };

                // Two positive actions rather than a yes/no: there is no "no"
                // to express, and Enter takes the highlighted row, which is
                // save.
                let choice = menu::pick_single(
                    &format!("New task ─ {name}"),
                    vec![
                        (
                            format!("Save task '{name}'"),
                            vec![format!("{} account(s) total", total(&accounts) + 1)],
                        ),
                        (
                            "Add another account".to_string(),
                            vec!["Site, account and content again.".to_string()],
                        ),
                    ],
                );
                match choice {
                    Step::Value(0) => {
                        commit_pending(&mut accounts, &site, &account, replace);
                        return finish(&name, accounts, editing);
                    }
                    Step::Value(_) => {
                        let idx = commit_pending(&mut accounts, &site, &account, replace);
                        last_added = Some((site.clone(), account, idx));
                        previous_username = None;
                        previous_kinds.clear();
                        // A normal pass: Esc at the site re-opens again.
                        escapes_to_picker = false;
                        break 'who;
                    }
                    // Esc re-asks the content question with its selection
                    // still marked, and drops the un-committed account.
                    Step::Back => continue 'detail,
                    Step::Cancel => return Created::Cancelled,
                }
            }
        }
    }
}

/// Add the pending account, or replace the one being re-opened.
///
/// Returns where it landed in that site's list. Replacing by index rather than
/// popping keeps the account where the user put it, and matters because two
/// accounts of the same site can exist: popping would have edited the wrong
/// one.
fn commit_pending(
    accounts: &mut std::collections::HashMap<String, Vec<TaskAccount>>,
    site: &str,
    account: &TaskAccount,
    replace: bool,
) -> usize {
    let slot = accounts.entry(site.to_string()).or_default();
    if replace && let Some(idx) = slot.len().checked_sub(1) {
        slot[idx] = account.clone();
        return idx;
    }
    slot.push(account.clone());
    slot.len() - 1
}

fn total(accounts: &std::collections::HashMap<String, Vec<TaskAccount>>) -> usize {
    accounts.values().map(|v| v.len()).sum()
}

fn finish(
    name: &str,
    accounts: std::collections::HashMap<String, Vec<TaskAccount>>,
    editing: bool,
) -> Created {
    if accounts.is_empty() {
        println!("— no accounts, nothing saved");
        return Created::Cancelled;
    }
    let Some(dir) = config::tasks_dir() else {
        crate::output::print_error("could not locate the tasks directory");
        return Created::Cancelled;
    };
    let path = dir.join(format!("{name}.toml"));
    if !editing && path.exists() {
        crate::output::print_error(&format!("task '{name}' already exists"));
        return Created::Cancelled;
    }
    let task = Task {
        task: Some(name.to_string()),
        display_name: Some(name.to_string()),
        accounts,
    };
    match config::write_task_file(&path, &task) {
        Ok(()) => Created::Saved(name.to_string()),
        Err(e) => Created::Failed(format!("could not save '{name}': {e}")),
    }
}

/// The one-line version of a wizard outcome, for the menu footer.
fn notice_from(outcome: &Created) -> Option<String> {
    match outcome {
        Created::Saved(name) => Some(format!("saved '{name}'")),
        Created::Cancelled => Some("cancelled".to_string()),
        // A failure already got the loud treatment; repeating it on the
        // footer would only be noise.
        Created::Failed(_) => None,
    }
}

/// Report the outcome of a wizard on the main screen, where it is readable.
///
/// Nothing is on screen while this waits: the menu has not redrawn yet, so the
/// message is not covered by the alternate screen the way a bare `println!`
/// between prompts is.
fn report(outcome: &Created) {
    let line = match outcome {
        Created::Saved(name) => format!("✔ task '{name}' saved"),
        Created::Cancelled => "— cancelled, nothing saved".to_string(),
        Created::Failed(why) => format!("⚠ {why}"),
    };
    println!("{line}");
    if matches!(outcome, Created::Failed(_)) {
        let _ = inquire::Text::new("press Enter to continue")
            .with_render_config(theme::render_config())
            .prompt();
    }
}

fn ask_task_name() -> Answer<String> {
    loop {
        let raw = menu::input_text_back(
            "New task",
            "Task name (becomes the output folder):",
            "school_stories",
            "letters, digits, - and _",
            "",
        );
        let name = match raw {
            Step::Value(v) => v,
            Step::Back => return Answer::Back,
            Step::Cancel => return Answer::Abort,
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            println!("⚠ the task needs a name");
            continue;
        }
        // Same rules as profile ids: the name becomes a folder name.
        if name.contains('/') || name.contains('.') || name.contains('\\') {
            println!("⚠ no / . or \\ in a task name — it becomes a folder");
            continue;
        }
        return Answer::Value(name);
    }
}

fn ask_site(task: &str, site_opts: &[String], previous: Option<&str>) -> Answer<String> {
    let site_list: Vec<(String, Vec<String>)> = site_opts
        .iter()
        .map(|s| {
            let details = crate::sites::registry::find_by_id(s)
                .map(|sp| {
                    vec![
                        format!("kinds: {}", sp.content_kinds.join(", ")),
                        format!("backend: {:?}", sp.backend),
                    ]
                })
                .unwrap_or_else(|| vec!["custom site (sites/*.toml)".to_string()]);
            (theme::brand_site_label(s), details)
        })
        .collect();
    let initial = previous
        .and_then(|p| site_opts.iter().position(|s| s == p))
        .unwrap_or(0);
    let picked = menu::pick_single_back_at(&format!("Task '{task}' ─ site"), site_list, initial);
    match picked {
        Step::Value(i) => match site_opts.get(i) {
            Some(s) => Answer::Value(s.clone()),
            None => Answer::Back,
        },
        Step::Back => Answer::Back,
        Step::Cancel => Answer::Abort,
    }
}

fn ask_username(site: &str, previous: Option<&str>) -> Answer<String> {
    loop {
        let raw = menu::input_text_back(
            &format!("Task ─ {site}"),
            "Username:",
            "someone",
            "no @ needed",
            previous.unwrap_or(""),
        );
        let value = match raw {
            Step::Value(v) => v,
            Step::Back => return Answer::Back,
            Step::Cancel => return Answer::Abort,
        };
        let value = value.trim().trim_start_matches('@').trim().to_string();
        if value.is_empty() {
            println!("⚠ enter a username");
            continue;
        }
        return Answer::Value(value);
    }
}

/// Choose which stored cookie session this account uses.
///
/// Only stored profiles are offered, and the ones that actually carry cookies
/// for this site come first — offering a session that cannot authenticate the
/// account is worse than offering none.
fn pick_cookie_for(site: &str) -> Answer<Option<String>> {
    use crate::config::cookies;
    let stored = cookies::list_profiles();
    if stored.is_empty() {
        // Nothing to choose: the account just keeps the site default.
        return Answer::Value(None);
    }
    let sites = vec![site.to_string()];
    let ranked = rank_cookie_profiles(stored, &sites);

    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    entries.push((
        "Site default".to_string(),
        vec!["whatever sites/*.toml configures".to_string()],
    ));
    for (matches, name) in &ranked {
        let summary = cookies::profile_summary(name).unwrap_or_default();
        entries.push((
            format!("{name}  — {summary}"),
            vec![if *matches {
                format!("has cookies for {site}")
            } else {
                format!("⚠ no cookies for {site}")
            }],
        ));
    }
    match menu::pick_single_back(&format!("Cookies for {site}"), entries) {
        // Index 0 is the site default, an explicit answer rather than a skip.
        Step::Value(0) => Answer::Value(None),
        Step::Value(i) => Answer::Value(ranked.get(i - 1).map(|(_, name)| name.clone())),
        Step::Back => Answer::Back,
        Step::Cancel => Answer::Abort,
    }
}

/// Stored profiles, the ones carrying cookies for `sites` first, then by name.
fn rank_cookie_profiles(stored: Vec<String>, sites: &[String]) -> Vec<(bool, String)> {
    use crate::config::cookies;
    let mut ranked: Vec<(bool, String)> = stored
        .into_iter()
        .map(|name| {
            let matches = cookies::has_cookies_for(&name, sites);
            (matches, name)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    ranked
}

/// Pick what this account wants, from its own site's menu.
fn ask_kinds(site: &str, previous: &[String]) -> Answer<Vec<String>> {
    let options = crate::cli::interactive::content::content_options(site);
    if options.is_empty() {
        return Answer::Value(vec!["All".to_string()]);
    }
    let entries: Vec<(String, Vec<String>)> = options
        .iter()
        .map(|o| (o.to_string(), Vec::new()))
        .collect();
    let prechecked = prechecked_indices(&options, previous);
    let idxs =
        match menu::pick_multi_back(&format!("Task ─ {site} ─ content"), entries, &prechecked) {
            Step::Value(v) => v,
            Step::Back => return Answer::Back,
            Step::Cancel => return Answer::Abort,
        };
    let picked: Vec<String> = idxs
        .into_iter()
        .filter_map(|i| options.get(i).map(|o| o.to_string()))
        .collect();
    if picked.is_empty() {
        println!("⚠ pick at least one kind");
        return ask_kinds(site, previous);
    }
    Answer::Value(picked)
}

/// Indices of `previous` within `options`, for the picker to pre-mark.
///
/// A label the site no longer offers is skipped rather than treated as an
/// error: the task was written when the site offered it, and the run reports
/// the account as skipped instead of refusing to open the menu.
fn prechecked_indices(options: &[&str], previous: &[String]) -> Vec<usize> {
    previous
        .iter()
        .filter_map(|k| options.iter().position(|o| *o == k.as_str()))
        .collect()
}

/// Run a saved task.
fn run_task(task: &Task, cfg: &config::Config) -> Step<()> {
    let cfg = cfg.clone();
    let task_id = task.task.clone().unwrap_or_else(|| "task".to_string());

    let mut jobs = Vec::new();
    // Which job indices still need a session for this run, so the per-run
    // cookie question can never overwrite one the task already pinned.
    let mut needs_session: Vec<usize> = Vec::new();

    let mut sites: Vec<&String> = task.accounts.keys().collect();
    sites.sort();
    for site in sites {
        for ta in &task.accounts[site] {
            let account = &ta.account;
            let Some(username) = account
                .username
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
            else {
                println!("⚠ {site}: account without a username — skipped");
                continue;
            };
            let requested: HashSet<String> = ta.kinds.iter().cloned().collect();
            let kinds = kinds_for_site(site, &requested);
            if kinds.is_empty() {
                // Name the account and the reason: a silent skip reads as
                // "the task has nothing to fetch".
                println!(
                    "⚠ {site}:{username} — {} has no content this site can fetch — skipped",
                    describe_kinds(&ta.kinds)
                );
                continue;
            }

            let tagged = crate::cli::interactive::content::build_tagged_urls(site, username);
            let site_cfg = cfg.sites.get(site.as_str()).cloned();
            let (cookies_file, cookies_from_browser) = config::cookies::resolve_session(
                account.cookie_profile.as_deref(),
                account.cookies.clone(),
                account.cookies_from_browser.clone(),
                cfg.general.cookies_from_browser.clone(),
                site_cfg.as_ref(),
            );
            let pinned = account.cookie_profile.is_some() || cookies_file.is_some();

            let ctx = crate::cli::interactive::scrape_flow::AccountCtx {
                site: site.clone(),
                username: username.to_string(),
                kinds,
                tagged,
                directory_template: site_cfg.as_ref().and_then(|s| s.directory_template.clone()),
                extractor_options: site_cfg
                    .as_ref()
                    .map(|s| s.extractor.clone())
                    .unwrap_or_default(),
                extra_args: resolve_extra_args(
                    site_cfg
                        .as_ref()
                        .map(|s| s.extra_args.as_slice())
                        .unwrap_or(&[]),
                    &account.extra_args,
                ),
                cookies_file,
                cookies_from_browser,
                archive: site_cfg.as_ref().and_then(|s| s.archive.clone()),
                rate_limit: site_cfg.as_ref().and_then(|s| s.rate_limit.clone()),
                filename_template: site_cfg.as_ref().and_then(|s| s.filename_template.clone()),
                // The task's name becomes `{scrapmf_root}`, so its media lands
                // in its own tree instead of mixing with a profile's.
                profile_name: task_id.clone(),
                output_dir: resolve_output_dir(
                    account.output_dir.as_deref(),
                    site_cfg.as_ref().and_then(|s| s.output_dir.as_deref()),
                    &cfg.general.output_dir,
                ),
            };
            match crate::cli::interactive::scrape_flow::jobs_for_account(&ctx) {
                Ok(built) => {
                    let first = jobs.len();
                    jobs.extend(built);
                    if !pinned {
                        for i in first..jobs.len() {
                            needs_session.push(i);
                        }
                    }
                }
                Err(reason) => println!("⚠ {site}:{username} — {reason}"),
            }
        }
    }

    if jobs.is_empty() {
        println!("ℹ nothing to download for '{task_id}'");
        return Step::Value(());
    }

    // Only ask about the accounts the task did not already give a session.
    if !needs_session.is_empty() && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        let mut unique: Vec<String> = needs_session
            .iter()
            .filter_map(|i| jobs.get(*i).map(|(_, site, ..)| site.clone()))
            .collect();
        unique.sort();
        unique.dedup();
        if let Some(file) = prompt_cookie_override(&unique) {
            crate::cli::interactive::scrape_flow::apply_cookie_file_where(
                &mut jobs,
                &file,
                |i, _| needs_session.contains(&i),
            );
        }
    }

    crate::cli::interactive::scrape_flow::preview_and_execute(jobs, &cfg)
}

/// Ask which stored session to use for the accounts left open.
fn prompt_cookie_override(sites: &[String]) -> Option<PathBuf> {
    use crate::config::cookies;
    let stored = cookies::list_profiles();
    if stored.is_empty() {
        return None;
    }
    let ranked = rank_cookie_profiles(stored, sites);

    let mut entries: Vec<(String, Vec<String>)> = vec![(
        "Site default".to_string(),
        vec!["whatever sites/*.toml configures".to_string()],
    )];
    for (matches, name) in &ranked {
        let summary = cookies::profile_summary(name).unwrap_or_default();
        entries.push((
            format!("{name}  — {summary}"),
            vec![if *matches {
                "has cookies for these sites".to_string()
            } else {
                "⚠ no cookies for these sites".to_string()
            }],
        ));
    }
    match menu::pick_single_back("Cookies for this run?", entries) {
        Step::Value(0) | Step::Back => None,
        Step::Value(i) => ranked
            .get(i - 1)
            .map(|(_, name)| name)
            .and_then(|n| cookies::profile_path(n))
            .filter(|p| p.exists()),
        Step::Cancel => None,
    }
}

/// Where one account's media goes: account, then site, then global.
///
/// `TaskAccount` flattens a profile `Account`, so a task file can carry
/// `output_dir` — and silently ignoring it would be a field that reads as
/// configured and does nothing. Same precedence the profile flow uses.
fn resolve_output_dir(
    account: Option<&std::path::Path>,
    site: Option<&std::path::Path>,
    global: &std::path::Path,
) -> std::path::PathBuf {
    account
        .or(site)
        .map(crate::config::expand_output_dir)
        .unwrap_or_else(|| crate::config::expand_output_dir(global))
}

/// Extra args for one account: the site's first, then the account's.
///
/// The account's come last so they can override — the same order the profile
/// flow builds them in.
fn resolve_extra_args(site: &[String], account: &[String]) -> Vec<String> {
    let mut out = site.to_vec();
    out.extend(account.iter().cloned());
    out
}

/// What one account can actually get: the content it asked for minus what the
/// site has no menu entry for, with `"All"` meaning everything this site
/// supports.
fn kinds_for_site(
    site: &str,
    requested: &HashSet<String>,
) -> Vec<crate::cli::interactive::content::ContentKind> {
    use crate::cli::interactive::content::{ContentKind, content_options};
    let options = content_options(site);
    if requested.is_empty() {
        // A task written before kinds were per-account, or hand-edited.
        return options
            .iter()
            .filter(|o| **o != "All")
            .filter_map(|o| ContentKind::from_label(o))
            .collect();
    }
    if requested.contains("All") {
        return options
            .iter()
            .filter(|o| **o != "All")
            .filter_map(|o| ContentKind::from_label(o))
            .collect();
    }
    options
        .iter()
        .filter(|o| **o != "All")
        .filter(|o| requested.contains(**o))
        .filter_map(|o| ContentKind::from_label(o))
        .collect()
}

fn describe_kinds(labels: &[String]) -> String {
    if labels.is_empty() {
        "no content".to_string()
    } else {
        labels.join(", ")
    }
}

/// One line describing what a task fetches, for the list.
fn describe_task(task: &Task) -> String {
    let mut per: Vec<String> = Vec::new();
    let mut sites: Vec<&String> = task.accounts.keys().collect();
    sites.sort();
    for site in sites {
        let mut kinds: Vec<&String> = task.accounts[site]
            .iter()
            .flat_map(|a| a.kinds.iter())
            .collect();
        kinds.sort();
        kinds.dedup();
        per.push(format!("{site}: {}", describe_kinds_owned(&kinds)));
    }
    per.join("  ")
}

fn describe_kinds_owned(labels: &[&String]) -> String {
    if labels.is_empty() {
        "no content".to_string()
    } else {
        labels
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn kinds_for_site_keeps_only_what_the_site_offers() {
        let requested: HashSet<String> = ["Stories".to_string()].into_iter().collect();
        let ig = kinds_for_site("instagram", &requested);
        assert_eq!(ig.len(), 1, "instagram has stories");
        // TikTok has no Stories menu entry, so the account is skipped rather
        // than silently given something else.
        assert!(
            kinds_for_site("tiktok", &requested).is_empty(),
            "tiktok offers no stories"
        );
    }

    #[test]
    fn all_expands_per_site_not_globally() {
        let requested: HashSet<String> = ["All".to_string()].into_iter().collect();
        let ig = kinds_for_site("instagram", &requested);
        let tiktok = kinds_for_site("tiktok", &requested);
        assert!(!ig.is_empty() && !tiktok.is_empty());
        assert_ne!(
            ig.len(),
            tiktok.len(),
            "'All' means whatever this site supports"
        );
    }

    #[test]
    fn two_accounts_can_want_different_things() {
        // Content is per account, so one account asking for stories does not
        // drag the other along — the whole reason kinds moved onto the account.
        let stories: HashSet<String> = ["Stories".to_string()].into_iter().collect();
        let reels: HashSet<String> = ["Reels".to_string()].into_iter().collect();
        assert_eq!(kinds_for_site("instagram", &stories).len(), 1);
        assert_eq!(kinds_for_site("instagram", &reels).len(), 1);
        assert_ne!(
            kinds_for_site("instagram", &stories),
            kinds_for_site("instagram", &reels)
        );
    }

    #[test]
    fn a_stale_kind_skips_the_account_instead_of_failing() {
        // A hand-edited task, or a site whose menu changed: the account is
        // dropped and named, never silently given different content.
        let gone: HashSet<String> = ["Nonsense".to_string()].into_iter().collect();
        assert!(kinds_for_site("instagram", &gone).is_empty());
    }

    #[test]
    fn an_account_with_no_kinds_falls_back_to_the_whole_menu() {
        let requested = HashSet::new();
        assert!(
            !kinds_for_site("instagram", &requested).is_empty(),
            "a task written before kinds were per-account still runs"
        );
    }

    fn account(username: &str, kinds: &[&str]) -> TaskAccount {
        let mut a = TaskAccount::default();
        a.account.username = Some(username.to_string());
        a.kinds = kinds.iter().map(|k| k.to_string()).collect();
        a
    }

    #[test]
    fn the_accounts_output_dir_wins_over_the_sites_and_the_global() {
        // A task file is meant to be hand-edited, so `output_dir` inside it has
        // to do something — the same precedence the profile flow uses.
        use super::resolve_output_dir;
        let global = std::path::Path::new("/global");
        assert_eq!(
            resolve_output_dir(
                Some(Path::new("/account")),
                Some(Path::new("/site")),
                global
            ),
            PathBuf::from("/account")
        );
    }

    #[test]
    fn without_an_account_output_dir_the_site_or_the_global_decides() {
        use super::resolve_output_dir;
        let global = std::path::Path::new("/global");
        assert_eq!(
            resolve_output_dir(None, Some(Path::new("/site")), global),
            PathBuf::from("/site"),
            "falls back to the site"
        );
        assert_eq!(
            resolve_output_dir(None, None, global),
            PathBuf::from("/global"),
            "and to the global when the site says nothing"
        );
    }

    #[test]
    fn extra_args_append_the_accounts_after_the_sites() {
        // Last wins is why the account's go second: an account can override
        // something the site set for everyone.
        use super::resolve_extra_args;
        let site = vec!["--quiet".to_string(), "--retries".to_string()];
        let account = vec!["--retries".to_string(), "9".to_string()];
        assert_eq!(
            resolve_extra_args(&site, &account),
            vec![
                "--quiet".to_string(),
                "--retries".to_string(),
                "--retries".to_string(),
                "9".to_string()
            ]
        );
        assert!(resolve_extra_args(&[], &[]).is_empty());
    }

    #[test]
    fn esc_at_the_site_re_opens_until_it_would_loop() {
        use super::{SiteBack, site_back_escapes};
        // Nothing committed yet: there is no previous account to go back to.
        assert_eq!(site_back_escapes(false, false), SiteBack::Leave);
        // A normal pass with an account to return to: re-open it.
        assert_eq!(site_back_escapes(false, true), SiteBack::Reopen);
        // Already inside a re-open: re-opening lands on the picker we came
        // from, so Esc has to leave or the user is trapped in the wizard.
        assert_eq!(site_back_escapes(true, true), SiteBack::Leave);
        assert_eq!(site_back_escapes(true, false), SiteBack::Leave);
    }

    #[test]
    fn the_reopen_escape_is_not_the_same_as_an_unreopened_pass() {
        use super::site_back_escapes;
        // The whole bug in one assertion: same state, opposite outcome,
        // because the pass is a re-open rather than a fresh account.
        assert_ne!(
            site_back_escapes(false, true),
            site_back_escapes(true, true),
            "only the pass kind distinguishes leaving from looping"
        );
    }

    #[test]
    fn commit_appends_when_the_account_is_new() {
        let mut accounts = std::collections::HashMap::new();
        assert_eq!(
            commit_pending(
                &mut accounts,
                "instagram",
                &account("one", &["Stories"]),
                false
            ),
            0
        );
        assert_eq!(
            commit_pending(
                &mut accounts,
                "instagram",
                &account("two", &["Reels"]),
                false
            ),
            1
        );
        let list = &accounts["instagram"];
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].account.username.as_deref(), Some("one"));
    }

    #[test]
    fn commit_replaces_in_place_when_reopening() {
        // Re-opening the last account must edit it, not stack a copy: this is
        // what made Esc at the picker duplicate an entry.
        let mut accounts = std::collections::HashMap::new();
        commit_pending(
            &mut accounts,
            "instagram",
            &account("one", &["Stories"]),
            false,
        );
        let idx = commit_pending(
            &mut accounts,
            "instagram",
            &account("one", &["Reels"]),
            true,
        );

        let list = &accounts["instagram"];
        assert_eq!(list.len(), 1, "replaced, not appended");
        assert_eq!(idx, 0);
        assert_eq!(list[0].kinds, vec!["Reels"], "the new content stuck");
    }

    #[test]
    fn replacing_edits_the_last_of_several_accounts_on_the_same_site() {
        // Two accounts on one network: popping the last element would have
        // edited the wrong one.
        let mut accounts = std::collections::HashMap::new();
        commit_pending(
            &mut accounts,
            "instagram",
            &account("one", &["Stories"]),
            false,
        );
        commit_pending(
            &mut accounts,
            "instagram",
            &account("two", &["Posts"]),
            false,
        );
        commit_pending(
            &mut accounts,
            "instagram",
            &account("two", &["Highlights"]),
            true,
        );

        let list = &accounts["instagram"];
        assert_eq!(list.len(), 2, "still two accounts");
        assert_eq!(list[0].kinds, vec!["Stories"], "the first is untouched");
        assert_eq!(list[1].kinds, vec!["Highlights"]);
    }

    #[test]
    fn replacing_on_an_empty_site_appends_instead() {
        // `replace` is only ever true for an account already in the list, but a
        // defensive branch beats an underflow panic.
        let mut accounts = std::collections::HashMap::new();
        let idx = commit_pending(
            &mut accounts,
            "instagram",
            &account("one", &["Stories"]),
            true,
        );
        assert_eq!(idx, 0);
        assert_eq!(accounts["instagram"].len(), 1);
    }

    #[test]
    fn back_and_abort_stay_distinct_answers() {
        // Collapsing these is what made Ctrl+C a dead key: it used to return
        // the same `None` as Esc, so leaving without saving was impossible.
        fn shape(a: Answer<String>) -> &'static str {
            match a {
                Answer::Value(_) => "value",
                Answer::Back => "back",
                Answer::Abort => "abort",
            }
        }
        assert_eq!(shape(Answer::Value("x".into())), "value");
        assert_eq!(shape(Answer::Back), "back");
        assert_eq!(shape(Answer::Abort), "abort");
    }

    #[test]
    fn precheck_marks_the_kinds_already_chosen() {
        // Esc at the content question has to re-open it marked, or the user
        // loses the selection just by stepping back one question.
        let options = ["All", "Posts", "Reels", "Highlights", "Stories", "Profile"];
        // Order follows the stored selection, not the menu: `prechecked` is a
        // set of rows to mark, so the order carries no meaning either way.
        assert_eq!(
            prechecked_indices(&options, &["Stories".into(), "Highlights".into()]),
            vec![4, 3]
        );
        assert_eq!(prechecked_indices(&options, &["Posts".into()]), vec![1]);
    }

    #[test]
    fn precheck_skips_a_kind_the_site_no_longer_offers() {
        // Written when the site offered it, or kept across a registry change:
        // the menu still opens, and the run reports the account as skipped.
        let options = ["All", "Posts", "Reels"];
        let stale: Vec<usize> = prechecked_indices(&options, &["Stories".into()]);
        assert!(stale.is_empty());
        let both = prechecked_indices(&options, &["Reels".into(), "Gone".into()]);
        assert_eq!(
            both,
            vec![2],
            "the stale kind is dropped, the live one is not"
        );
    }

    #[test]
    fn precheck_of_nothing_marks_nothing() {
        let options = ["All", "Posts"];
        assert!(prechecked_indices(&options, &[]).is_empty());
    }

    #[test]
    fn task_list_line_names_each_site_and_its_content() {
        let mut task = Task {
            task: Some("colegio".into()),
            display_name: Some("colegio".into()),
            accounts: Default::default(),
        };
        task.accounts.insert(
            "instagram".into(),
            vec![TaskAccount {
                account: config::Account {
                    username: Some("one".into()),
                    ..Default::default()
                },
                kinds: vec!["Stories".into()],
            }],
        );
        task.accounts.insert(
            "tiktok".into(),
            vec![TaskAccount {
                account: config::Account {
                    username: Some("two".into()),
                    ..Default::default()
                },
                kinds: vec!["Videos".into()],
            }],
        );
        let line = describe_task(&task);
        assert!(line.contains("instagram: Stories"), "{line}");
        assert!(line.contains("tiktok: Videos"), "{line}");
    }
}
