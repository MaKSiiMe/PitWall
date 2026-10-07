//! Loads cup definitions (`*.toml`) and their Lua rules from a directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cup::def::{CupDef, CupDefError};
use crate::cup::rules::{CupRules, RulesError};
use crate::format::FormatRegistry;

/// A definition and where it was loaded from.
#[derive(Debug, Clone)]
pub struct LoadedCup {
    pub def: CupDef,
    pub path: PathBuf,
}

/// Errors while loading cups.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Def(#[from] CupDefError),
    #[error("cup {cup}, phase {phase}: unknown format {format:?}")]
    UnknownFormat {
        cup: String,
        phase: String,
        format: String,
    },
    #[error("cup {cup}: duplicate id (also in {other})")]
    Duplicate { cup: String, other: String },
    #[error("cup {cup}: rules {path}: {source}")]
    Rules {
        cup: String,
        path: String,
        #[source]
        source: RulesError,
    },
}

/// Every cup definition, by id.
#[derive(Debug, Clone, Default)]
pub struct CupLibrary {
    cups: BTreeMap<String, LoadedCup>,
}

impl CupLibrary {
    /// Loads `dir/*.toml` and checks their formats exist.
    pub fn load(dir: &Path, formats: &FormatRegistry) -> Result<CupLibrary, LibraryError> {
        let mut library = CupLibrary::default();
        if !dir.exists() {
            tracing::info!(dir = %dir.display(), "no cup directory");
            return Ok(library);
        }
        let io = |path: &Path, source| LibraryError::Io {
            path: path.display().to_string(),
            source,
        };
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| io(dir, e))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let source = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
            let def = CupDef::from_toml(&source, &path.display().to_string())?;
            for phase in &def.phases {
                if formats.get(&phase.format).is_none() {
                    return Err(LibraryError::UnknownFormat {
                        cup: def.id.clone(),
                        phase: phase.id.clone(),
                        format: phase.format.clone(),
                    });
                }
            }
            if let Some(other) = library.cups.get(&def.id) {
                return Err(LibraryError::Duplicate {
                    cup: def.id.clone(),
                    other: other.path.display().to_string(),
                });
            }
            let loaded = LoadedCup { def, path };
            // Fail early on broken rules.
            library_rules(&loaded)?;
            tracing::info!(id = %loaded.def.id, path = %loaded.path.display(), "loaded cup");
            library.cups.insert(loaded.def.id.clone(), loaded);
        }
        Ok(library)
    }

    pub fn get(&self, id: &str) -> Option<&LoadedCup> {
        self.cups.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &LoadedCup> {
        self.cups.values()
    }

    pub fn insert(&mut self, cup: LoadedCup) {
        self.cups.insert(cup.def.id.clone(), cup);
    }
}

/// Loads the Lua rules of a cup, if it has any.
pub fn library_rules(cup: &LoadedCup) -> Result<Option<Arc<dyn CupRules>>, LibraryError> {
    let Some(rules) = &cup.def.rules else {
        return Ok(None);
    };
    let base = cup.path.parent().unwrap_or_else(|| Path::new("."));
    load_rules(&cup.def.id, &base.join(rules))
}

/// Loads a rules file.
pub fn load_rules(cup: &str, path: &Path) -> Result<Option<Arc<dyn CupRules>>, LibraryError> {
    let shown = path.display().to_string();
    let source = std::fs::read_to_string(path).map_err(|source| LibraryError::Io {
        path: shown.clone(),
        source,
    })?;
    #[cfg(feature = "lua")]
    {
        let rules = crate::cup::rules::LuaRules::load(&source, &shown).map_err(|source| {
            LibraryError::Rules {
                cup: cup.to_owned(),
                path: shown,
                source,
            }
        })?;
        Ok(Some(Arc::new(rules)))
    }
    #[cfg(not(feature = "lua"))]
    {
        let _ = source;
        Err(LibraryError::Rules {
            cup: cup.to_owned(),
            path: shown,
            source: RulesError::Unsupported,
        })
    }
}
