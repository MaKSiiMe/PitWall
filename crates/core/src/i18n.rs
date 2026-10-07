//! Translations (FR + EN) for every text shown to players.
//!
//! Catalogs are flat TOML tables (`key = "text with {placeholders}"`). Core ships its own catalog;
//! plugins add theirs at startup. Missing keys fall back to English, then to the key itself.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// Supported languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Fr];

    /// Parses a language code as reported by the game (`fr`, `fr-FR`, `en_US`, ...).
    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match primary.as_str() {
            "fr" => Some(Lang::Fr),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A text to show to someone: either verbatim or a translation key with arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Text {
    Raw(String),
    Key {
        key: String,
        #[serde(default)]
        args: Vec<(String, String)>,
    },
}

impl Text {
    /// Verbatim text, shown as is in every language.
    pub fn raw(text: impl Into<String>) -> Text {
        Text::Raw(text.into())
    }

    /// Translation key.
    pub fn tr(key: impl Into<String>) -> Text {
        Text::Key {
            key: key.into(),
            args: Vec::new(),
        }
    }

    /// Adds a `{name}` argument (no-op on raw texts).
    pub fn arg(mut self, name: impl Into<String>, value: impl ToString) -> Text {
        if let Text::Key { args, .. } = &mut self {
            args.push((name.into(), value.to_string()));
        }
        self
    }

    /// `true` when the rendering does not depend on the language.
    pub fn is_raw(&self) -> bool {
        matches!(self, Text::Raw(_))
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Text {
        Text::raw(s)
    }
}

impl From<String> for Text {
    fn from(s: String) -> Text {
        Text::raw(s)
    }
}

/// Errors while loading a catalog.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("invalid translation catalog: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("translation {key:?} must be a string")]
    NotAString { key: String },
}

/// Thread-safe translation catalogs.
#[derive(Debug, Clone, Default)]
pub struct Translator {
    catalogs: Arc<RwLock<HashMap<Lang, HashMap<String, String>>>>,
}

impl Translator {
    /// A translator preloaded with the core catalogs.
    pub fn with_core_catalogs() -> Result<Translator, CatalogError> {
        let t = Translator::default();
        t.add_catalog(Lang::En, include_str!("../locales/en.toml"))?;
        t.add_catalog(Lang::Fr, include_str!("../locales/fr.toml"))?;
        Ok(t)
    }

    /// Adds (or overrides) entries from a TOML catalog. Nested tables become dotted keys.
    pub fn add_catalog(&self, lang: Lang, toml_source: &str) -> Result<(), CatalogError> {
        let table: toml::Table = toml::from_str(toml_source)?;
        let mut flat = HashMap::new();
        flatten("", &table, &mut flat)?;
        let mut catalogs = self
            .catalogs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        catalogs.entry(lang).or_default().extend(flat);
        Ok(())
    }

    /// Renders a text in a language.
    pub fn render(&self, lang: Lang, text: &Text) -> String {
        match text {
            Text::Raw(s) => s.clone(),
            Text::Key { key, args } => {
                let catalogs = self
                    .catalogs
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let template = catalogs
                    .get(&lang)
                    .and_then(|c| c.get(key))
                    .or_else(|| catalogs.get(&Lang::En).and_then(|c| c.get(key)));
                match template {
                    Some(template) => substitute(template, args),
                    None => {
                        tracing::debug!(key, %lang, "missing translation");
                        key.clone()
                    }
                }
            }
        }
    }

    /// Shortcut for `render(lang, &Text::tr(key))`.
    pub fn t(&self, lang: Lang, key: &str) -> String {
        self.render(lang, &Text::tr(key))
    }
}

fn flatten(
    prefix: &str,
    table: &toml::Table,
    out: &mut HashMap<String, String>,
) -> Result<(), CatalogError> {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::String(s) => {
                out.insert(key, s.clone());
            }
            toml::Value::Table(t) => flatten(&key, t, out)?,
            _ => return Err(CatalogError::NotAString { key }),
        }
    }
    Ok(())
}

fn substitute(template: &str, args: &[(String, String)]) -> String {
    let mut out = template.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_codes() {
        assert_eq!(Lang::from_code("fr-FR"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("en_US"), Some(Lang::En));
        assert_eq!(Lang::from_code("de"), None);
    }

    #[test]
    fn renders_with_fallbacks() {
        let t = Translator::default();
        t.add_catalog(
            Lang::En,
            "hello = \"Hello {name}\"\n[cup]\nstart = \"Cup starts\"",
        )
        .unwrap();
        t.add_catalog(Lang::Fr, "hello = \"Bonjour {name}\"")
            .unwrap();
        let hello = Text::tr("hello").arg("name", "Max");
        assert_eq!(t.render(Lang::Fr, &hello), "Bonjour Max");
        assert_eq!(t.render(Lang::En, &hello), "Hello Max");
        // Missing in French → English.
        assert_eq!(t.t(Lang::Fr, "cup.start"), "Cup starts");
        // Missing everywhere → the key.
        assert_eq!(t.t(Lang::Fr, "nope"), "nope");
        assert_eq!(t.render(Lang::Fr, &Text::raw("as is")), "as is");
    }

    #[test]
    fn core_catalogs_have_the_same_keys() {
        let en: toml::Table = toml::from_str(include_str!("../locales/en.toml")).unwrap();
        let fr: toml::Table = toml::from_str(include_str!("../locales/fr.toml")).unwrap();
        let (mut a, mut b) = (HashMap::new(), HashMap::new());
        flatten("", &en, &mut a).unwrap();
        flatten("", &fr, &mut b).unwrap();
        let mut missing: Vec<_> = a.keys().filter(|k| !b.contains_key(*k)).collect();
        missing.extend(b.keys().filter(|k| !a.contains_key(*k)));
        assert!(
            missing.is_empty(),
            "keys present in only one language: {missing:?}"
        );
    }
}
