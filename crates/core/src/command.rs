//! Commands sent by plugins to the runtime. Plugins never touch the socket.

use pitwall_gbx::types::ScriptSettings;
use pitwall_gbx::{GbxError, SpectatorMode, Value};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use crate::event::Actor;
use crate::i18n::Text;

/// Who receives a chat message or a manialink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipients {
    All,
    Logins(Vec<String>),
}

impl Recipients {
    pub fn login(login: impl Into<String>) -> Recipients {
        Recipients::Logins(vec![login.into()])
    }
}

/// A complete mode change, applied in one map load.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModeSwitch {
    /// Mode script, e.g. `Trackmania/TM_Cup_Online.Script.txt`.
    pub script: String,
    /// `S_*` settings of the new mode.
    pub settings: ScriptSettings,
    /// Map files (relative to the Maps directory) to play. Empty keeps the current playlist.
    pub maps: Vec<String>,
    /// Points repartition for round-based modes, applied once the mode is loaded.
    pub points_repartition: Option<Vec<i32>>,
    /// Name used in matchsettings file names and logs.
    pub label: String,
}

/// Actions on the game server.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Chat {
        to: Recipients,
        text: Text,
    },
    Kick {
        login: String,
        reason: String,
    },
    Ban {
        login: String,
        reason: String,
        blacklist: bool,
    },
    Unban {
        login: String,
    },
    Mute {
        login: String,
    },
    Unmute {
        login: String,
    },
    ForceSpectator {
        login: String,
        mode: SpectatorModeDto,
    },
    ForceTeam {
        login: String,
        team: i32,
    },
    /// Hot mode change without disconnecting players.
    SwitchMode(ModeSwitch),
    SetModeSettings {
        settings: ScriptSettings,
    },
    SetPointsRepartition {
        points: Vec<i32>,
    },
    RestartMap,
    NextMap,
    JumpToMap {
        uid: String,
    },
    ForceEndRound,
    Pause {
        active: bool,
    },
    StopWarmUp,
    SetPlayerPoints {
        login: String,
        round: Option<i32>,
        map: Option<i32>,
        r#match: Option<i32>,
    },
    SetTeamPoints {
        team: i32,
        round: Option<i32>,
        map: Option<i32>,
        r#match: Option<i32>,
    },
    /// Upload a map (bytes) and add or insert it in the playlist.
    UploadMap {
        file: String,
        #[serde(skip)]
        data: Vec<u8>,
        insert_next: bool,
    },
    AddMap {
        file: String,
    },
    RemoveMap {
        file: String,
    },
    /// Replaces the playlist with these files, in this order (reorder, clear, prune). Files
    /// must already be on the server; the current map keeps running and the one after it in
    /// the new order is played next.
    SetPlaylist {
        files: Vec<String>,
    },
    /// Map (already in the playlist) played after the current one.
    ChooseNextMap {
        file: String,
    },
    Manialink {
        to: Recipients,
        xml: String,
        timeout_ms: i32,
        hide_on_click: bool,
    },
    AddGuest {
        login: String,
    },
    RemoveGuest {
        login: String,
    },
    CleanGuests,
    SetPassword {
        password: String,
    },
    ConnectBot,
    DisconnectBots,
    /// Escape hatch for internal plugins: any XML-RPC call.
    Raw {
        method: String,
        #[serde(skip)]
        params: Vec<Value>,
    },
}

/// Serializable mirror of [`SpectatorMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpectatorModeDto {
    UserSelectable,
    Spectator,
    Player,
    SpectatorSelectable,
}

impl From<SpectatorModeDto> for SpectatorMode {
    fn from(m: SpectatorModeDto) -> Self {
        match m {
            SpectatorModeDto::UserSelectable => SpectatorMode::UserSelectable,
            SpectatorModeDto::Spectator => SpectatorMode::Spectator,
            SpectatorModeDto::Player => SpectatorMode::Player,
            SpectatorModeDto::SpectatorSelectable => SpectatorMode::SpectatorSelectable,
        }
    }
}

impl Command {
    /// Name recorded in the audit log, or `None` for routine commands (chat, UI).
    pub fn audit_name(&self) -> Option<&'static str> {
        Some(match self {
            Command::Chat { .. } | Command::Manialink { .. } | Command::Raw { .. } => return None,
            Command::Kick { .. } => "kick",
            Command::Ban { .. } => "ban",
            Command::Unban { .. } => "unban",
            Command::Mute { .. } => "mute",
            Command::Unmute { .. } => "unmute",
            Command::ForceSpectator { .. } => "force_spectator",
            Command::ForceTeam { .. } => "force_team",
            Command::SwitchMode(_) => "switch_mode",
            Command::SetModeSettings { .. } => "set_mode_settings",
            Command::SetPointsRepartition { .. } => "set_points_repartition",
            Command::RestartMap => "restart_map",
            Command::NextMap => "next_map",
            Command::JumpToMap { .. } => "jump_to_map",
            Command::ForceEndRound => "force_end_round",
            Command::Pause { .. } => "pause",
            Command::StopWarmUp => "stop_warmup",
            Command::SetPlayerPoints { .. } => "set_player_points",
            Command::SetTeamPoints { .. } => "set_team_points",
            Command::UploadMap { .. } => "upload_map",
            Command::AddMap { .. } => "add_map",
            Command::RemoveMap { .. } => "remove_map",
            Command::SetPlaylist { .. } => "set_playlist",
            Command::ChooseNextMap { .. } => "choose_next_map",
            Command::AddGuest { .. } => "add_guest",
            Command::RemoveGuest { .. } => "remove_guest",
            Command::CleanGuests => "clean_guests",
            Command::SetPassword { .. } => "set_password",
            Command::ConnectBot => "connect_bot",
            Command::DisconnectBots => "disconnect_bots",
        })
    }
}

/// Errors returned for a command.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("the game server is not connected")]
    NotConnected,
    #[error(transparent)]
    Gbx(#[from] GbxError),
    #[error("{0}")]
    Invalid(String),
    #[error("cannot write {path}: {source}")]
    File {
        path: String,
        source: std::io::Error,
    },
    #[error("the runtime is shut down")]
    Closed,
}

/// A command with its origin and an optional reply channel.
#[derive(Debug)]
pub struct CommandRequest {
    pub command: Command,
    pub actor: Actor,
    pub reply: Option<oneshot::Sender<Result<Value, CommandError>>>,
}

/// Handle to send commands. Cheap to clone.
#[derive(Debug, Clone)]
pub struct CommandHandle {
    tx: mpsc::Sender<CommandRequest>,
    actor: Actor,
}

impl CommandHandle {
    pub(crate) fn new(tx: mpsc::Sender<CommandRequest>, actor: Actor) -> CommandHandle {
        CommandHandle { tx, actor }
    }

    /// Same channel, different actor (e.g. a player who typed an admin command).
    pub fn as_actor(&self, actor: Actor) -> CommandHandle {
        CommandHandle {
            tx: self.tx.clone(),
            actor,
        }
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// Queues a command without waiting; failures are logged by the runtime.
    pub fn send(&self, command: Command) {
        let request = CommandRequest {
            command,
            actor: self.actor.clone(),
            reply: None,
        };
        if let Err(err) = self.tx.try_send(request) {
            tracing::warn!(actor = %self.actor, "dropping command: {err}");
        }
    }

    /// Sends a command and waits for its result.
    pub async fn call(&self, command: Command) -> Result<Value, CommandError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(CommandRequest {
                command,
                actor: self.actor.clone(),
                reply: Some(reply),
            })
            .await
            .map_err(|_| CommandError::Closed)?;
        rx.await.map_err(|_| CommandError::Closed)?
    }

    /// Chat message to everyone.
    pub fn chat_all(&self, text: impl Into<Text>) {
        self.send(Command::Chat {
            to: Recipients::All,
            text: text.into(),
        });
    }

    /// Chat message to one player.
    pub fn chat_to(&self, login: &str, text: impl Into<Text>) {
        self.send(Command::Chat {
            to: Recipients::login(login),
            text: text.into(),
        });
    }
}
