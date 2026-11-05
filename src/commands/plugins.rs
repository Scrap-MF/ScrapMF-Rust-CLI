use anyhow::{Context, Result};

use crate::plugins;

pub fn run(command: Option<crate::cli::args::PluginCommands>) -> Result<()> {
    match command {
        Some(crate::cli::args::PluginCommands::List) => {
            println!("threads:   {:?}", plugins::threads_state());
            println!("tiktok:    {:?}", plugins::tiktok_state());
            println!("termux-scan: {:?}", plugins::termux_scan_state());
        }
        Some(crate::cli::args::PluginCommands::Enable { id }) => match id.as_str() {
            "threads" => plugins::install().context("enable threads plugin")?,
            "tiktok" => plugins::install_tiktok().context("enable tiktok plugin")?,
            other => anyhow::bail!("unknown plugin: {other}"),
        },
        Some(crate::cli::args::PluginCommands::Disable { id }) => match id.as_str() {
            "threads" => plugins::set_disabled(true).context("disable threads plugin")?,
            "tiktok" => plugins::set_tiktok_disabled(true).context("disable tiktok plugin")?,
            "termux-scan" => {
                plugins::set_termux_scan_disabled(true).context("disable termux-scan")?
            }
            other => anyhow::bail!("unknown plugin: {other}"),
        },
        Some(crate::cli::args::PluginCommands::Remove { id }) => match id.as_str() {
            "threads" => plugins::remove().context("remove threads plugin")?,
            "tiktok" => plugins::remove_tiktok().context("remove tiktok plugin")?,
            other => anyhow::bail!("unknown plugin: {other}"),
        },
        None => {
            println!("threads:   {:?}", plugins::threads_state());
            println!("tiktok:    {:?}", plugins::tiktok_state());
            println!("termux-scan: {:?}", plugins::termux_scan_state());
        }
    }
    Ok(())
}
