use anyhow::{Context, Result};
use inquire::{Select, Text};

use crate::cli::args::ConfigCommands;
use crate::config;

pub fn run(command: Option<ConfigCommands>) -> Result<()> {
    match command {
        Some(ConfigCommands::List) => {
            let Some(path) = config::config_path() else {
                anyhow::bail!(crate::i18n::t("cannot_determine_config_path"));
            };
            if !path.exists() {
                println!(
                    "{}",
                    crate::i18n::t("no_config_yet").replace("{}", &path.display().to_string())
                );
                println!("Run: scrapmf config edit");
                return Ok(());
            }
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            println!("{content}");
        }
        Some(ConfigCommands::Path) => {
            let Some(path) = config::config_path() else {
                anyhow::bail!(crate::i18n::t("cannot_determine_config_path"));
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
        anyhow::bail!(crate::i18n::t("cannot_determine_config_path"));
    };
    if !path.exists() {
        let cfg = config::Config::default();
        config::save(&cfg).context("create default config")?;
        println!(
            "{}",
            crate::i18n::t("created_default_config").replace("{}", &path.display().to_string())
        );
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
    println!("{}", crate::i18n::t("config_saved"));
    Ok(())
}

fn interactive() -> Result<()> {
    let mut cfg = config::load().unwrap_or_default();
    let show_label = crate::i18n::t("show_config_path");
    let edit_label = crate::i18n::t("edit_manually");
    let exit_label = crate::i18n::t("exit");
    let options = vec![
        "Default output directory".to_string(),
        show_label.clone(),
        edit_label.clone(),
        exit_label.clone(),
    ];
    let choice = Select::new(&crate::i18n::t("what_to_configure"), options)
        .prompt()
        .unwrap_or(exit_label.clone());

    match choice.as_str() {
        "Default output directory" => {
            let current = cfg.general.output_dir.display().to_string();
            let dir = Text::new("Default output directory:")
                .with_default(&current)
                .prompt()
                .context("output dir")?;
            if !dir.trim().is_empty() {
                cfg.general.output_dir = std::path::PathBuf::from(dir.trim());
                config::save(&cfg)?;
                println!("{}", crate::i18n::t("output_dir_saved"));
            }
        }
        s if s == show_label => {
            if let Some(p) = config::config_path() {
                println!("{}", p.display());
            }
        }
        s if s == edit_label => {
            edit()?;
        }
        _ => {
            println!("{}", crate::i18n::t("bye"));
        }
    }
    Ok(())
}
