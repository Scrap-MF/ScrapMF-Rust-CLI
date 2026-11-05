use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "scrapmf",
    version,
    about = "Safe, interactive archiver for social media galleries",
    long_about = "scrapmf orchestrates gallery-dl (and optional plugins) to archive social media galleries.\n\
                  Run without a subcommand to open the interactive TUI.",
    arg_required_else_help = false,
    styles = clap::builder::styling::Styles::styled(),
    help_template = "╭─ {name} {version} ─╮\n│ {about} │\n╰───────────────────╯\n\n{usage-heading} {usage}\n\n{all-args}{after-help}",
    after_help = "Examples:\n  scrapmf scrape https://www.instagram.com/user/ --dry-run\n  scrapmf scrape https://www.tiktok.com/@user -o ~/scrapmf --cookies-from-browser brave\n  scrapmf doctor\n  scrapmf plugins list"
)]
pub struct Cli {
    /// Increase verbosity (-v, -vv)
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scrape a URL using the configured backend
    #[command(
        after_help = "Examples:\n  scrapmf scrape https://www.instagram.com/user/ --dry-run\n  scrapmf scrape https://www.tiktok.com/@user -o ~/scrapmf --cookies-from-browser brave\n  scrapmf scrape URL --no-archive"
    )]
    Scrape {
        /// URL to scrape
        url: String,

        /// Output directory (overrides site/global output)
        #[arg(long, short = 'o', value_name = "PATH")]
        output: Option<PathBuf>,

        /// Preset from presets/ (forces a site config)
        #[arg(long, value_name = "NAME")]
        preset: Option<String>,

        /// Cookies file (Netscape cookies.txt)
        #[arg(long, value_name = "FILE")]
        cookies: Option<PathBuf>,

        /// Cookies from browser (e.g. firefox, brave, chrome)
        #[arg(long, value_name = "BROWSER")]
        cookies_from_browser: Option<String>,

        /// Do not download, only print what would be done
        #[arg(long)]
        dry_run: bool,

        /// Disable the download archive (dedup) for this run
        #[arg(long)]
        no_archive: bool,

        /// Threads only: download profile picture instead of posts
        #[arg(long)]
        profile_pic_only: bool,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },

    /// Check backends and system
    Doctor,

    /// List available sites and their backends
    Sites {
        #[command(subcommand)]
        command: Option<SiteCommands>,
    },

    /// Manage cookie profiles
    Profiles {
        #[command(subcommand)]
        command: Option<ProfileCommands>,
    },

    /// Manage plugins (threads, tiktok, termux-scan)
    Plugins {
        #[command(subcommand)]
        command: Option<PluginCommands>,
    },

    /// Install the bundled pinned gallery-dl backend
    ///
    /// Hidden from `--help`: offered automatically on first interactive run.
    #[command(hide = true)]
    Setup {
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SiteCommands {
    /// List registered sites
    List,
}

#[derive(Subcommand, Debug)]
pub enum ProfileCommands {
    /// List stored cookie profiles
    List,
}

#[derive(Subcommand, Debug)]
pub enum PluginCommands {
    /// List plugins and their state
    List,
    /// Enable (install) a plugin: threads | tiktok
    Enable {
        /// Plugin id
        id: String,
    },
    /// Disable a plugin (keeps files)
    Disable {
        /// Plugin id
        id: String,
    },
    /// Remove a plugin completely
    Remove {
        /// Plugin id
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// List current configuration
    List,
    /// Print config file path
    Path,
    /// Edit config file with $EDITOR
    Edit,
}
