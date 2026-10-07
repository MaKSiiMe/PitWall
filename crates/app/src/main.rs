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

    let formats = FormatRegistry::load(Some(&config.controller.formats_dir))?;
    tracing::info!(count = formats.all().count(), "formats loaded");
    let cups = CupLibrary::load(&config.controller.cups_dir, &formats)?;
    tracing::info!(count = cups.all().count(), "cups loaded");
    let translator = Translator::with_core_catalogs()?;
    if cli.check {
        println!(
            "Configuration OK ({} formats, {} cups).",
            formats.all().count(),
            cups.all().count()
        );
        return Ok(());
    }

    let options = ControllerOptions {
        server: config.server.clone(),
        default_lang: config.controller.default_lang,
        announce_joins: config.controller.announce_joins,
        plugin_configs: config.plugins.clone(),
    };
    let storage =
        pitwall_storage::Storage::open(&config.controller.data_dir.join("pitwall.db")).await?;
    let mut controller = Controller::new(options, RoleBook::from_config(&config.roles), translator);
    let formats = Arc::new(RwLock::new(formats));
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
    controller.add_plugin(cup_plugin);
    controller.add_plugin(pitwall_plugin_storage::StoragePlugin::new(storage.clone()));
    let http = pitwall_integrations::http_client().map_err(AppError::Integration)?;
    let nadeo = match pitwall_integrations::nadeo::Nadeo::new(
        http.clone(),
        &config.nadeo.login,
        &config.nadeo.password,
    ) {
        Ok(nadeo) => Some(nadeo),
        Err(_) => {
            tracing::info!(
                "Nadeo API disabled (set PITWALL_NADEO__LOGIN / PITWALL_NADEO__PASSWORD)"
            );
            None
        }
    };
    controller.add_plugin(pitwall_plugin_maps::MapsPlugin::new(
        pitwall_integrations::tmx::Tmx::new(http.clone()),
        nadeo,
        Some(storage.clone()),
        config.controller.maps_dir.clone(),
    ));
    if !config.discord.webhook_url.is_empty() {
        let webhook = pitwall_integrations::discord::Webhook::new(
            http.clone(),
            &config.discord.webhook_url,
            &config.discord.username,
        )
        .map_err(AppError::Integration)?;
        controller.add_plugin(pitwall_plugin_discord::DiscordPlugin::new(webhook));
    }
    controller.add_plugin(pitwall_plugin_ui::UiPlugin::new(
        Arc::clone(&formats),
        config.controller.data_dir.clone(),
    ));

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
