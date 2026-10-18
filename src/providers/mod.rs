pub mod browser;
pub mod gallery_dl;
pub mod threadstractor;

use std::ffi::OsString;

pub use crate::application::ScrapeRequest;

/// Abstraction for external download backends (gallery-dl, yt-dlp).
pub trait Provider {
    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
    fn version(&self) -> anyhow::Result<String>;
    fn build_args(&self, req: &ScrapeRequest) -> anyhow::Result<Vec<OsString>>;
}
