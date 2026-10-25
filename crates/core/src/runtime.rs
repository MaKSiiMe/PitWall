//! The controller runtime: connection management, state tracking, command execution.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pitwall_gbx::callbacks::{Boundary, Lifecycle, ScriptCallback, Section};
use pitwall_gbx::types::{MapInfo, PlayerInfo, ScriptSettings};
use pitwall_gbx::{Callback, ClientOptions, GbxClient, GbxError, Value};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::builtin::CorePlugin;
use crate::chat::{CommandRegistry, ReplyTo, parse_command_line};
use crate::command::{
    Command, CommandError, CommandHandle, CommandRequest, ModeSwitch, Recipients,
};
use crate::event::{Actor, AuditEntry, Event};
use crate::i18n::{Lang, Text, Translator};
use crate::matchsettings;
use crate::playlist;
use crate::plugin::{EventBus, Plugin, PluginContext};
use crate::roles::{Role, RoleBook};
use crate::state::{Player, StateHandle, now_ms};

/// Connection settings of the dedicated server.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    /// XML-RPC port (5000 by default).
    pub port: u16,
    /// XML-RPC login, usually `SuperAdmin`.
    pub user: String,
    /// Password of that login. Never hard-code it: set `PITWALL_SERVER__PASSWORD`.
    pub password: String,
    /// `SetApiVersion` value.
    pub api_version: String,
    /// Optional `XmlRpc.SetApiVersion` value for script callbacks (semver).
    pub script_api_version: Option<String>,
    pub call_timeout_secs: u64,
    /// First reconnection delay; doubles up to `max_reconnect_delay_secs`.
    pub reconnect_delay_secs: u64,
    pub max_reconnect_delay_secs: u64,
    /// Directory (relative to Maps) where generated MatchSettings files go.
    pub matchsettings_dir: String,
    /// MatchSettings file the server starts with (relative to Maps; `/game_settings` of the
    /// server, `TM_GAME_SETTINGS` of the Docker image). PitWall saves the playlist and the mode
    /// there after each change, so a server restart keeps them. Empty: never saved.
    pub playlist_file: String,
    /// Local path of the server's Maps directory (shared volume). When set, uploaded maps are
    /// written there directly; otherwise they go through `WriteFile`, whose request size is
    /// limited (about 3 MB of map). Empty: `WriteFile`.
    pub maps_path: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 5000,
            user: "SuperAdmin".to_owned(),
            password: String::new(),
            api_version: "2023-03-25".to_owned(),
            script_api_version: None,
            call_timeout_secs: 10,
            reconnect_delay_secs: 2,
            max_reconnect_delay_secs: 30,
            matchsettings_dir: "MatchSettings/PitWall".to_owned(),
            playlist_file: "MatchSettings/default.txt".to_owned(),
            maps_path: String::new(),
        }
    }
}

/// Runtime options.
#[derive(Debug, Clone, Default)]
pub struct ControllerOptions {
    pub server: ServerConfig,
    /// Language for console output and players whose language is unknown.
    pub default_lang: Lang,
    /// Announce joins/leaves in the chat.
    pub announce_joins: bool,
    /// `[plugins.<name>]` sections.
    pub plugin_configs: toml::Table,
    /// Records the GbxRemote traffic of every connection, when set.
    pub recorder: Option<pitwall_gbx::record::Recorder>,
}

/// Errors that stop the runtime.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("plugin {plugin}: {source}")]
    Registry {
        plugin: String,
        #[source]
        source: crate::chat::RegistryError,
    },
    #[error("plugin {plugin}: {source}")]
    Catalog {
        plugin: String,
        #[source]
        source: crate::i18n::CatalogError,
    },
}

/// Handles to interact with a running controller from outside plugins (console, web, Discord).
#[derive(Debug, Clone)]
pub struct Handles {
    pub bus: EventBus,
    pub state: StateHandle,
    pub roles: RoleBook,
    pub i18n: Translator,
    pub registry: CommandRegistry,
    pub default_lang: Lang,
    command_tx: mpsc::Sender<CommandRequest>,
}

impl Handles {
    /// A command handle acting as `actor`.
    pub fn commands(&self, actor: Actor) -> CommandHandle {
        CommandHandle::new(self.command_tx.clone(), actor)
    }

    /// Parses and dispatches a command line (`/mode cup`; the leading slash is optional here).
    pub fn invoke(&self, actor: Actor, role: Role, lang: Lang, line: &str, reply: ReplyTo) {
        crate::chat::dispatch(
            &crate::chat::Dispatcher {
                registry: &self.registry,
                bus: &self.bus,
                i18n: &self.i18n,
                commands: &self.commands(Actor::System),
            },
            actor,
            role,
            lang,
            line,
            reply,
        );
    }
}

/// A mode switch waiting for the next map load to be verified.
#[derive(Debug, Clone)]
struct PendingSwitch {
    switch: ModeSwitch,
    actor: Actor,
}

/// The controller: owns plugins and runs everything.
pub struct Controller {
    options: ControllerOptions,
    plugins: Vec<Box<dyn Plugin>>,
    handles: Handles,
    command_rx: mpsc::Receiver<CommandRequest>,
}

impl std::fmt::Debug for Controller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controller")
            .field(
                "plugins",
                &self.plugins.iter().map(|p| p.name()).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Controller {
    pub fn new(options: ControllerOptions, roles: RoleBook, i18n: Translator) -> Controller {
        let (command_tx, command_rx) = mpsc::channel(1024);
        let handles = Handles {
            bus: EventBus::new(8192),
            state: StateHandle::default(),
            roles,
            i18n,
            registry: CommandRegistry::default(),
            default_lang: options.default_lang,
            command_tx,
        };
        let mut controller = Controller {
            options,
            plugins: Vec::new(),
            handles,
            command_rx,
        };
        controller.add_plugin(CorePlugin);
        controller
    }

    pub fn add_plugin(&mut self, plugin: impl Plugin) {
        self.plugins.push(Box::new(plugin));
    }

    pub fn handles(&self) -> Handles {
        self.handles.clone()
    }

    /// Registers every plugin's commands and translations in a scratch registry, as
    /// [`Controller::run`] does first: a duplicate command or a broken catalog fails here
    /// instead of at startup on the server. Returns that registry.
    pub fn check(&self) -> Result<CommandRegistry, CoreError> {
        let registry = CommandRegistry::default();
        register(&self.plugins, &registry, &Translator::default())?;
        Ok(registry)
    }

    /// Runs until `shutdown` is cancelled.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), CoreError> {
        let Controller {
            options,
            plugins,
            handles,
            command_rx,
        } = self;

        // Register commands and translations first so plugins see a complete registry.
        register(&plugins, &handles.registry, &handles.i18n)?;

        let mut tasks = tokio::task::JoinSet::new();
        for plugin in plugins {
            let name = plugin.name();
            let config = options
                .plugin_configs
                .get(name)
                .and_then(|v| v.as_table())
                .cloned()
                .unwrap_or_default();
            let ctx = PluginContext {
                name,
                events: handles.bus.subscribe(name),
                commands: handles.commands(Actor::Plugin(name.to_owned())),
                state: handles.state.clone(),
                roles: handles.roles.clone(),
                i18n: handles.i18n.clone(),
                registry: handles.registry.clone(),
                bus: handles.bus.clone(),
                shutdown: shutdown.child_token(),
                config,
                default_lang: options.default_lang,
            };
            tasks.spawn(async move {
                tracing::debug!(plugin = name, "plugin started");
                match plugin.run(ctx).await {
                    Ok(()) => tracing::debug!(plugin = name, "plugin stopped"),
                    Err(err) => tracing::error!(plugin = name, %err, "plugin failed"),
                }
            });
        }

        let (client_tx, client_rx) = watch::channel::<Option<GbxClient>>(None);
        let pending: Arc<Mutex<Option<PendingSwitch>>> = Arc::default();

        let executor = Executor {
            client: client_rx,
            handles: handles.clone(),
            pending: Arc::clone(&pending),
            matchsettings_dir: options.server.matchsettings_dir.clone(),
            playlist_file: options.server.playlist_file.clone(),
            maps_path: options.server.maps_path.clone(),
        };
        let executor_task = tokio::spawn(executor.run(command_rx, shutdown.clone()));

        let connection = Connection {
            options,
            handles: handles.clone(),
            client_tx,
            pending,
        };
        connection.run(shutdown.clone()).await;

        executor_task.abort();
        // Give plugins a moment to finish (they observe the same shutdown token).
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            while tasks.join_next().await.is_some() {}
        })
        .await;
        tasks.abort_all();
        Ok(())
    }
}

/// Registers the commands and translations of `plugins`.
fn register(
    plugins: &[Box<dyn Plugin>],
    registry: &CommandRegistry,
    i18n: &Translator,
) -> Result<(), CoreError> {
    for plugin in plugins {
        for spec in plugin.commands() {
            registry
                .register(spec)
                .map_err(|source| CoreError::Registry {
                    plugin: plugin.name().to_owned(),
                    source,
                })?;
        }
        for (lang, catalog) in plugin.translations() {
            i18n.add_catalog(lang, catalog)
                .map_err(|source| CoreError::Catalog {
                    plugin: plugin.name().to_owned(),
                    source,
                })?;
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------------------------
// Connection and callback tracking
// -----------------------------------------------------------------------------------------------

struct Connection {
    options: ControllerOptions,
    handles: Handles,
    client_tx: watch::Sender<Option<GbxClient>>,
    pending: Arc<Mutex<Option<PendingSwitch>>>,
}

impl Connection {
    async fn run(self, shutdown: CancellationToken) {
        let server = &self.options.server;
        let addr = format!("{}:{}", server.host, server.port);
        let mut delay = Duration::from_secs(server.reconnect_delay_secs.max(1));
        let max_delay = Duration::from_secs(server.max_reconnect_delay_secs.max(1));

        while !shutdown.is_cancelled() {
            match self.connect(&addr).await {
                Ok((client, callbacks, version)) => {
                    delay = Duration::from_secs(server.reconnect_delay_secs.max(1));
                    // The executor must have the client before plugins hear `Connected`,
                    // otherwise their first commands fail with "not connected".
                    self.client_tx.send_replace(Some(client.clone()));
                    self.handles.bus.publish(Event::Connected { version });
                    self.serve(&client, callbacks, &shutdown).await;
                    self.client_tx.send_replace(None);
                    self.handles.state.write(|s| s.connected = false);
                    if shutdown.is_cancelled() {
                        // Leave the server clean for the next controller run.
                        let _ = client.hide_all_manialinks().await;
                        break;
                    }
                    tracing::warn!("disconnected from the game server");
                    self.handles.bus.publish(Event::Disconnected);
                }
                Err(err) => {
                    tracing::warn!(%addr, %err, retry_in = ?delay, "cannot reach the game server");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = shutdown.cancelled() => break,
            }
            delay = (delay * 2).min(max_delay);
        }
    }

    async fn connect(
        &self,
        addr: &str,
    ) -> Result<
        (
            GbxClient,
            pitwall_gbx::Callbacks,
            pitwall_gbx::types::VersionInfo,
        ),
        GbxError,
    > {
        let server = &self.options.server;
        let options = ClientOptions {
            call_timeout: Duration::from_secs(server.call_timeout_secs.max(1)),
            recorder: self.options.recorder.clone(),
            ..ClientOptions::default()
        };
        let (client, callbacks) = GbxClient::connect(addr, options).await?;
        client.authenticate(&server.user, &server.password).await?;
        if let Err(err) = client.set_api_version(&server.api_version).await {
            tracing::warn!(%err, version = %server.api_version, "SetApiVersion refused");
        }
        client.enable_callbacks(true).await?;
        if let Err(err) = client.enable_script_callbacks().await {
            tracing::warn!(%err, "could not enable mode script callbacks (not in script mode?)");
        }
        if let Some(version) = &server.script_api_version
            && let Err(err) = client.set_script_api_version(version).await
        {
            tracing::warn!(%err, version, "XmlRpc.SetApiVersion refused");
        }
        let version = client.get_version().await?;
        tracing::info!(
            name = %version.name,
            build = %version.build,
            api = %version.api_version,
            "connected to the game server"
        );
        self.sync(&client).await?;
        self.handles.state.write(|s| {
            s.connected = true;
            s.version = Some(version.clone());
        });
        // Remove leftovers of a previous controller run.
        let _ = client.hide_all_manialinks().await;
        // Current scores (`Trackmania.Scores`, handled once `Connected` is out): rankings come
        // back right away after a controller restart instead of at the end of the round.
        if let Err(err) = client.request_scores("pitwall").await {
            tracing::debug!(%err, "cannot request the scores (not in script mode?)");
        }
        Ok((client, callbacks, version))
    }

    /// Reads the full server state after a (re)connection.
    async fn sync(&self, client: &GbxClient) -> Result<(), GbxError> {
        let system = client.get_system_info().await.unwrap_or_default();
        let list = client.get_player_list(1000, 0).await?;
        let mut players = std::collections::BTreeMap::new();
        for info in list {
            if info.is_server() || info.login == system.server_login {
                continue;
            }
            let player = self.load_player(client, &info.login, Some(&info)).await;
            players.insert(player.login.clone(), player);
        }
        let map = client.get_current_map_info().await.ok();
        let script = client
            .get_script_name()
            .await
            .map(|s| s.current_value)
            .unwrap_or_default();
        let settings = client.get_mode_script_settings().await.unwrap_or_default();
        self.handles.state.write(|s| {
            s.server_login = system.server_login;
            s.players = players;
            s.map = map;
            s.script = script;
            s.script_settings = settings;
        });
        Ok(())
    }

    async fn load_player(
        &self,
        client: &GbxClient,
        login: &str,
        info: Option<&PlayerInfo>,
    ) -> Player {
        let detailed = client.get_detailed_player_info(login).await.ok();
        let nickname = detailed
            .as_ref()
            .map(|d| d.nick_name.clone())
            .or_else(|| info.map(|i| i.nick_name.clone()))
            .unwrap_or_else(|| login.to_owned());
        Player {
            login: login.to_owned(),
            nickname,
            account_id: String::new(),
            zone: detailed
                .as_ref()
                .map(|d| d.path.clone())
                .unwrap_or_default(),
            lang: detailed
                .as_ref()
                .and_then(|d| Lang::from_code(&d.language))
                .unwrap_or(self.options.default_lang),
            team: info.map_or(-1, |i| i.team_id),
            spectator: detailed.as_ref().is_some_and(|d| d.is_spectator)
                || info.is_some_and(PlayerInfo::is_spectator),
            role: self.handles.roles.get(login),
            joined_at: now_ms(),
        }
    }

    async fn serve(
        &self,
        client: &GbxClient,
        mut callbacks: pitwall_gbx::Callbacks,
        shutdown: &CancellationToken,
    ) {
        loop {
            let callback = tokio::select! {
                cb = callbacks.recv() => cb,
                () = shutdown.cancelled() => return,
            };
            let Some(callback) = callback else { return };
            self.on_callback(client, callback).await;
        }
    }

    async fn on_callback(&self, client: &GbxClient, callback: Callback) {
        let bus = &self.handles.bus;
        let state = &self.handles.state;
        let shared = Arc::new(callback);
        bus.publish(Event::Callback {
            callback: Arc::clone(&shared),
        });
        let server_login = state.read(|s| s.server_login.clone());

        match &*shared {
            Callback::PlayerConnect { login, .. } => {
                if *login == server_login {
                    return;
                }
                let player = self.load_player(client, login, None).await;
                state.write(|s| s.players.insert(login.clone(), player.clone()));
                if self.options.announce_joins {
                    self.handles
                        .commands(Actor::System)
                        .chat_all(Text::tr("core.player_joined").arg("name", &player.nickname));
                }
                bus.publish(Event::PlayerJoined { player });
            }
            Callback::PlayerDisconnect { login, reason } => {
                if let Some(player) = state.write(|s| s.players.remove(login)) {
                    if self.options.announce_joins {
                        self.handles
                            .commands(Actor::System)
                            .chat_all(Text::tr("core.player_left").arg("name", &player.nickname));
                    }
                    bus.publish(Event::PlayerLeft {
                        player,
                        reason: reason.clone(),
                    });
                }
            }
            Callback::PlayerInfoChanged(info) => {
                let updated = state.write(|s| {
                    let player = s.players.get_mut(&info.login)?;
                    let spectator = info.is_spectator();
                    if player.team == info.team_id && player.spectator == spectator {
                        return None;
                    }
                    player.team = info.team_id;
                    player.spectator = spectator;
                    Some(player.clone())
                });
                if let Some(player) = updated {
                    bus.publish(Event::PlayerUpdated { player });
                }
            }
            Callback::PlayerChat {
                player_uid,
                login,
                text,
                ..
            } => {
                if *player_uid == 0 || *login == server_login {
                    return;
                }
                if parse_command_line(text).is_some() {
                    let role = self.handles.roles.get(login);
                    let lang = state
                        .read(|s| s.lang_of(login))
                        .unwrap_or(self.options.default_lang);
                    self.handles.invoke(
                        Actor::Player(login.clone()),
                        role,
                        lang,
                        text,
                        ReplyTo::Player(login.clone()),
                    );
                } else {
                    bus.publish(Event::Chat {
                        login: login.clone(),
                        text: text.clone(),
                    });
                }
            }
            Callback::PlayerManialinkPageAnswer {
                login,
                answer,
                entries,
                ..
            } => bus.publish(Event::ManialinkAnswer {
                login: login.clone(),
                action: answer.clone(),
                entries: entries
                    .iter()
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect(),
            }),
            Callback::BeginMap(map) => {
                let map = map.clone();
                state.write(|s| s.map = Some(map));
            }
            Callback::Script(script) => self.on_script_callback(client, script).await,
            _ => {}
        }
    }

    async fn on_script_callback(&self, client: &GbxClient, script: &ScriptCallback) {
        let bus = &self.handles.bus;
        let state = &self.handles.state;
        match script {
            ScriptCallback::WayPoint(wp) => {
                let waypoint = Arc::new(wp.clone());
                if wp.isendrace {
                    bus.publish(Event::Finish { waypoint });
                } else {
                    bus.publish(Event::Checkpoint { waypoint });
                }
            }
            ScriptCallback::GiveUp(e) => bus.publish(Event::GiveUp { event: e.clone() }),
            ScriptCallback::StartLine(e) => bus.publish(Event::StartLine { event: e.clone() }),
            ScriptCallback::Respawn(e) => bus.publish(Event::Respawn {
                event: Arc::new(e.clone()),
            }),
            ScriptCallback::Scores(scores) => {
                let scores = Arc::new((**scores).clone());
                state.write(|s| s.last_scores = Some(Arc::clone(&scores)));
                bus.publish(Event::Scores { scores });
            }
            ScriptCallback::WarmUpStart => {
                state.write(|s| s.warmup = true);
                bus.publish(Event::WarmUp { active: true });
            }
            ScriptCallback::WarmUpEnd => {
                state.write(|s| s.warmup = false);
                bus.publish(Event::WarmUp { active: false });
            }
            ScriptCallback::WarmUpStatus(status) => state.write(|s| s.warmup = status.active),
            ScriptCallback::PauseStatus(status) => {
                let changed = state
                    .write(|s| std::mem::replace(&mut s.paused, status.active) != status.active);
                if changed {
                    bus.publish(Event::Pause {
                        active: status.active,
                    });
                }
            }
            ScriptCallback::Lifecycle(lifecycle) => self.on_lifecycle(client, lifecycle).await,
            _ => {}
        }
    }

    async fn on_lifecycle(&self, client: &GbxClient, l: &Lifecycle) {
        let bus = &self.handles.bus;
        let state = &self.handles.state;
        match (l.section, l.boundary) {
            (Section::StartMatch, Boundary::Start) => {
                state.write(|s| s.match_running = true);
                bus.publish(Event::MatchStart);
            }
            (Section::EndMatch, Boundary::Start) => {
                state.write(|s| s.match_running = false);
                bus.publish(Event::MatchEnd);
            }
            (Section::StartMap, Boundary::Start) => {
                let map = match client.get_current_map_info().await {
                    Ok(map) => map,
                    Err(err) => {
                        tracing::warn!(%err, "GetCurrentMapInfo failed");
                        l.map.as_ref().map(script_map_to_info).unwrap_or_default()
                    }
                };
                state.write(|s| {
                    s.map = Some(map.clone());
                    s.round = 0;
                });
                self.verify_pending_switch(client).await;
                // Captures mode switches once they took effect.
                save_playlist(client, &self.options.server.playlist_file).await;
                bus.publish(Event::MapStart {
                    map: Arc::new(map),
                    restarted: l.restarted,
                });
                // Points of the new map (reset, or carried over in the match) without waiting for
                // the end of the first round.
                if let Err(err) = client.request_scores("pitwall").await {
                    tracing::debug!(%err, "cannot request the scores");
                }
            }
            (Section::EndMap, Boundary::Start) => {
                let map = state.read(|s| s.map.clone()).map(Arc::new);
                bus.publish(Event::MapEnd { map });
            }
            (Section::StartRound, Boundary::Start) => {
                state.write(|s| s.round = l.count);
                bus.publish(Event::RoundStart { count: l.count });
            }
            (Section::EndRound, Boundary::End) => bus.publish(Event::RoundEnd { count: l.count }),
            _ => {}
        }
    }

    /// After a mode switch, checks the new script is running and settings are applied.
    async fn verify_pending_switch(&self, client: &GbxClient) {
        let script = match client.get_script_name().await {
            Ok(names) => names.current_value,
            Err(err) => {
                tracing::warn!(%err, "GetScriptName failed");
                return;
            }
        };
        let previous = self
            .handles
            .state
            .write(|s| std::mem::replace(&mut s.script, script.clone()));

        let pending = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(PendingSwitch { switch, actor }) = pending else {
            if matchsettings::normalize_script_name(&previous)
                != matchsettings::normalize_script_name(&script)
            {
                self.handles.bus.publish(Event::ModeChanged { script });
            }
            return;
        };

        if matchsettings::normalize_script_name(&script)
            != matchsettings::normalize_script_name(&switch.script)
        {
            tracing::error!(expected = %switch.script, running = %script, "mode switch did not take effect");
            self.audit(&actor, "switch_mode_verify", &switch.script, false);
            return;
        }

        // Settings normally come with the MatchSettings; re-apply any that differ.
        let current = client.get_mode_script_settings().await.unwrap_or_default();
        let missing: ScriptSettings = switch
            .settings
            .iter()
            .filter(|(k, v)| current.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !missing.is_empty() {
            tracing::info!(
                count = missing.len(),
                "re-applying mode settings after the switch"
            );
            if let Err(err) = client.set_mode_script_settings(&missing).await {
                tracing::warn!(%err, "could not apply mode settings");
            }
        }
        if let Some(points) = &switch.points_repartition
            && let Err(err) = client.set_points_repartition(points).await
        {
            tracing::warn!(%err, "could not set points repartition");
        }
        let settings = client.get_mode_script_settings().await.unwrap_or_default();
        self.handles.state.write(|s| s.script_settings = settings);
        tracing::info!(script = %script, label = %switch.label, "mode switch complete");
        self.audit(&actor, "switch_mode_verify", &switch.label, true);
        self.handles.bus.publish(Event::ModeChanged { script });
    }

    fn audit(&self, actor: &Actor, action: &str, detail: &str, success: bool) {
        self.handles.bus.publish(Event::Audit {
            entry: AuditEntry {
                at: now_ms(),
                actor: actor.clone(),
                action: action.to_owned(),
                details: serde_json::json!({ "detail": detail }),
                success,
            },
        });
    }
}

fn script_map_to_info(map: &pitwall_gbx::callbacks::ScriptMap) -> MapInfo {
    MapInfo {
        uid: map.uid.clone(),
        name: map.name.clone(),
        file_name: map.filename.clone(),
        author: map.author.clone(),
        author_nickname: map.authornickname.clone(),
        environnement: map.environment.clone(),
        mood: map.mood.clone(),
        bronze_time: map.bronzetime,
        silver_time: map.silvertime,
        gold_time: map.goldtime,
        author_time: map.authortime,
        lap_race: map.laprace,
        nb_laps: map.nblaps,
        map_type: map.maptype.clone(),
        map_style: map.mapstyle.clone(),
        ..MapInfo::default()
    }
}

// -----------------------------------------------------------------------------------------------
// Command execution
// -----------------------------------------------------------------------------------------------

struct Executor {
    client: watch::Receiver<Option<GbxClient>>,
    handles: Handles,
    pending: Arc<Mutex<Option<PendingSwitch>>>,
    matchsettings_dir: String,
    playlist_file: String,
    maps_path: String,
}

impl Executor {
    async fn run(self, mut commands: mpsc::Receiver<CommandRequest>, shutdown: CancellationToken) {
        loop {
            let request = tokio::select! {
                r = commands.recv() => r,
                () = shutdown.cancelled() => None,
            };
            let Some(request) = request else { break };
            let CommandRequest {
                command,
                actor,
                reply,
            } = request;
            let audit_name = command.audit_name();
            let details = if audit_name.is_some() {
                serde_json::to_value(&command).unwrap_or_default()
            } else {
                serde_json::Value::Null
            };

            let result = self.execute(command, &actor).await;

            if let Err(err) = &result {
                if reply.is_none() {
                    tracing::warn!(actor = %actor, %err, "command failed");
                }
            }
            if let Some(action) = audit_name
                && !matches!(actor, Actor::System)
            {
                self.handles.bus.publish(Event::Audit {
                    entry: AuditEntry {
                        at: now_ms(),
                        actor: actor.clone(),
                        action: action.to_owned(),
                        details,
                        success: result.is_ok(),
                    },
                });
            }
            if let Some(reply) = reply {
                let _ = reply.send(result);
            }
        }
    }

    async fn execute(&self, command: Command, actor: &Actor) -> Result<Value, CommandError> {
        let client = self
            .client
            .borrow()
            .clone()
            .ok_or(CommandError::NotConnected)?;
        let changes_playlist = matches!(
            command,
            Command::UploadMap { .. }
                | Command::AddMap { .. }
                | Command::RemoveMap { .. }
                | Command::SetPlaylist { .. }
        );
        let result = self.execute_on(&client, command, actor).await;
        if changes_playlist && result.is_ok() {
            save_playlist(&client, &self.playlist_file).await;
        }
        result
    }

    async fn execute_on(
        &self,
        client: &GbxClient,
        command: Command,
        actor: &Actor,
    ) -> Result<Value, CommandError> {
        let ok =
            |r: Result<(), GbxError>| r.map(|()| Value::Bool(true)).map_err(CommandError::from);
        match command {
            Command::Chat { to, text } => self.chat(client, to, &text).await,
            Command::Kick { login, reason } => ok(client.kick(&login, &reason).await),
            Command::Ban {
                login,
                reason,
                blacklist,
            } => {
                if blacklist {
                    ok(client.ban_and_blacklist(&login, &reason, true).await)
                } else {
                    ok(client.ban(&login, &reason).await)
                }
            }
            Command::Unban { login } => ok(client.unban(&login).await),
            Command::Mute { login } => ok(client.ignore(&login).await),
            Command::Unmute { login } => ok(client.unignore(&login).await),
            Command::ForceSpectator { login, mode } => {
                ok(client.force_spectator(&login, mode.into()).await)
            }
            Command::ForceTeam { login, team } => ok(client.force_player_team(&login, team).await),
            Command::SwitchMode(switch) => self.switch_mode(client, switch, actor).await,
            Command::SetModeSettings { settings } => {
                let result = client.set_mode_script_settings(&settings).await;
                if result.is_ok() {
                    self.handles
                        .state
                        .write(|s| s.script_settings.extend(settings));
                }
                ok(result)
            }
            Command::SetPointsRepartition { points } => {
                ok(client.set_points_repartition(&points).await)
            }
            Command::RestartMap => ok(client.restart_map().await),
            Command::NextMap => ok(client.next_map().await),
            Command::JumpToMap { uid } => ok(client.jump_to_map(&uid).await),
            Command::ForceEndRound => ok(client.force_end_round().await),
            Command::Pause { active } => ok(client.set_pause(active).await),
            Command::StopWarmUp => ok(client.force_stop_warmup().await),
            Command::SetPlayerPoints {
                login,
                round,
                map,
                r#match,
            } => ok(client.set_player_points(&login, round, map, r#match).await),
            Command::SetTeamPoints {
                team,
                round,
                map,
                r#match,
            } => ok(client.set_team_points(team, round, map, r#match).await),
            Command::UploadMap {
                file,
                data,
                insert_next,
            } => {
                self.store_map(client, &file, data).await?;
                if insert_next {
                    ok(client.insert_map(&file).await)
                } else {
                    ok(client.add_map(&file).await)
                }
            }
            Command::AddMap { file } => ok(client.add_map(&file).await),
            Command::RemoveMap { file } => ok(client.remove_map(&file).await),
            Command::SetPlaylist { files } => self.set_playlist(client, files).await,
            Command::ChooseNextMap { file } => ok(client.choose_next_map(&file).await),
            Command::Manialink {
                to,
                xml,
                timeout_ms,
                hide_on_click,
            } => match to {
                Recipients::All => ok(client.send_manialink(&xml, timeout_ms, hide_on_click).await),
                Recipients::Logins(logins) if logins.is_empty() => Ok(Value::Bool(true)),
                Recipients::Logins(logins) => ok(client
                    .send_manialink_to(&logins.join(","), &xml, timeout_ms, hide_on_click)
                    .await),
            },
            Command::AddGuest { login } => ok(client.add_guest(&login).await),
            Command::RemoveGuest { login } => ok(client.remove_guest(&login).await),
            Command::CleanGuests => ok(client.clean_guest_list().await),
            Command::SetPassword { password } => ok(client.set_server_password(&password).await),
            Command::ConnectBot => Ok(Value::String(client.connect_fake_player().await?)),
            Command::DisconnectBots => ok(client.disconnect_fake_player("*").await),
            Command::Raw { method, params } => Ok(client.call(&method, params).await?),
        }
    }

    /// Sends a chat message, translated per player language.
    async fn chat(
        &self,
        client: &GbxClient,
        to: Recipients,
        text: &Text,
    ) -> Result<Value, CommandError> {
        let i18n = &self.handles.i18n;
        let default_lang = self.handles.default_lang;
        let groups: Vec<(Lang, Vec<String>)> = self.handles.state.read(|s| {
            let logins: Vec<String> = match &to {
                Recipients::All => s.players.keys().cloned().collect(),
                Recipients::Logins(logins) => logins.clone(),
            };
            Lang::ALL
                .iter()
                .map(|lang| {
                    let group = logins
                        .iter()
                        .filter(|l| s.lang_of(l).unwrap_or(default_lang) == *lang)
                        .cloned()
                        .collect();
                    (*lang, group)
                })
                .collect()
        });
        for (lang, logins) in groups {
            if logins.is_empty() {
                continue;
            }
            let message = format!("{}{}", i18n.t(lang, "core.prefix"), i18n.render(lang, text));
            client.chat_send_to(&message, &logins.join(",")).await?;
        }
        Ok(Value::Bool(true))
    }

    /// Puts a map file in the server's Maps directory: directly on the shared volume when
    /// `maps_path` is set, else with `WriteFile`.
    async fn store_map(
        &self,
        client: &GbxClient,
        file: &str,
        data: Vec<u8>,
    ) -> Result<(), CommandError> {
        let relative = std::path::Path::new(file);
        let safe = relative
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !safe {
            return Err(CommandError::Invalid(format!(
                "invalid map file name: {file}"
            )));
        }
        if self.maps_path.is_empty() {
            // Base64 grows the data by a third; keep room for the XML around it.
            let request = data.len().div_ceil(3) * 4 + file.len() + 512;
            if request > WRITE_FILE_LIMIT {
                return Err(CommandError::Invalid(format!(
                    "{file} is too large for WriteFile ({:.1} MB): share the server's Maps \
                     directory with PitWall and set server.maps_path",
                    data.len() as f64 / 1_000_000.0
                )));
            }
            return Ok(client.write_file(file, data).await?);
        }
        let path = std::path::Path::new(&self.maps_path).join(relative);
        let error = |source| CommandError::File {
            path: path.display().to_string(),
            source,
        };
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(error)?;
        }
        tokio::fs::write(&path, data).await.map_err(error)?;
        Ok(())
    }

    /// Applies a new playlist order (see [`playlist::plan`]); the map after the current one in
    /// the new order plays next.
    async fn set_playlist(
        &self,
        client: &GbxClient,
        files: Vec<String>,
    ) -> Result<Value, CommandError> {
        if files.is_empty() {
            return Err(CommandError::Invalid(
                "the playlist cannot be empty".to_owned(),
            ));
        }
        let mut seen = HashSet::new();
        if let Some(twice) = files.iter().find(|f| !seen.insert(f.as_str())) {
            return Err(CommandError::Invalid(format!("{twice} is listed twice")));
        }
        let current: Vec<String> = client
            .get_map_list(5000, 0)
            .await?
            .into_iter()
            .map(|m| m.file_name)
            .collect();
        let plan = playlist::plan(&current, &files);
        if plan.is_empty() {
            return Ok(Value::Bool(true));
        }
        tracing::info!(
            remove = plan.remove.len(),
            append = plan.append.len(),
            "rebuilding the playlist"
        );
        if let Some(first) = &plan.add_first {
            client.add_map(first).await?;
        }
        if !plan.remove.is_empty() {
            client.remove_map_list(&plan.remove).await?;
        }
        if !plan.append.is_empty() {
            client.add_map_list(&plan.append).await?;
        }
        let playing = self
            .handles
            .state
            .read(|s| s.map.as_ref().map(|m| m.file_name.clone()));
        let next = playing
            .and_then(|p| files.iter().position(|f| *f == p))
            .map_or(0, |i| (i + 1) % files.len());
        if let Err(err) = client
            .set_next_map_index(i32::try_from(next).unwrap_or(0))
            .await
        {
            tracing::warn!(%err, "SetNextMapIndex failed");
        }
        Ok(Value::Bool(true))
    }

    /// Hot mode switch: MatchSettings (script + settings + playlist) then a single map load.
    async fn switch_mode(
        &self,
        client: &GbxClient,
        switch: ModeSwitch,
        actor: &Actor,
    ) -> Result<Value, CommandError> {
        let (maps, start_index) = if switch.maps.is_empty() {
            let list = client.get_map_list(5000, 0).await?;
            if list.is_empty() {
                return Err(CommandError::Invalid(
                    "the server has no map in its playlist".to_owned(),
                ));
            }
            let index = client.get_current_map_index().await.unwrap_or(0).max(0);
            let index = usize::try_from(index).unwrap_or(0).min(list.len() - 1);
            (
                list.into_iter().map(|m| m.file_name).collect::<Vec<_>>(),
                index,
            )
        } else {
            (switch.maps.clone(), 0)
        };
        let xml = matchsettings::render(&switch.script, &switch.settings, &maps, start_index);
        let file = format!(
            "{}/{}.txt",
            self.matchsettings_dir.trim_end_matches('/'),
            matchsettings::file_safe(&switch.label)
        );
        tracing::info!(script = %switch.script, %file, maps = maps.len(), "switching mode");
        client.write_file(&file, xml.into_bytes()).await?;
        client.load_match_settings(&file).await?;

        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(PendingSwitch {
            switch: switch.clone(),
            actor: actor.clone(),
        });

        let restart = if switch.maps.is_empty() {
            client.restart_map().await
        } else {
            match client.choose_next_map(&maps[0]).await {
                Ok(()) => client.next_map().await,
                Err(err) => Err(err),
            }
        };
        if let Err(err) = restart {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            return Err(err.into());
        }
        Ok(Value::Bool(true))
    }
}

/// Largest request the client sends (see `GbxCodec`); bigger `WriteFile` calls would fail.
const WRITE_FILE_LIMIT: usize = 4 * 1024 * 1024;

/// Saves the playlist and the mode in the server's startup MatchSettings (see
/// [`ServerConfig::playlist_file`]).
async fn save_playlist(client: &GbxClient, file: &str) {
    if file.is_empty() {
        return;
    }
    match client.save_match_settings(file).await {
        Ok(count) => tracing::debug!(%file, maps = count, "playlist saved"),
        Err(err) => tracing::warn!(%file, %err, "cannot save the playlist"),
    }
}
