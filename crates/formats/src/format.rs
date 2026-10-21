//! Declarative formats: a mode script, its settings, points and maps, described in TOML.
//!
//! ```toml
//! id = "cup-short"
//! mode = "cup"
//! name = { en = "Short cup", fr = "Cup courte" }
//! points_repartition = [10, 6, 4, 3, 2, 1]
//!
//! [settings]
//! S_PointsLimit = 60
//! S_RoundsPerMap = 3
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use pitwall_core::gbx::types::{ScriptSetting, ScriptSettings};
use pitwall_core::{Lang, ModeSwitch};
use serde::{Deserialize, Serialize};

use crate::mode::{find_mode, mode_of_script, resolve_script};

/// A text in both languages (missing ones fall back to the other).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Localized {
    pub en: String,
    pub fr: String,
}

impl Localized {
    pub fn get(&self, lang: Lang) -> &str {
        let (first, second) = match lang {
            Lang::Fr => (&self.fr, &self.en),
            Lang::En => (&self.en, &self.fr),
        };
        if first.is_empty() { second } else { first }
    }
}

/// A format definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatDef {
    /// Unique id, used in `/mode <id>` and cup definitions.
    pub id: String,
    #[serde(default)]
    pub name: Localized,
    #[serde(default)]
    pub description: Localized,
    /// Mode id/alias (`cup`, `ta`, ...) or a custom script path.
    pub mode: String,
    #[serde(default)]
    pub settings: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub points_repartition: Option<Vec<i32>>,
    /// Map files (relative to the Maps directory); empty keeps the server playlist.
    #[serde(default)]
    pub maps: Vec<String>,
}

/// Errors in format definitions.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("format {0:?} not found")]
    NotFound(String),
    #[error("format {id}: unknown mode {mode:?} (use a known id like `cup` or a script path)")]
    UnknownMode { id: String, mode: String },
    #[error("format {id}: setting {name} must be a boolean, an integer, a float or a string")]
    BadSetting { id: String, name: String },
    #[error("format {id}: setting {name} does not start with `S_`")]
    BadSettingName { id: String, name: String },
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid format file {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

impl FormatDef {
    /// Parses and validates a TOML definition.
    pub fn from_toml(source: &str, path: &str) -> Result<FormatDef, FormatError> {
        let def: FormatDef = toml::from_str(source).map_err(|source| FormatError::Parse {
            path: path.to_owned(),
            source,
        })?;
        def.validate()?;
        Ok(def)
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        self.script()?;
        self.script_settings()?;
        Ok(())
    }

    /// Mode script to load.
    pub fn script(&self) -> Result<String, FormatError> {
        resolve_script(&self.mode).ok_or_else(|| FormatError::UnknownMode {
            id: self.id.clone(),
            mode: self.mode.clone(),
        })
    }

    /// `true` if the mode uses blue/red teams.
    pub fn uses_teams(&self) -> bool {
        find_mode(&self.mode)
            .or_else(|| self.script().ok().and_then(|s| mode_of_script(&s)))
            .is_some_and(|m| m.teams)
    }

    /// Typed `S_*` settings.
    pub fn script_settings(&self) -> Result<ScriptSettings, FormatError> {
        self.settings
            .iter()
            .map(|(name, value)| {
                if !name.starts_with("S_") {
                    return Err(FormatError::BadSettingName {
                        id: self.id.clone(),
                        name: name.clone(),
                    });
                }
                Ok((
                    name.clone(),
                    toml_to_setting(value).ok_or_else(|| FormatError::BadSetting {
                        id: self.id.clone(),
                        name: name.clone(),
                    })?,
                ))
            })
            .collect()
    }

    /// Builds the mode switch, with extra settings and maps overriding the definition.
    pub fn to_mode_switch(
        &self,
        overrides: &ScriptSettings,
        maps: Option<&[String]>,
    ) -> Result<ModeSwitch, FormatError> {
        let mut settings = self.script_settings()?;
        settings.extend(overrides.iter().map(|(k, v)| (k.clone(), v.clone())));
        Ok(ModeSwitch {
            script: self.script()?,
            settings,
            maps: maps.map_or_else(|| self.maps.clone(), <[String]>::to_vec),
            points_repartition: self.points_repartition.clone(),
            label: self.id.clone(),
        })
    }

    pub fn display_name(&self, lang: Lang) -> &str {
        let name = self.name.get(lang);
        if name.is_empty() { &self.id } else { name }
    }
}

/// Converts a TOML value into a script setting.
pub fn toml_to_setting(value: &toml::Value) -> Option<ScriptSetting> {
    Some(match value {
        toml::Value::Boolean(b) => ScriptSetting::Bool(*b),
        toml::Value::Integer(i) => ScriptSetting::Int(i32::try_from(*i).ok()?),
        toml::Value::Float(f) => ScriptSetting::Real(*f),
        toml::Value::String(s) => ScriptSetting::Text(s.clone()),
        _ => return None,
    })
}

/// Parses a setting typed on the command line (`true`, `42`, `0.5`, anything else is text).
pub fn parse_setting(raw: &str) -> ScriptSetting {
    match raw {
        "true" | "on" => ScriptSetting::Bool(true),
        "false" | "off" => ScriptSetting::Bool(false),
        _ => raw
            .parse::<i32>()
            .map(ScriptSetting::Int)
            .or_else(|_| raw.parse::<f64>().map(ScriptSetting::Real))
            .unwrap_or_else(|_| ScriptSetting::Text(raw.to_owned())),
    }
}

/// Built-in formats shipped in the binary.
const BUILTIN: &[(&str, &str)] = &[
    (
        "timeattack.toml",
        include_str!("../builtin/timeattack.toml"),
    ),
    ("rounds.toml", include_str!("../builtin/rounds.toml")),
    ("cup.toml", include_str!("../builtin/cup.toml")),
    ("teams.toml", include_str!("../builtin/teams.toml")),
    ("knockout.toml", include_str!("../builtin/knockout.toml")),
    ("laps.toml", include_str!("../builtin/laps.toml")),
    ("tmwt.toml", include_str!("../builtin/tmwt.toml")),
    ("royal.toml", include_str!("../builtin/royal.toml")),
];

/// All available formats, by id.
#[derive(Debug, Clone, Default)]
pub struct FormatRegistry {
    formats: BTreeMap<String, FormatDef>,
    /// Ids offered to admins (`/mode`, `/formats`, admin panel, dashboard); the others stay
    /// loaded for cups. `None`: all of them.
    offered: Option<BTreeSet<String>>,
}

impl FormatRegistry {
    /// Built-in formats only.
    pub fn builtin() -> Result<FormatRegistry, FormatError> {
        let mut registry = FormatRegistry::default();
        for (path, source) in BUILTIN {
            registry.insert(FormatDef::from_toml(source, path)?);
        }
        Ok(registry)
    }

    /// Built-in formats plus every `*.toml` of `dir` (same id overrides the built-in one).
    pub fn load(dir: Option<&Path>) -> Result<FormatRegistry, FormatError> {
        let mut registry = Self::builtin()?;
        let Some(dir) = dir else {
            return Ok(registry);
        };
        if !dir.exists() {
            tracing::info!(dir = %dir.display(), "no custom format directory");
            return Ok(registry);
        }
        let io = |source| FormatError::Io {
            path: dir.display().to_string(),
            source,
        };
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map_err(io)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let shown = path.display().to_string();
            let source = std::fs::read_to_string(&path).map_err(|source| FormatError::Io {
                path: shown.clone(),
                source,
            })?;
            let def = FormatDef::from_toml(&source, &shown)?;
            tracing::info!(id = %def.id, path = %shown, "loaded format");
            registry.insert(def);
        }
        Ok(registry)
    }

    pub fn insert(&mut self, def: FormatDef) {
        self.formats.insert(def.id.to_ascii_lowercase(), def);
    }

    /// Finds a format by id, or by mode alias (`ta` → the `timeattack` format).
    pub fn get(&self, id: &str) -> Option<&FormatDef> {
        let id = id.to_ascii_lowercase();
        self.formats.get(&id).or_else(|| {
            let mode = find_mode(&id)?;
            self.formats.get(mode.id)
        })
    }

    pub fn all(&self) -> impl Iterator<Item = &FormatDef> {
        self.formats.values()
    }

    /// Offers only these formats to admins (ids or mode aliases); empty offers all of them.
    pub fn offer_only(&mut self, ids: &[String]) -> Result<(), FormatError> {
        if ids.is_empty() {
            self.offered = None;
            return Ok(());
        }
        let offered = ids
            .iter()
            .map(|id| {
                self.get(id)
                    .map(|def| def.id.to_ascii_lowercase())
                    .ok_or_else(|| FormatError::NotFound(id.clone()))
            })
            .collect::<Result<_, _>>()?;
        self.offered = Some(offered);
        Ok(())
    }

    /// Formats offered to admins.
    pub fn offered(&self) -> impl Iterator<Item = &FormatDef> {
        self.formats
            .iter()
            .filter(|(id, _)| self.offered.as_ref().is_none_or(|o| o.contains(*id)))
            .map(|(_, def)| def)
    }

    /// Like [`FormatRegistry::get`], among the offered formats only.
    pub fn get_offered(&self, id: &str) -> Option<&FormatDef> {
        self.get(id).filter(|def| {
            self.offered
                .as_ref()
                .is_none_or(|o| o.contains(&def.id.to_ascii_lowercase()))
        })
    }

    /// `apply_format("name")`: the mode switch for a format.
    pub fn apply_format(&self, id: &str) -> Result<ModeSwitch, FormatError> {
        self.get(id)
            .ok_or_else(|| FormatError::NotFound(id.to_owned()))?
            .to_mode_switch(&ScriptSettings::new(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_formats_are_valid() {
        let registry = FormatRegistry::builtin().unwrap();
        for id in [
            "timeattack",
            "rounds",
            "cup",
            "teams",
            "knockout",
            "laps",
            "tmwt",
            "royal",
        ] {
            assert!(registry.get(id).is_some(), "missing {id}");
        }
        assert_eq!(registry.get("ta").unwrap().id, "timeattack");
        assert!(registry.get("teams").unwrap().uses_teams());
        assert!(!registry.get("cup").unwrap().uses_teams());
    }

    #[test]
    fn apply_format_builds_a_mode_switch() {
        let registry = FormatRegistry::builtin().unwrap();
        let switch = registry.apply_format("cup").unwrap();
        assert_eq!(switch.script, "Trackmania/TM_Cup_Online.Script.txt");
        assert_eq!(switch.settings["S_PointsLimit"], ScriptSetting::Int(100));
        assert!(switch.maps.is_empty());
        assert!(matches!(
            registry.apply_format("nope"),
            Err(FormatError::NotFound(_))
        ));
    }

    #[test]
    fn custom_format_with_overrides() {
        let def = FormatDef::from_toml(
            r#"
id = "mini-cup"
mode = "cup"
name = { fr = "Mini coupe" }
points_repartition = [10, 6, 4]
maps = ["Campaigns/A01.Map.Gbx"]
[settings]
S_PointsLimit = 40
S_UseTieBreak = false
"#,
            "mini.toml",
        )
        .unwrap();
        assert_eq!(def.display_name(Lang::En), "Mini coupe");
        let mut overrides = ScriptSettings::new();
        overrides.insert("S_PointsLimit".into(), ScriptSetting::Int(60));
        let switch = def.to_mode_switch(&overrides, None).unwrap();
        assert_eq!(switch.settings["S_PointsLimit"], ScriptSetting::Int(60));
        assert_eq!(switch.settings["S_UseTieBreak"], ScriptSetting::Bool(false));
        assert_eq!(switch.maps, vec!["Campaigns/A01.Map.Gbx".to_owned()]);
        assert_eq!(switch.points_repartition, Some(vec![10, 6, 4]));
    }

    #[test]
    fn rejects_invalid_definitions() {
        let err = FormatDef::from_toml("id = \"x\"\nmode = \"nope\"", "x.toml").unwrap_err();
        assert!(matches!(err, FormatError::UnknownMode { .. }));
        let err = FormatDef::from_toml(
            "id = \"x\"\nmode = \"cup\"\n[settings]\nS_A = [1]",
            "x.toml",
        )
        .unwrap_err();
        assert!(matches!(err, FormatError::BadSetting { .. }));
        let err = FormatDef::from_toml(
            "id = \"x\"\nmode = \"cup\"\n[settings]\nPoints = 1",
            "x.toml",
        )
        .unwrap_err();
        assert!(matches!(err, FormatError::BadSettingName { .. }));
    }

    #[test]
    fn offers_a_subset() {
        let mut registry = FormatRegistry::builtin().unwrap();
        registry
            .offer_only(&["ta".to_owned(), "rounds".to_owned()])
            .unwrap();
        let ids: Vec<&str> = registry.offered().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["rounds", "timeattack"]);
        assert!(registry.get_offered("ta").is_some());
        assert!(registry.get_offered("cup").is_none());
        // Still loaded for cups.
        assert!(registry.get("cup").is_some());
        assert!(matches!(
            registry.offer_only(&["nope".to_owned()]),
            Err(FormatError::NotFound(_))
        ));
        registry.offer_only(&[]).unwrap();
        assert_eq!(registry.offered().count(), registry.all().count());
    }

    #[test]
    fn parses_command_line_settings() {
        assert_eq!(parse_setting("true"), ScriptSetting::Bool(true));
        assert_eq!(parse_setting("300"), ScriptSetting::Int(300));
        assert_eq!(parse_setting("0.5"), ScriptSetting::Real(0.5));
        assert_eq!(
            parse_setting("10,6,4"),
            ScriptSetting::Text("10,6,4".into())
        );
    }
}
