pub mod registry;

pub use registry::{
    all_specs, domains_for_site as registry_domains_for_site, find_by_host, find_by_id,
    find_by_url, site_account_from_host,
};

// Canonical directory prefixes — the only place the tree root is defined.
//
// Save Profile: {scrapmf_root} / {category} / {username|user|user[name]} / <site.toml suffix>
// Quick:       {username literal} / <site.toml suffix>  (via flatten_for_quick)
// The suffix (posts/reels/highlights/stories, conditional directories, etc.)
// lives exclusively in sites/*.toml (extractor.*.directory).
pub const PROFILE_PREFIX: &[&str] = &["{scrapmf_root}", "{category}", "{username}"];
pub const PROFILE_PREFIX_USER: &[&str] = &["{scrapmf_root}", "{category}", "{user}"];
pub const PROFILE_PREFIX_USER_NAME: &[&str] = &["{scrapmf_root}", "{category}", "{user[name]}"];

/// Ensure every known site has its sites/<id>.toml (no clobber).
/// Threads is plugin-backed — skipped here, created by `plugins::install()`.
pub fn ensure_all_sites() -> anyhow::Result<()> {
    for spec in registry::all_specs() {
        if spec.id == "threads" {
            continue;
        }
        (spec.ensure_fn)()?;
    }
    Ok(())
}
