use anyhow::Result;

pub fn run(command: Option<crate::cli::args::SiteCommands>) -> Result<()> {
    match command {
        Some(crate::cli::args::SiteCommands::List) | None => {
            for s in crate::sites::registry::REGISTRY {
                println!(
                    "{} ({}) - {} - kinds: {}",
                    s.id,
                    s.display,
                    match s.backend {
                        crate::sites::registry::BackendKind::GalleryDl => "gallery-dl",
                        crate::sites::registry::BackendKind::Threadstractor => "threadstractor",
                    },
                    s.content_kinds.join(", ")
                );
            }
        }
    }
    Ok(())
}
