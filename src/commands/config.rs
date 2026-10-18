use anyhow::{Context, Result};
use inquire::{Select, Text};

use crate::cli::args::ConfigCommands;
use crate::config;

pub fn run(command: Option<ConfigCommands>) -> Result<()> {
    match command {
        Some(ConfigCommands::List) => {
            let Some(path) = config::config_path() else {
                anyhow::bail!("cannot determine config path");
            };
            if !path.exists() {
                println!("No config yet at {}", path.display());
                println!("Run: scrapmf config edit");
                return Ok(());
            }
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            println!("{content}");
        }
        Some(ConfigCommands::Path) => {
            let Some(path) = config::config_path() else {
                anyhow::bail!("cannot determine config path");
            };
            println!("{}", path.display());
        }
        Some(ConfigCommands::Edit) => {
            edit()?;
        }
        None => {
            // Interactive mode
            interactive()?;
        }
    }
    Ok(())
}

fn edit() -> Result<()> {
    let Some(path) = config::config_path() else {
        anyhow::bail!("cannot determine config path");
    };
    if !path.exists() {
        let cfg = config::Config::default();
        config::save(&cfg).context("create default config")?;
        println!("Created default config at {}", path.display());
    }
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    if which::which(&editor).is_err() {
        anyhow::bail!("editor '{editor}' not found in $PATH");
    }
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("launch editor {editor}"))?;
    if !status.success() {
        anyhow::bail!("editor exited with {:?}", status.code());
    }
    // Validate after edit
    let s = std::fs::read_to_string(&path).context("read after edit")?;
    toml::from_str::<config::Config>(&s).context("config parse error after edit")?;
    println!("✔ Config saved");
    Ok(())
}

fn interactive() -> Result<()> {
    let mut cfg = config::load().unwrap_or_default();
    let options = vec![
        "Default output directory",
        "Show config path",
        "Edit manually",
        "Exit",
    ];
    let choice = Select::new("What to configure?", options)
        .prompt()
        .unwrap_or("Exit");

    match choice {
        "Default output directory" => {
            let current = cfg.general.output_dir.display().to_string();
            let dir = Text::new("Default output directory:")
                .with_default(&current)
                .prompt()
                .context("output dir")?;
            if !dir.trim().is_empty() {
                cfg.general.output_dir = std::path::PathBuf::from(dir.trim());
                config::save(&cfg)?;
                println!("✔ Output dir saved");
            }
        }
        "Show config path" => {
            if let Some(p) = config::config_path() {
                println!("{}", p.display());
            }
        }
        "Edit manually" => {
            edit()?;
        }
        _ => {
            println!("bye");
        }
    }
    Ok(())
}
