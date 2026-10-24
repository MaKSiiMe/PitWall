//! Runs cups on the server: executes the actions of the cup engine, feeds it with game events,
//! exposes `/cup`, `/join`, `/leave`, `/ready`, saves its state for crash recovery and offers a
//! [`CupHandle`] to the web dashboard and Discord.
//!
//! Custom events published (source `cup`): `state` (snapshot), `ready_check`, `ready_check_end`,
//! `phase_completed`, `finished`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use pitwall_core::gbx::types::ScriptSettings;
use pitwall_core::matchsettings::normalize_script_name;
use pitwall_core::{
    Actor, Command, CommandHandle, CommandSpec, Event, Invocation, Lang, Plugin, PluginContext,
    PluginError, PluginFuture, Recipients, Role, SpectatorModeDto, Text, now_ms,
};
use pitwall_formats::FormatRegistry;
use pitwall_formats::cup::library::library_rules;
use pitwall_formats::cup::{
    CupAction, CupEngine, CupError, CupInput, CupLibrary, CupSnapshot, CupState, CupStatus,
    LoadedCup, RegistrationMode,
};
use pitwall_formats::format::toml_to_setting;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

/// Shared format registry.
pub type SharedFormats = Arc<RwLock<FormatRegistry>>;
/// Shared cup library.
pub type SharedLibrary = Arc<RwLock<CupLibrary>>;

/// Requests from outside the plugin (web, Discord).
#[derive(Debug)]
enum CupRequest {
    Load {
        id: String,
        actor: Actor,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Input {
        input: CupInput,
        actor: Actor,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// Drives the active cup from outside the game. Cheap to clone.
#[derive(Debug, Clone)]
pub struct CupHandle {
    tx: mpsc::Sender<CupRequest>,
    snapshot: watch::Receiver<Option<CupSnapshot>>,
    library: SharedLibrary,
}

impl CupHandle {
    /// Snapshot of the active cup, if any.
    pub fn snapshot(&self) -> Option<CupSnapshot> {
        self.snapshot.borrow().clone()
    }

    /// Waits for the next snapshot change.
    pub async fn changed(&mut self) -> Option<CupSnapshot> {
        self.snapshot.changed().await.ok()?;
        self.snapshot()
    }

    /// Ids and names of the available cups.
    pub fn definitions(&self) -> Vec<(String, pitwall_formats::Localized, usize)> {
        self.library
            .read()
            .map(|l| {
                l.all()
                    .map(|c| (c.def.id.clone(), c.def.name.clone(), c.def.phases.len()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Loads a cup (becomes the active one).
    pub async fn load(&self, id: &str, actor: Actor) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(CupRequest::Load {
                id: id.to_owned(),
                actor,
                reply,
            })
            .await
            .map_err(|_| "cup plugin stopped".to_owned())?;
        rx.await.map_err(|_| "cup plugin stopped".to_owned())?
    }

    /// Sends an input to the active cup.
    pub async fn input(&self, input: CupInput, actor: Actor) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(CupRequest::Input {
                input,
                actor,
                reply,
            })
            .await
            .map_err(|_| "cup plugin stopped".to_owned())?;
        rx.await.map_err(|_| "cup plugin stopped".to_owned())?
    }
}

/// What is saved on disk.
#[derive(Debug, Serialize, Deserialize)]
struct SavedCup {
    cup_path: PathBuf,
    state: CupState,
}

/// The cup plugin.
pub struct CupPlugin {
    formats: SharedFormats,
    library: SharedLibrary,
    data_dir: PathBuf,
    requests: mpsc::Receiver<CupRequest>,
    snapshot_tx: watch::Sender<Option<CupSnapshot>>,
}

impl std::fmt::Debug for CupPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CupPlugin")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl CupPlugin {
    /// `data_dir`: where the active cup state is saved (`<data_dir>/cups/active.json`).
    pub fn new(
        formats: SharedFormats,
        library: SharedLibrary,
        data_dir: PathBuf,
    ) -> (CupPlugin, CupHandle) {
        let (tx, requests) = mpsc::channel(64);
        let (snapshot_tx, snapshot) = watch::channel(None);
        let handle = CupHandle {
            tx,
            snapshot,
            library: Arc::clone(&library),
        };
        (
            CupPlugin {
                formats,
                library,
                data_dir,
                requests,
                snapshot_tx,
            },
            handle,
        )
    }
}

impl Plugin for CupPlugin {
    fn name(&self) -> &'static str {
        "cup"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            // Referees run the match, admins manage the cups.
            ["", "status", "pause", "resume", "dq", "reinstate", "go"]
                .into_iter()
                .fold(CommandSpec::new("cup", "cup", Role::Admin), |spec, sub| {
                    spec.sub(sub, Role::Referee)
                })
                .usage("/cup <list|load|open|start|go|status|...>"),
            CommandSpec::new("cup", "join", Role::Player),
            CommandSpec::new("cup", "leave", Role::Player),
            CommandSpec::new("cup", "ready", Role::Player).alias("r"),
        ]
    }

    fn translations(&self) -> Vec<(Lang, &'static str)> {
        vec![
            (Lang::En, include_str!("../locales/en.toml")),
            (Lang::Fr, include_str!("../locales/fr.toml")),
        ]
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let CupPlugin {
                formats,
                library,
                data_dir,
                mut requests,
                snapshot_tx,
            } = *self;
            let mut runner = Runner {
                formats,
                library,
                state_file: data_dir.join("cups").join("active.json"),
                snapshot_tx,
                active: None,
                placement: HashMap::new(),
                last_format: None,
            };
            runner.restore(&ctx);
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    event = ctx.next_event() => match event {
                        Some(event) => runner.on_event(&ctx, event),
                        None => break,
                    },
                    Some(request) = requests.recv() => runner.on_request(&ctx, request),
                    _ = ticker.tick() => {
                        if runner.engine().is_some_and(|c| c.state().deadline_ms.is_some() || !c.state().disconnected.is_empty()) {
                            let _ = runner.input(&ctx, CupInput::Tick, &Actor::System);
                        }
                    }
                }
            }
            Ok::<(), PluginError>(())
        })
    }
}

/// The active cup and what the plugin remembers about the server.
struct Runner {
    formats: SharedFormats,
    library: SharedLibrary,
    state_file: PathBuf,
    snapshot_tx: watch::Sender<Option<CupSnapshot>>,
    active: Option<(CupEngine, PathBuf)>,
    /// Last spectator mode forced per login (avoids redundant calls).
    placement: HashMap<String, SpectatorModeDto>,
    /// Last `ApplyFormat` (for `/cup retry`).
    last_format: Option<CupAction>,
}

impl Runner {
    fn engine(&self) -> Option<&CupEngine> {
        self.active.as_ref().map(|(engine, _)| engine)
    }

    fn restore(&mut self, ctx: &PluginContext) {
        let Ok(raw) = std::fs::read_to_string(&self.state_file) else {
            self.publish(ctx);
            return;
        };
        match serde_json::from_str::<SavedCup>(&raw) {
            Ok(saved) => {
                let loaded = LoadedCup {
                    def: saved.state.def.clone(),
                    path: saved.cup_path.clone(),
                };
                let rules = match library_rules(&loaded) {
                    Ok(rules) => rules,
                    Err(err) => {
                        tracing::error!(%err, "cannot reload the rules of the saved cup");
                        None
                    }
                };
                tracing::info!(cup = %saved.state.def.id, status = saved.state.status.name(), "restored the active cup");
                self.active = Some((CupEngine::restore(saved.state, rules), saved.cup_path));
            }
            Err(err) => {
                tracing::error!(%err, file = %self.state_file.display(), "invalid saved cup, ignoring it")
            }
        }
        self.publish(ctx);
    }

    fn save(&self) {
        let Some((engine, path)) = &self.active else {
            let _ = std::fs::remove_file(&self.state_file);
            return;
        };
        let saved = SavedCup {
            cup_path: path.clone(),
            state: engine.state().clone(),
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.state_file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.state_file.with_extension("json.tmp");
            let json = serde_json::to_vec_pretty(&saved).map_err(std::io::Error::other)?;
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, &self.state_file)
        })();
        if let Err(err) = result {
            tracing::error!(%err, file = %self.state_file.display(), "cannot save the cup state");
        }
    }

    fn publish(&self, ctx: &PluginContext) {
        let snapshot = self.engine().map(CupEngine::snapshot);
        ctx.emit("state", serde_json::to_value(&snapshot).unwrap_or_default());
        self.snapshot_tx.send_replace(snapshot);
    }

    fn load(&mut self, ctx: &PluginContext, id: &str) -> Result<Text, Text> {
        if let Some(engine) = self.engine()
            && !matches!(
                engine.state().status,
                CupStatus::Draft | CupStatus::Finished | CupStatus::Aborted
            )
        {
            return Err(
                Text::tr("cup.error").arg("message", "abort the current cup first (/cup abort)")
            );
        }
        let loaded = self
            .library
            .read()
            .ok()
            .and_then(|l| l.get(id).cloned())
            .ok_or_else(|| Text::tr("cup.unknown").arg("id", id))?;
        let rules = library_rules(&loaded).map_err(|e| Text::tr("cup.error").arg("message", e))?;
        let name = Text::localized(
            loaded.def.display_name(Lang::En),
            loaded.def.display_name(Lang::Fr),
        );
        self.active = Some((CupEngine::new(loaded.def, rules), loaded.path));
        self.placement.clear();
        self.last_format = None;
        self.save();
        self.publish(ctx);
        Ok(Text::tr("cup.loaded").arg_text("cup", name))
    }

    /// Feeds an input to the engine and executes the resulting actions.
    fn input(
        &mut self,
        ctx: &PluginContext,
        input: CupInput,
        actor: &Actor,
    ) -> Result<(), CupError> {
        let Some((engine, _)) = &mut self.active else {
            return Err(CupError::InvalidState("not loaded"));
        };
        let quiet = matches!(input, CupInput::Tick | CupInput::Scores(_));
        let before = engine.state().status;
        let actions = engine.handle(input, now_ms())?;
        let changed = !actions.is_empty() || engine.state().status != before;
        for action in actions {
            self.execute(ctx, action, actor);
        }
        if changed || !quiet {
            self.save();
            self.publish(ctx);
        }
        Ok(())
    }

    fn execute(&mut self, ctx: &PluginContext, action: CupAction, actor: &Actor) {
        let commands = ctx.commands.as_actor(actor.clone());
        match action {
            CupAction::Announce(text) => ctx.chat(Recipients::All, text),
            CupAction::Notify { logins, text } => ctx.chat(Recipients::Logins(logins), text),
            CupAction::ApplyFormat { .. } => {
                self.last_format = Some(action.clone());
                self.apply_format(ctx, &action, commands);
            }
            CupAction::Place { racing } => self.place(ctx, &racing, &commands),
            CupAction::ReleasePlacement => {
                for login in ctx
                    .state
                    .read(|s| s.players.keys().cloned().collect::<Vec<_>>())
                {
                    commands.send(Command::ForceSpectator {
                        login,
                        mode: SpectatorModeDto::UserSelectable,
                    });
                }
                self.placement.clear();
            }
            CupAction::AssignTeams { blue, red } => {
                let connected = ctx
                    .state
                    .read(|s| s.players.keys().cloned().collect::<Vec<_>>());
                for (team, logins) in [(0, blue), (1, red)] {
                    for login in logins.into_iter().filter(|l| connected.contains(l)) {
                        commands.send(Command::ForceTeam { login, team });
                    }
                }
            }
            CupAction::ReadyCheck {
                logins,
                deadline_ms,
            } => {
                ctx.emit(
                    "ready_check",
                    serde_json::json!({ "logins": logins, "deadline_ms": deadline_ms }),
                );
            }
            CupAction::EndReadyCheck => ctx.emit("ready_check_end", serde_json::Value::Null),
            CupAction::PauseMatch(active) => commands.send(Command::Pause { active }),
            CupAction::PhaseCompleted(result) => {
                let cup = self
                    .engine()
                    .map(|e| e.def().id.clone())
                    .unwrap_or_default();
                ctx.emit(
                    "phase_completed",
                    serde_json::json!({ "cup": cup, "result": result }),
                );
            }
            CupAction::Finished { standings } => {
                let cup = self
                    .engine()
                    .map(|e| e.def().id.clone())
                    .unwrap_or_default();
                ctx.emit(
                    "finished",
                    serde_json::json!({ "cup": cup, "standings": standings }),
                );
            }
        }
    }

    fn apply_format(&self, ctx: &PluginContext, action: &CupAction, commands: CommandHandle) {
        let CupAction::ApplyFormat {
            format,
            settings,
            maps,
        } = action
        else {
            return;
        };
        let switch = self.formats.read().ok().and_then(|registry| {
            let def = registry.get(format)?;
            let overrides: ScriptSettings = settings
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), toml_to_setting(v)?)))
                .collect();
            let maps = (!maps.is_empty()).then_some(maps.as_slice());
            Some(def.to_mode_switch(&overrides, maps))
        });
        let label = match (
            self.engine(),
            self.engine().and_then(CupEngine::current_phase),
        ) {
            (Some(engine), Some(phase)) => format!("cup-{}-{}", engine.def().id, phase.id),
            _ => format!("cup-{format}"),
        };
        let replier_commands = ctx.commands.clone();
        match switch {
            Some(Ok(mut switch)) => {
                switch.label = label;
                tokio::spawn(async move {
                    if let Err(err) = commands.call(Command::SwitchMode(switch)).await {
                        tracing::error!(%err, "cup phase format failed");
                        replier_commands
                            .chat_all(Text::tr("cup.format_failed").arg("message", err));
                    }
                });
            }
            Some(Err(err)) => ctx
                .commands
                .chat_all(Text::tr("cup.format_failed").arg("message", err)),
            None => ctx.commands.chat_all(
                Text::tr("cup.format_failed").arg("message", format!("unknown format {format}")),
            ),
        }
    }

    fn place(&mut self, ctx: &PluginContext, racing: &[String], commands: &CommandHandle) {
        let lock = self.engine().is_some_and(|e| e.def().lock_server);
        let connected = ctx
            .state
            .read(|s| s.players.keys().cloned().collect::<Vec<_>>());
        for login in connected {
            let mode = if racing.contains(&login) {
                SpectatorModeDto::Player
            } else if lock {
                SpectatorModeDto::Spectator
            } else {
                continue;
            };
            if self.placement.get(&login) == Some(&mode) {
                continue;
            }
            self.placement.insert(login.clone(), mode);
            commands.send(Command::ForceSpectator { login, mode });
        }
    }

    fn on_event(&mut self, ctx: &PluginContext, event: Event) {
        let result = match event {
            Event::Command { invocation } => {
                self.on_command(ctx, &invocation);
                Ok(())
            }
            Event::ModeChanged { script } => {
                let expected = self
                    .engine()
                    .filter(|e| matches!(e.state().status, CupStatus::Loading { .. }))
                    .and_then(CupEngine::current_phase)
                    .and_then(|phase| self.formats.read().ok()?.get(&phase.format)?.script().ok());
                match expected {
                    Some(expected)
                        if normalize_script_name(&expected) == normalize_script_name(&script) =>
                    {
                        self.input(ctx, CupInput::ModeLoaded, &Actor::System)
                    }
                    _ => Ok(()),
                }
            }
            Event::Scores { scores } if self.engine().is_some() => self.input(
                ctx,
                CupInput::Scores(Box::new((*scores).clone())),
                &Actor::System,
            ),
            Event::PlayerLeft { player, .. } => {
                self.placement.remove(&player.login);
                if self.engine().is_some() {
                    self.input(
                        ctx,
                        CupInput::PlayerLeft {
                            login: player.login,
                        },
                        &Actor::System,
                    )
                } else {
                    Ok(())
                }
            }
            Event::PlayerJoined { player } if self.engine().is_some() => self.input(
                ctx,
                CupInput::PlayerJoined {
                    login: player.login,
                },
                &Actor::System,
            ),
            Event::ManialinkAnswer { login, action, .. } if action == "pw:cup:ready" => self.input(
                ctx,
                CupInput::Ready {
                    login: login.clone(),
                },
                &Actor::Player(login),
            ),
            Event::Connected { .. } => {
                // The server may have restarted: placements must be applied again.
                self.placement.clear();
                if let Some(engine) = self.engine()
                    && engine.state().status.phase().is_some()
                {
                    let racing = engine.racing_logins();
                    let commands = ctx.commands.clone();
                    self.place(ctx, &racing, &commands);
                }
                self.publish(ctx);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(err) = result {
            tracing::debug!(%err, "cup input ignored");
        }
    }

    fn on_request(&mut self, ctx: &PluginContext, request: CupRequest) {
        match request {
            CupRequest::Load { id, actor, reply } => {
                let result = self
                    .load(ctx, &id)
                    .map(|text| ctx.chat(Recipients::All, text));
                let _ = reply.send(result.map_err(|t| ctx.i18n.render(ctx.default_lang, &t)));
                tracing::info!(%actor, cup = %id, "cup loaded");
            }
            CupRequest::Input {
                input,
                actor,
                reply,
            } => {
                let result = self.input(ctx, input, &actor).map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
        }
    }

    fn on_command(&mut self, ctx: &PluginContext, inv: &Invocation) {
        let replier = ctx.replier(inv);
        let say_err = |err: CupError| replier.say(Text::tr("cup.error").arg("message", err));
        match inv.name.as_str() {
            "join" => {
                let Some(login) = inv.login() else { return };
                let name = ctx
                    .state
                    .player(login)
                    .map_or_else(|| login.to_owned(), |p| p.nickname);
                let input = CupInput::Register {
                    id: login.to_owned(),
                    name,
                    players: vec![],
                    via: RegistrationMode::InGame,
                };
                if let Err(err) = self.input(ctx, input, &inv.actor) {
                    say_err(err);
                }
            }
            "leave" => {
                let Some(login) = inv.login() else { return };
                if let Err(err) = self.input(
                    ctx,
                    CupInput::Unregister {
                        id: login.to_owned(),
                    },
                    &inv.actor,
                ) {
                    say_err(err);
                }
            }
            "ready" => {
                let Some(login) = inv.login() else { return };
                if let Err(err) = self.input(
                    ctx,
                    CupInput::Ready {
                        login: login.to_owned(),
                    },
                    &inv.actor,
                ) {
                    say_err(err);
                }
            }
            "cup" => self.on_cup_command(ctx, inv),
            _ => {}
        }
    }

    fn on_cup_command(&mut self, ctx: &PluginContext, inv: &Invocation) {
        let replier = ctx.replier(inv);
        let sub = inv.arg(0).unwrap_or("status");
        let admin_only = !matches!(
            sub,
            "status" | "pause" | "resume" | "dq" | "reinstate" | "go"
        );
        if admin_only && !inv.role.can(pitwall_core::Permission::ManageCup) {
            replier.say(Text::tr("core.permission_denied").arg("name", format!("cup {sub}")));
            return;
        }
        let run = |runner: &mut Runner, input: CupInput| {
            if let Err(err) = runner.input(ctx, input, &inv.actor) {
                replier.say(Text::tr("cup.error").arg("message", err));
            }
        };
        match sub {
            "list" => {
                replier.say(Text::tr("cup.list_header"));
                if let Ok(library) = self.library.read() {
                    for cup in library.all() {
                        replier.say(
                            Text::tr("cup.list_entry")
                                .arg("id", &cup.def.id)
                                .arg("name", cup.def.display_name(inv.lang))
                                .arg("phases", cup.def.phases.len()),
                        );
                    }
                }
            }
            "load" => {
                let Some(id) = inv.arg(1) else {
                    replier.say(Text::tr("core.usage").arg("usage", "/cup load <id>"));
                    return;
                };
                match self.load(ctx, id) {
                    Ok(text) => ctx.chat(Recipients::All, text),
                    Err(text) => replier.say(text),
                }
            }
            "unload" => {
                if let Some(engine) = self.engine()
                    && !matches!(
                        engine.state().status,
                        CupStatus::Draft | CupStatus::Finished | CupStatus::Aborted
                    )
                {
                    replier.say(Text::tr("cup.error").arg("message", "abort the cup first"));
                    return;
                }
                self.active = None;
                self.save();
                self.publish(ctx);
                replier.say(Text::tr("core.done"));
            }
            "status" => self.status(ctx, inv),
            "open" => run(self, CupInput::OpenRegistration),
            "start" => {
                let connected = ctx.state.read(|s| {
                    s.players
                        .values()
                        .map(|p| (p.login.clone(), p.nickname.clone()))
                        .collect()
                });
                run(self, CupInput::Start { connected });
            }
            "go" => run(self, CupInput::Go),
            "pause" => run(self, CupInput::Pause),
            "resume" => run(self, CupInput::Resume),
            "advance" | "next" => run(self, CupInput::Advance),
            "abort" => run(self, CupInput::Abort),
            "winner" => match inv.arg(1) {
                Some(id) => run(self, CupInput::SetWinner { id: id.to_owned() }),
                None => replier.say(Text::tr("core.usage").arg("usage", "/cup winner <team id>")),
            },
            "retry" => match self.last_format.clone() {
                Some(action) => {
                    let commands = ctx.commands_for(inv);
                    self.apply_format(ctx, &action, commands);
                }
                None => replier.say(Text::tr("cup.error").arg("message", "nothing to retry")),
            },
            "dq" | "reinstate" => {
                let Some(query) = inv.arg(1) else {
                    replier.say(
                        Text::tr("core.usage").arg("usage", format!("/cup {sub} <player|team>")),
                    );
                    return;
                };
                let id = self.resolve_participant(ctx, query);
                let input = if sub == "dq" {
                    CupInput::Disqualify { id }
                } else {
                    CupInput::Reinstate { id }
                };
                run(self, input);
            }
            "add" => {
                let Some(id) = inv.arg(1) else {
                    replier.say(Text::tr("core.usage").arg(
                        "usage",
                        "/cup add <login> | /cup add <team> <name> <login,login>",
                    ));
                    return;
                };
                let (name, players) = match (inv.arg(2), inv.arg(3)) {
                    (Some(name), Some(players)) => (
                        name.to_owned(),
                        players
                            .split(',')
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty())
                            .collect(),
                    ),
                    _ => (
                        ctx.state
                            .player(id)
                            .map_or_else(|| id.to_owned(), |p| p.nickname),
                        Vec::new(),
                    ),
                };
                run(
                    self,
                    CupInput::Register {
                        id: id.to_owned(),
                        name,
                        players,
                        via: RegistrationMode::List,
                    },
                );
            }
            "remove" => match inv.arg(1) {
                Some(id) => run(self, CupInput::Unregister { id: id.to_owned() }),
                None => replier.say(Text::tr("core.usage").arg("usage", "/cup remove <id>")),
            },
            "approve" => match inv.arg(1) {
                Some(id) => run(self, CupInput::Approve { id: id.to_owned() }),
                None => replier.say(Text::tr("core.usage").arg("usage", "/cup approve <id>")),
            },
            other => {
                replier.say(Text::tr("core.unknown_command").arg("name", format!("cup {other}")))
            }
        }
    }

    /// Participant id from an id, a login or a partial nickname.
    fn resolve_participant(&self, ctx: &PluginContext, query: &str) -> String {
        let Some(engine) = self.engine() else {
            return query.to_owned();
        };
        if engine.state().participants.iter().any(|p| p.id == query) {
            return query.to_owned();
        }
        let login = ctx
            .state
            .read(|s| s.find_player(query).map(|p| p.login.clone()))
            .unwrap_or_else(|_| query.to_owned());
        engine
            .participant_of_login(&login)
            .map_or(login, |p| p.id.clone())
    }

    fn status(&self, ctx: &PluginContext, inv: &Invocation) {
        let replier = ctx.replier(inv);
        let Some(engine) = self.engine() else {
            replier.say(Text::tr("cup.no_cup"));
            return;
        };
        let phase = engine
            .current_phase()
            .map(|p| format!("· {}", p.display_name(inv.lang)))
            .unwrap_or_default();
        replier.say(
            Text::tr("cup.status")
                .arg("cup", engine.def().display_name(inv.lang))
                .arg("status", engine.state().status.name())
                .arg("phase", phase),
        );
        if inv.arg(1) == Some("full") {
            for p in &engine.state().participants {
                replier.say(
                    Text::tr("cup.participant")
                        .arg("name", &p.name)
                        .arg("id", &p.id)
                        .arg("status", format!("{:?}", p.status).to_lowercase()),
                );
            }
        }
    }
}

/// Default location of cup definitions.
pub fn default_cups_dir() -> &'static Path {
    Path::new("config/cups")
}
