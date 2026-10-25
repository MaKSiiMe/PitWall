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
    pub nadeo: NadeoSection,
    pub trackmania_api: TrackmaniaApiSection,
    pub discord: DiscordSection,
    pub api: ApiSection,
    pub recording: RecordingSection,
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
    /// Local `.Map.Gbx` files that `/map local` can upload.
    pub maps_dir: PathBuf,
    /// Read commands from stdin.
    pub console: bool,
    /// Formats offered in `/mode`, `/formats`, the admin panel and the dashboard (ids). Empty:
    /// all of them.
    pub formats: Vec<String>,
    /// Cups: `/cup`, `/join`, `/ready`, the cup banner and the dashboard's cup pages.
    pub cups: bool,
}

impl Default for ControllerSection {
    fn default() -> Self {
        Self {
            default_lang: Lang::En,
            announce_joins: true,
            formats_dir: PathBuf::from("config/formats"),
            cups_dir: PathBuf::from("config/cups"),
            data_dir: PathBuf::from("data"),
            maps_dir: PathBuf::from("config/maps"),
            console: true,
            // Time attack and rounds first; the competition formats come back once they are
            // reliable on the server.
            formats: vec!["timeattack".to_owned(), "rounds".to_owned()],
            cups: false,
        }
    }
}

/// Nadeo web services, with the dedicated server account (`PITWALL_NADEO__LOGIN` / `__PASSWORD`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NadeoSection {
    pub login: String,
    pub password: String,
}

/// Trackmania API OAuth application, for the names of the world records
/// (`PITWALL_TRACKMANIA_API__CLIENT_ID` / `__CLIENT_SECRET`, from api.trackmania.com/manager).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TrackmaniaApiSection {
    pub client_id: String,
    pub client_secret: String,
}

/// Discord webhook (`PITWALL_DISCORD__WEBHOOK_URL`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DiscordSection {
    pub webhook_url: String,
    pub username: String,
}

impl Default for DiscordSection {
    fn default() -> Self {
        Self {
            webhook_url: String::new(),
            username: "PitWall".to_owned(),
        }
    }
}

/// Web dashboard and API.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ApiSection {
    pub enabled: bool,
    /// Listen address; keep it on localhost unless a reverse proxy adds TLS.
    pub bind: String,
    /// Admin token (`PITWALL_API__ADMIN_TOKEN`); empty disables admin pages.
    pub admin_token: String,
    /// Public base URL shown in links (registration page).
    pub public_url: String,
}

impl Default for ApiSection {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: "127.0.0.1:8080".to_owned(),
            admin_token: String::new(),
            public_url: String::new(),
        }
    }
}

/// Recording of the raw GbxRemote traffic (docs/ENREGISTREMENT.fr.md). Off by default.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RecordingSection {
    pub enabled: bool,
    /// Directory of the recordings; empty: `<data_dir>/recordings`.
    pub dir: PathBuf,
    /// A new file starts past this size.
    pub max_file_mb: u64,
    /// Files kept (the newest).
    pub max_files: usize,
}

impl Default for RecordingSection {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: PathBuf::new(),
            max_file_mb: 20,
            max_files: 10,
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

impl Config {
    /// Boolean `key` of `[plugins.<plugin>]`, `false` when missing.
    pub fn plugin_flag(&self, plugin: &str, key: &str) -> bool {
        self.plugins
            .get(plugin)
            .and_then(|section| section.get(key))
            .and_then(toml::Value::as_bool)
            .unwrap_or(false)
    }
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
    fn lists_from_environment() {
        // The Portainer stack has no pitwall.toml: roles come from the environment.
        figment::Jail::expect_with(|jail| {
            jail.set_env("PITWALL_SERVER__PASSWORD", "pw");
            jail.set_env(
                "PITWALL_ROLES__SUPERADMINS",
                r#"["X6N6WxRIR_Sj6Xr10yuDJA"]"#,
            );
            jail.set_env("PITWALL_ROLES__ADMINS", "[a_login, b-login]");
            jail.set_env("PITWALL_ROLES__ADMIN_CLUBS", "[12345]");
            jail.set_env("PITWALL_PLUGINS__MAPS__PLAYERS_CAN_ADD", "true");
            let config = load(Path::new("missing.toml")).map_err(|e| e.to_string())?;
            assert_eq!(
                config.roles.superadmins,
                vec!["X6N6WxRIR_Sj6Xr10yuDJA".to_owned()]
            );
            assert_eq!(
                config.roles.admins,
                vec!["a_login".to_owned(), "b-login".to_owned()]
            );
            assert_eq!(config.roles.admin_clubs, [12345]);
            assert!(config.plugin_flag("maps", "players_can_add"));
            assert!(!config.plugin_flag("maps", "missing"));
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

    #[test]
    fn competition_formats_are_opt_in() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("PITWALL_SERVER__PASSWORD", "pw");
            let config = load(Path::new("missing.toml")).map_err(|e| e.to_string())?;
            assert_eq!(config.controller.formats, ["timeattack", "rounds"]);
            assert!(!config.controller.cups);
            assert!(!config.recording.enabled);
            jail.set_env("PITWALL_CONTROLLER__FORMATS", "[]");
            jail.set_env("PITWALL_CONTROLLER__CUPS", "true");
            let config = load(Path::new("missing.toml")).map_err(|e| e.to_string())?;
            assert!(config.controller.formats.is_empty());
            assert!(config.controller.cups);
            Ok(())
        });
    }
}
