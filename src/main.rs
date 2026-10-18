use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use tracing_subscriber::EnvFilter;

use scrapmf::cli::{Cli, Commands};
use scrapmf::commands;

fn init_tracing(verbose: u8) {
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// One config initialization step: label + fallible action.
type InitStep = (&'static str, fn() -> anyhow::Result<()>);

/// Initialize config dirs/files/migrations. Only called for commands that need
/// them (never for `--help` / `--version`, which must stay side-effect free).
/// Failures are logged (visible with -v) but do not abort execution.
fn ensure_app_config() {
    if std::env::var_os("SCRAPMF_NO_INIT").is_some() {
        return;
    }
    // One-time rename migration (scarpmf -> scrapmf) before anything else
    // touches the new directories.
    scrapmf::config::migrate_legacy_dirs();
    let steps: &[InitStep] = &[
        ("create config dirs", scrapmf::config::ensure_config_dirs),
        ("default config", scrapmf::config::ensure_default_config),
        // Single canonical site generation — registry is the only source of truth.
        // Adding a new network = one `SiteSpec` entry; threads remains plugin-backed.
        ("ensure sites", scrapmf::sites::ensure_all_sites),
        ("site highlights migration", || {
            scrapmf::config::migrate_all_sites_highlights().map(|_: usize| ())
        }),
        ("filename date-first migration", || {
            scrapmf::config::migrate_all_sites_filenames().map(|_: usize| ())
        }),
        ("legacy placeholder migration", || {
            scrapmf::config::migrate_legacy_placeholders().map(|_: usize| ())
        }),
    ];
    for (label, step) in steps {
        if let Err(e) = step() {
            tracing::warn!(step = label, error = %e, "config initialization failed");
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Some(Commands::Scrape {
            url,
            output,
            preset,
            cookies,
            cookies_from_browser,
            dry_run,
            no_archive,
            profile_pic_only,
        }) => {
            ensure_app_config();
            commands::scrape::run(
                url,
                output,
                preset,
                cookies,
                cookies_from_browser,
                dry_run,
                no_archive,
                profile_pic_only,
                cli.verbose,
            )
            .context("scrape command failed")?;
        }
        Some(Commands::Config { command }) => {
            ensure_app_config();
            commands::config::run(command).context("config command failed")?;
        }
        Some(Commands::Doctor) => {
            ensure_app_config();
            commands::doctor::run(cli.verbose).context("doctor failed")?;
        }
        Some(Commands::Setup { yes }) => {
            ensure_app_config();
            commands::setup::run(yes).context("setup failed")?;
        }
        None => {
            // No subcommand: enter interactive mode if TTY, otherwise show help
            if scrapmf::cli::interactive::is_interactive() {
                ensure_app_config();
                // Offer the bundled backend on first run (respects
                // auto_install_backends and never prompts outside a TTY)
                commands::setup::offer_if_missing();
                // All interactive scrape flows run as internal batches.
                scrapmf::cli::interactive::run();
                std::process::exit(0);
            } else {
                let mut cmd = Cli::command();
                let _ = cmd.print_help();
                println!();
                std::process::exit(2);
            }
        }
    }

    Ok(())
}
