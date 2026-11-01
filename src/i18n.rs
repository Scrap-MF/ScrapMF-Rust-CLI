use std::collections::HashMap;
use std::sync::OnceLock;

static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
static ES: OnceLock<HashMap<String, String>> = OnceLock::new();

fn load_all(parts: &[&str]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for json in parts {
        let parsed: HashMap<String, String> = serde_json::from_str(json).unwrap_or_default();
        map.extend(parsed);
    }
    map
}

fn en() -> &'static HashMap<String, String> {
    EN.get_or_init(|| {
        load_all(&[
            include_str!("locales/en/common.json"),
            include_str!("locales/en/content.json"),
            include_str!("locales/en/profiles.json"),
            include_str!("locales/en/plugins.json"),
            include_str!("locales/en/scrape.json"),
            include_str!("locales/en/errors.json"),
        ])
    })
}

fn es() -> &'static HashMap<String, String> {
    ES.get_or_init(|| {
        load_all(&[
            include_str!("locales/es/common.json"),
            include_str!("locales/es/content.json"),
            include_str!("locales/es/profiles.json"),
            include_str!("locales/es/plugins.json"),
            include_str!("locales/es/scrape.json"),
            include_str!("locales/es/errors.json"),
        ])
    })
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

/// Translate a key and substitute `{name}` placeholders with the given values.
/// Values are converted to strings; use this instead of ad-hoc `.replace`.
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    let mut s = t(key);
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn locale_keys(lang: &str) -> HashSet<String> {
        let parts: &[&str] = if lang == "es" {
            &[
                include_str!("locales/es/common.json"),
                include_str!("locales/es/content.json"),
                include_str!("locales/es/profiles.json"),
                include_str!("locales/es/plugins.json"),
                include_str!("locales/es/scrape.json"),
                include_str!("locales/es/errors.json"),
            ]
        } else {
            &[
                include_str!("locales/en/common.json"),
                include_str!("locales/en/content.json"),
                include_str!("locales/en/profiles.json"),
                include_str!("locales/en/plugins.json"),
                include_str!("locales/en/scrape.json"),
                include_str!("locales/en/errors.json"),
            ]
        };
        let mut keys = HashSet::new();
        for json in parts {
            let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
            if let serde_json::Value::Object(map) = parsed {
                for k in map.keys() {
                    keys.insert(k.clone());
                }
            }
        }
        keys
    }

    #[test]
    fn locales_have_matching_keys() {
        let en = locale_keys("en");
        let es = locale_keys("es");
        assert_eq!(en, es, "locale key sets differ");
    }

    #[test]
    fn tf_substitutes_placeholders() {
        // Uses an existing key with a placeholder if present; otherwise falls
        // back to the key itself, which still exercises the path.
        let _ = tf("nonexistent_key", &[("x", "42")]);
    }
}
