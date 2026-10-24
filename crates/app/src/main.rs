//! PitWall binary: loads the configuration, connects to the dedicated server and runs the plugins.

mod config;
mod console;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, RwLock};

use clap::Parser;
use pitwall_core::{Controller, ControllerOptions, RoleBook, Translator};
use pitwall_formats::FormatRegistry;
use pitwall_formats::cup::CupLibrary;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::config::{Config, LogFormat};

/// Trackmania 2020 dedicated server controller.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Configuration file.
    #[arg(short, long, env = "PITWALL_CONFIG", default_value = "pitwall.toml")]
    config: PathBuf,
    /// Check the configuration and the formats, then exit.
    #[arg(long)]
    check: bool,
}

/// Fatal startup errors.
#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(transparent)]
    Formats(#[from] pitwall_formats::FormatError),
    #[error(transparent)]
    Cups(#[from] pitwall_formats::cup::LibraryError),
    #[error(transparent)]
    Catalog(#[from] pitwall_core::i18n::CatalogError),
    #[error(transparent)]
    Core(#[from] pitwall_core::CoreError),
    #[error("cannot open the database: {0}")]
    Storage(#[from] pitwall_storage::StorageError),
    #[error(transparent)]
    Integration(pitwall_integrations::IntegrationError),
}

fn init_logs(config: &Config) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log.filter));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match config.log.format {
        LogFormat::Json => builder.json().init(),
        LogFormat::Pretty => builder.init(),
    }
}

async fn run(cli: Cli) -> Result<(), AppError> {
    let config = config::load(&cli.config)?;
    init_logs(&config);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), config = %cli.config.display(), "PitWall starting");

    let mut formats = FormatRegistry::load(Some(&config.controller.formats_dir))?;
    formats.offer_only(&config.controller.formats)?;
    tracing::info!(
        count = formats.all().count(),
        offered = ?formats.offered().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        "formats loaded"
    );
    let cups = if config.controller.cups {
        let cups = CupLibrary::load(&config.controller.cups_dir, &formats)?;
        tracing::info!(count = cups.all().count(), "cups loaded");
        cups
    } else {
        tracing::info!("cups disabled (controller.cups = false)");
        CupLibrary::default()
    };
    let translator = Translator::with_core_catalogs()?;
    if cli.check {
        println!(
            "Configuration OK ({} formats, {} cups).",
            formats.all().count(),
            cups.all().count()
        );
        return Ok(());
    }

    let app = build(&config, formats, cups, translator).await?;
    let App {
        controller,
        cup_handle,
        formats,
        storage,
    } = app;

    let shutdown = CancellationToken::new();
    tokio::spawn(wait_for_signal(shutdown.clone()));
    if config.api.enabled {
        let api = pitwall_api::ApiState::new(
            controller.handles(),
            cup_handle,
            Arc::clone(&formats),
            storage.clone(),
            pitwall_api::ApiConfig {
                bind: config.api.bind.clone(),
                admin_token: config.api.admin_token.clone(),
                public_url: config.api.public_url.clone(),
                cups: config.controller.cups,
            },
        );
        let api_shutdown = shutdown.clone();
        tokio::spawn(async move {
            if let Err(err) = pitwall_api::serve(api, api_shutdown).await {
                tracing::error!(%err, "web dashboard stopped");
            }
        });
    }
    if config.controller.console {
        tokio::spawn(console::run(controller.handles(), shutdown.clone()));
    }

    controller.run(shutdown).await?;
    tracing::info!("PitWall stopped");
    Ok(())
}

/// The controller with every plugin, ready to run.
struct App {
    controller: Controller,
    cup_handle: pitwall_plugin_cup::CupHandle,
    formats: Arc<RwLock<FormatRegistry>>,
    storage: pitwall_storage::Storage,
}

/// Opens the database and adds the plugins the configuration enables.
async fn build(
    config: &Config,
    formats: FormatRegistry,
    cups: CupLibrary,
    translator: Translator,
) -> Result<App, AppError> {
    let formats = Arc::new(RwLock::new(formats));
    let options = ControllerOptions {
        server: config.server.clone(),
        default_lang: config.controller.default_lang,
        announce_joins: config.controller.announce_joins,
        plugin_configs: config.plugins.clone(),
    };
    let storage =
        pitwall_storage::Storage::open(&config.controller.data_dir.join("pitwall.db")).await?;
    let mut controller = Controller::new(options, RoleBook::from_config(&config.roles), translator);
    controller.add_plugin(pitwall_plugin_logger::LoggerPlugin);
    controller.add_plugin(pitwall_plugin_formats::FormatsPlugin::new(Arc::clone(
        &formats,
    )));
    controller.add_plugin(pitwall_plugin_admin::AdminPlugin);
    let (cup_plugin, cup_handle) = pitwall_plugin_cup::CupPlugin::new(
        Arc::clone(&formats),
        Arc::new(RwLock::new(cups)),
        config.controller.data_dir.clone(),
    );
    if config.controller.cups {
        controller.add_plugin(cup_plugin);
    }
    controller.add_plugin(pitwall_plugin_storage::StoragePlugin::new(storage.clone()));
    let http = pitwall_integrations::http_client().map_err(AppError::Integration)?;
    let nadeo = match pitwall_integrations::nadeo::Nadeo::new(
        http.clone(),
        &config.nadeo.login,
        &config.nadeo.password,
    ) {
        Ok(nadeo) => Some(Arc::new(nadeo)),
        Err(_) => {
            tracing::info!(
                "Nadeo API disabled (set PITWALL_NADEO__LOGIN / PITWALL_NADEO__PASSWORD)"
            );
            None
        }
    };
    if !config.roles.admin_clubs.is_empty() {
        match &nadeo {
            Some(nadeo) => controller.add_plugin(pitwall_plugin_clubs::ClubsPlugin::new(
                Arc::clone(nadeo),
                config.roles.admin_clubs.clone(),
            )),
            None => tracing::warn!(
                "roles.admin_clubs needs the Nadeo API (PITWALL_NADEO__LOGIN / PITWALL_NADEO__PASSWORD)"
            ),
        }
    }
    if let Some(nadeo) = &nadeo {
        let names = match pitwall_integrations::trackmania::TrackmaniaApi::new(
            http.clone(),
            &config.trackmania_api.client_id,
            &config.trackmania_api.client_secret,
        ) {
            Ok(api) => Some(Arc::new(api)),
            Err(_) => {
                tracing::info!(
                    "world record names from connected players only (set PITWALL_TRACKMANIA_API__CLIENT_ID / __CLIENT_SECRET)"
                );
                None
            }
        };
        controller.add_plugin(pitwall_plugin_worldrecords::WorldRecordsPlugin::new(
            Arc::clone(nadeo),
            names,
        ));
    }
    controller.add_plugin(pitwall_plugin_votes::VotesPlugin);
    controller.add_plugin(
        pitwall_plugin_maps::MapsPlugin::new(
            pitwall_integrations::tmx::Tmx::new(http.clone()),
            nadeo,
            Some(storage.clone()),
            config.controller.maps_dir.clone(),
        )
        .players_can_add(config.plugin_flag("maps", "players_can_add")),
    );
    if !config.discord.webhook_url.is_empty() {
        let webhook = pitwall_integrations::discord::Webhook::new(
            http.clone(),
            &config.discord.webhook_url,
            &config.discord.username,
        )
        .map_err(AppError::Integration)?;
        controller.add_plugin(pitwall_plugin_discord::DiscordPlugin::new(webhook));
    }
    controller.add_plugin(
        pitwall_plugin_ui::UiPlugin::new(Arc::clone(&formats), config.controller.data_dir.clone())
            .with_cups(config.controller.cups),
    );

    Ok(App {
        controller,
        cup_handle,
        formats,
        storage,
    })
}

/// Cancels `shutdown` on Ctrl+C or SIGTERM (`docker stop`).
async fn wait_for_signal(shutdown: CancellationToken) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                    () = shutdown.cancelled() => return,
                }
            }
            Err(err) => {
                tracing::warn!(%err, "cannot listen to SIGTERM");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("shutting down…");
    shutdown.cancel();
}

#[tokio::main]
async fn main() -> ExitCode {
    // Local secrets: `.env` next to the binary's working directory, if present.
    let dotenv = dotenvy::dotenv();
    let cli = Cli::parse();
    if let Err(err) = dotenv
        && !err.not_found()
    {
        eprintln!("pitwall: invalid .env file: {err}");
        return ExitCode::FAILURE;
    }
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("pitwall: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use pitwall_core::chat::{CommandRegistry, Resolution, parse_command_line};
    use pitwall_core::{Role, RolesConfig};

    use super::*;

    /// Builds the binary's plugins, with the optional ones switched on, and registers their
    /// commands as at startup.
    async fn registry(tweak: impl FnOnce(&mut Config)) -> CommandRegistry {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let data_dir =
            std::env::temp_dir().join(format!("pitwall-build-{}-{nanos}", std::process::id()));
        let mut config = Config::default();
        config.server.password = "test".to_owned();
        config.controller.data_dir = data_dir.clone();
        config.controller.cups = true;
        config.nadeo.login = "login".to_owned();
        config.nadeo.password = "password".to_owned();
        config.trackmania_api.client_id = "id".to_owned();
        config.trackmania_api.client_secret = "secret".to_owned();
        config.discord.webhook_url = "https://discord.invalid/webhook".to_owned();
        config.roles = RolesConfig {
            admin_clubs: vec![1],
            ..RolesConfig::default()
        };
        tweak(&mut config);

        let app = build(
            &config,
            FormatRegistry::load(None).unwrap(),
            CupLibrary::default(),
            Translator::with_core_catalogs().unwrap(),
        )
        .await
        .unwrap();
        let checked = app.controller.check();
        let _ = std::fs::remove_dir_all(&data_dir);
        checked.unwrap()
    }

    fn allowed(registry: &CommandRegistry, line: &str, role: Role) -> bool {
        let (name, args, _) = parse_command_line(line).unwrap();
        matches!(
            registry.resolve_invocation(&name, args.first().map(String::as_str), role),
            Resolution::Allowed(_)
        )
    }

    /// Role just below `role`.
    fn below(role: Role) -> Option<Role> {
        match role {
            Role::Player => None,
            Role::Caster => Some(Role::Player),
            Role::Referee => Some(Role::Caster),
            Role::Admin => Some(Role::Referee),
            Role::SuperAdmin => Some(Role::Admin),
        }
    }

    /// Every plugin registers its commands and translations without conflict (a duplicate
    /// stopped PitWall at startup).
    #[tokio::test]
    async fn every_plugin_registers_without_conflict() {
        registry(|_| {}).await;
    }

    /// The role each sensitive command needs: the role is enough, the one below is refused.
    #[tokio::test]
    async fn sensitive_commands_refuse_a_lower_role() {
        let registry = registry(|_| {}).await;
        let expected = [
            ("/players", Role::Caster),
            ("/kick bob", Role::Referee),
            ("/mute bob", Role::Referee),
            ("/unmute bob", Role::Referee),
            ("/spec bob", Role::Referee),
            ("/play bob", Role::Referee),
            ("/free bob", Role::Referee),
            ("/team bob blue", Role::Referee),
            ("/restart", Role::Referee),
            ("/addtime 5", Role::Referee),
            ("/endround", Role::Referee),
            ("/pause", Role::Referee),
            ("/resume", Role::Referee),
            ("/endwarmup", Role::Referee),
            ("/points bob map 3", Role::Referee),
            ("/teampoints blue map 3", Role::Referee),
            ("/say hello", Role::Referee),
            ("/vote pass", Role::Referee),
            ("/role bob", Role::Referee),
            ("/cup", Role::Referee),
            ("/cup status", Role::Referee),
            ("/cup pause", Role::Referee),
            ("/cup dq bob", Role::Referee),
            ("/cup go", Role::Referee),
            ("/ban bob", Role::Admin),
            ("/blacklist bob", Role::Admin),
            ("/unban bob", Role::Admin),
            ("/next", Role::Admin),
            ("/guest add bob", Role::Admin),
            ("/password off", Role::Admin),
            ("/bot", Role::Admin),
            ("/mode rounds", Role::Admin),
            ("/set S_TimeLimit 600", Role::Admin),
            ("/maps", Role::Admin),
            ("/map list", Role::Admin),
            ("/map add 12345", Role::Admin),
            ("/map tmx 12345 next", Role::Admin),
            ("/map uid abc", Role::Admin),
            ("/map search snow", Role::Admin),
            ("/map club 1 2", Role::Admin),
            ("/map clear", Role::Admin),
            ("/map remove", Role::Admin),
            ("/cup load mini-cup", Role::Admin),
            ("/cup start", Role::Admin),
            ("/cup abort", Role::Admin),
        ];
        for (line, role) in expected {
            assert!(allowed(&registry, line, role), "{line} for {role:?}");
            if let Some(lower) = below(role) {
                assert!(!allowed(&registry, line, lower), "{line} for {lower:?}");
            }
        }
        for line in [
            "/help", "/lang fr", "/ui all", "/formats", "/join", "/ready", "/skip",
        ] {
            assert!(allowed(&registry, line, Role::Player), "{line}");
        }
    }

    /// Whatever the command or sub-command, a role below the one it declares is refused.
    #[tokio::test]
    async fn every_command_refuses_a_lower_role() {
        let registry = registry(|_| {}).await;
        for spec in registry.available_to(Role::SuperAdmin) {
            let subs = spec.sub_roles.keys().map(|s| Some(s.as_str()));
            for sub in std::iter::once(None).chain(subs) {
                let needed = spec.role_for(sub);
                if let Some(lower) = below(needed) {
                    assert!(
                        matches!(
                            registry.resolve_invocation(&spec.name, sub, lower),
                            Resolution::Denied(_)
                        ),
                        "/{} {sub:?} for {lower:?}",
                        spec.name
                    );
                }
            }
        }
    }

    /// `[plugins.maps] players_can_add`: players add maps, the rest stays for admins.
    #[tokio::test]
    async fn players_can_add_maps_when_allowed() {
        let registry = registry(|config| {
            config.plugins = toml::from_str("[maps]\nplayers_can_add = true").unwrap();
        })
        .await;
        for line in [
            "/map add 12345",
            "/map tmx 12345 next",
            "/map uid abc",
            "/map search snow",
        ] {
            assert!(allowed(&registry, line, Role::Player), "{line}");
        }
        for line in [
            "/map clear",
            "/map remove 2",
            "/map list",
            "/map club 1 2",
            "/maps",
        ] {
            assert!(!allowed(&registry, line, Role::Player), "{line}");
        }
    }
}
