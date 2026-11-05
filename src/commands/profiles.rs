use anyhow::Result;

pub fn run(command: Option<crate::cli::args::ProfileCommands>) -> Result<()> {
    match command {
        Some(crate::cli::args::ProfileCommands::List) | None => {
            let profiles = crate::config::cookies::list_profiles();
            if profiles.is_empty() {
                println!("no profiles");
            } else {
                for p in profiles {
                    println!("{p}");
                }
            }
        }
    }
    Ok(())
}
