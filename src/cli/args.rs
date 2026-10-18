use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "scrapmf",
    version,
    about = "Safe, interactive archiver for social media galleries",
    long_about = None,
    arg_required_else_help = false
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
    Scrape {
        /// URL to scrape
        url: String,

        /// Output directory
        #[arg(long, short = 'o', value_name = "PATH")]
        output: Option<PathBuf>,

        /// Preset from presets/
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

    /// Install the bundled pinned gallery-dl backend
    ///
    /// Hidden from `--help`: the install is offered automatically on first
    /// interactive run, so the command is only needed as an advanced/manual
    /// entry point (kept for scripts and recovery).
    #[command(hide = true)]
    Setup {
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
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
