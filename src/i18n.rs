use std::collections::HashMap;
use std::sync::OnceLock;

static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
static ES: OnceLock<HashMap<String, String>> = OnceLock::new();

fn load(json: &str) -> HashMap<String, String> {
    serde_json::from_str(json).unwrap_or_default()
}

fn en() -> &'static HashMap<String, String> {
    EN.get_or_init(|| load(include_str!("locales/en.json")))
}

fn es() -> &'static HashMap<String, String> {
    ES.get_or_init(|| load(include_str!("locales/es.json")))
}

/// Translate a key using the configured language, falling back to English,
/// then to the key itself.
pub fn t(key: &str) -> String {
    let lang = crate::config::load().unwrap_or_default().general.language;
    let map = if lang == "es" { es() } else { en() };
    map.get(key)
        .cloned()
        .or_else(|| en().get(key).cloned())
        .unwrap_or_else(|| key.to_string())
}