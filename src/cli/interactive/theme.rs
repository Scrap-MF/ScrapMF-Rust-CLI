//! Clack/Vite-inspired visual theme for interactive prompts.
//!
//! Centralizes every visual decision: the inquire `RenderConfig` applied to
//! all prompts, brand colors for social-network menu items, and NO_COLOR
//! support (when set, everything degrades to plain output).

use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};

/// NO_COLOR convention: any non-empty value disables colors.
pub fn colors_enabled() -> bool {
    colors_enabled_from(std::env::var_os("NO_COLOR").map(|v| v.to_string_lossy().into_owned()))
}

fn colors_enabled_from(no_color: Option<String>) -> bool {
    match no_color {
        Some(v) => v.is_empty(),
        None => true,
    }
}

fn dim() -> StyleSheet {
    StyleSheet::new().with_fg(Color::DarkGrey)
}

fn fg(color: Color) -> StyleSheet {
    StyleSheet::new().with_fg(color)
}

/// Clack/Vite look: ◆ violet prompts, ❯ cyan pointer, green answers/checks,
/// dim help messages. When NO_COLOR is set this falls back to the stock
/// configuration (which is colorless under that convention anyway).
pub fn render_config() -> RenderConfig<'static> {
    if !colors_enabled() {
        return RenderConfig::default();
    }
    RenderConfig {
        prompt_prefix: Styled::new("◆").with_fg(Color::LightMagenta),
        answered_prompt_prefix: Styled::new("◇").with_fg(Color::DarkMagenta),
        answer: fg(Color::LightGreen),
        help_message: dim(),
        highlighted_option_prefix: Styled::new("❯").with_fg(Color::LightCyan),
        unhighlighted_option_prefix: Styled::new(" "),
        selected_option: Some(fg(Color::White)),
        selected_checkbox: Styled::new("[◉] ").with_fg(Color::LightGreen),
        unselected_checkbox: Styled::new("[ ] ").with_fg(Color::DarkGrey),
        ..RenderConfig::default()
    }
}

// ─── Brand-colored site labels ──────────────────────────────────────────────

const INSTAGRAM_PINK: anstyle::RgbColor = anstyle::RgbColor(225, 48, 108);
const TWITTER_BLUE: anstyle::RgbColor = anstyle::RgbColor(29, 161, 242);
const FACEBOOK_BLUE: anstyle::RgbColor = anstyle::RgbColor(24, 119, 242);

/// True-brand ANSI color per known site; None → render plain.
fn brand_color(site_key: &str) -> Option<anstyle::RgbColor> {
    match site_key {
        "instagram" => Some(INSTAGRAM_PINK),
        // TikTok's actual brand color is black — invisible on dark terminals;
        // bright white reads as its dark-mode identity instead.
        "tiktok" => Some(anstyle::RgbColor(255, 255, 255)),
        "twitter" => Some(TWITTER_BLUE),
        "x" => Some(TWITTER_BLUE),
        "vsco" => Some(anstyle::RgbColor(254, 232, 158)),
        "facebook" => Some(FACEBOOK_BLUE),
        "fb" => Some(FACEBOOK_BLUE),
        _ => None,
    }
}

fn paint(text: &str, color: anstyle::RgbColor) -> String {
    let style = anstyle::Style::new().fg_color(Some(anstyle::Color::Rgb(color)));
    format!("{}{text}{}", style.render(), anstyle::Reset.render())
}

/// Menu label for a social-network site key, painted with its brand color
/// (Instagram pink, Twitter/X blue, TikTok white…). Unknown keys render plain.
pub fn brand_site_label(site_key: &str) -> String {
    brand_site_label_inner(site_key, colors_enabled())
}

fn brand_site_label_inner(site_key: &str, colors: bool) -> String {
    if !colors {
        return site_key.to_string();
    }
    match brand_color(site_key) {
        Some(c) => paint(site_key, c),
        None => site_key.to_string(),
    }
}

/// Paints only the network part of an `site:username` account label with
/// its brand color ("instagram:sample_user"). Labels without a colon or with
/// an unknown network render unchanged.
pub fn brand_account_label(label: &str) -> String {
    brand_account_label_inner(label, colors_enabled())
}

fn brand_account_label_inner(label: &str, colors: bool) -> String {
    if !colors {
        return label.to_string();
    }
    match label.split_once(':') {
        Some((site, rest)) => match brand_color(site) {
            Some(c) => format!("{}:{rest}", paint(site, c)),
            None => label.to_string(),
        },
        None => label.to_string(),
    }
}

/// Option for site-selection menus: displays brand-colored when it is a
/// network key, verbatim otherwise ("Create new site", "Back", …).
#[derive(Clone)]
pub struct SiteItem {
    pub key: String,
}

impl SiteItem {
    pub fn new(key: impl Into<String>) -> Self {
        SiteItem { key: key.into() }
    }

    /// The clean key (no ANSI codes) — safe for config lookups and argv.
    pub fn key(&self) -> String {
        self.key.clone()
    }
}

impl std::fmt::Display for SiteItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", brand_site_label(&self.key))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn no_color_disables_everything() {
        assert!(!colors_enabled_from(Some("1".into())));
        assert!(colors_enabled_from(Some(String::new()))); // empty = colors stay
        assert!(colors_enabled_from(None));
    }

    #[test]
    fn brand_labels_paint_known_sites_and_leave_unknown_plain() {
        let pink = brand_site_label_inner("instagram", true);
        assert!(pink.contains("\u{1b}["));
        assert!(pink.contains("instagram"));

        assert_eq!(brand_site_label_inner("unknown-site", true), "unknown-site");
        assert_eq!(brand_site_label_inner("instagram", false), "instagram");
    }

    #[test]
    fn brand_account_label_paints_site_part_only() {
        let out = brand_account_label_inner("instagram:sample_user", true);
        assert!(out.contains("\u{1b}["), "site part colored");
        assert!(out.ends_with(":sample_user"), "username untouched: {out}");

        assert_eq!(brand_account_label_inner("unknown:x", true), "unknown:x");
        assert_eq!(brand_account_label_inner("instagram", false), "instagram");
        assert_eq!(
            brand_account_label_inner("no-colon-label", true),
            "no-colon-label"
        );
    }

    #[test]
    fn site_item_display_brands_but_key_stays_clean() {
        let branded = SiteItem::new("tiktok");
        assert!(branded.to_string().contains("\u{1b}["), "colored display");
        assert_eq!(branded.key(), "tiktok", "key must be clean for lookups");

        let plain = SiteItem::new("Back");
        assert_eq!(plain.to_string(), "Back");
        assert_eq!(plain.key(), "Back");
    }
}
