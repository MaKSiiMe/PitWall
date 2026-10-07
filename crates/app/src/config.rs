//! Configuration: `pitwall.toml`, then `PITWALL_*` environment variables (`__` = nesting).
//!
//! Secrets (XML-RPC password, API keys) belong in the environment, never in the file:
//! `PITWALL_SERVER__PASSWORD=... pitwall`.

use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use pitwall_core::{Lang, RolesConfig, ServerConfig};
use serde::Deserialize;

/// Whole configuration file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub controller: ControllerSection,
    pub roles: RolesConfig,
    pub log: LogSection,
    /// `[plugins.<name>]` sections, passed to each plugin.
    pub plugins: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ControllerSection {
    /// Language of the console and of players whose language is unknown.
    pub default_lang: Lang,
    /// Announce joins/leaves in the chat.
    pub announce_joins: bool,
    /// Directory of custom format definitions (`*.toml`).
    pub formats_dir: PathBuf,
    /// Directory of cup definitions (`*.toml` + Lua rules).
    pub cups_dir: PathBuf,
    /// Directory for runtime data (database, exports).
    pub data_dir: PathBuf,
    /// Read commands from stdin.
    pub console: bool,
}

impl Default for ControllerSection {
    fn default() -> Self {
        Self {
            default_lang: Lang::En,
            announce_joins: true,
            formats_dir: PathBuf::from("config/formats"),
            cups_dir: PathBuf::from("config/cups"),
            data_dir: PathBuf::from("data"),
            console: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LogSection {
    /// `tracing` filter, overridden by `RUST_LOG`.
    pub filter: String,
    /// `pretty` or `json`.
    pub format: LogFormat,
}

impl Default for LogSection {
    fn default() -> Self {
        Self {
            filter: "info".to_owned(),
            format: LogFormat::Pretty,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid configuration: {0}")]
    Invalid(#[from] Box<figment::Error>),
    #[error(
        "the XML-RPC password is empty: set PITWALL_SERVER__PASSWORD (the SuperAdmin password of the dedicated server)"
    )]
    MissingPassword,
}

/// Loads the file (if it exists) and the environment.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    load_from(Figment::new().merge(Toml::file(path)))
}

fn load_from(base: Figment) -> Result<Config, ConfigError> {
    let config: Config = base
        .merge(Env::prefixed("PITWALL_").split("__"))
        .extract()
        .map_err(Box::new)?;
    if config.server.password.is_empty() {
        return Err(ConfigError::MissingPassword);
    }
    Ok(config)
}

#[cfg(test)]
#[allow(clippy::result_large_err)] // figment::Jail closures return figment::Error
mod tests {
    use super::*;

    #[test]
    fn file_then_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "pitwall.toml",
                r#"
[server]
port = 5001
[controller]
default_lang = "fr"
[roles]
admins = ["boss"]
[plugins.logger]
skip = ["checkpoint"]
"#,
            )?;
            jail.set_env("PITWALL_SERVER__PASSWORD", "from-env");
            jail.set_env("PITWALL_SERVER__HOST", "trackmania");
            let config = load(Path::new("pitwall.toml")).map_err(|e| e.to_string())?;
            assert_eq!(config.server.port, 5001);
            assert_eq!(config.server.host, "trackmania");
            assert_eq!(config.server.password, "from-env");
            assert_eq!(config.controller.default_lang, Lang::Fr);
            assert_eq!(config.roles.admins, vec!["boss".to_owned()]);
            assert!(config.plugins.contains_key("logger"));
            Ok(())
        });
    }

    #[test]
    fn password_is_required() {
        figment::Jail::expect_with(|_jail| {
            let err = load(Path::new("missing.toml")).unwrap_err();
            assert!(matches!(err, ConfigError::MissingPassword));
            Ok(())
        });
    }
}
