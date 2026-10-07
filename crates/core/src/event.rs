//! Events broadcast to plugins.

use std::sync::Arc;

use pitwall_gbx::Callback;
use pitwall_gbx::callbacks::{PlayerEvent, Respawn, Scores, WayPoint};
use pitwall_gbx::types::{MapInfo, VersionInfo};
use serde::Serialize;

use crate::chat::Invocation;
use crate::state::Player;

/// Who triggered an action (used for permissions and the audit log).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Actor {
    /// The controller itself.
    System,
    /// A plugin acting on its own.
    Plugin(String),
    /// A player in game (login).
    Player(String),
    /// The server console (stdin).
    Console,
    /// A web dashboard user.
    Web(String),
    /// A Discord user.
    Discord(String),
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Actor::System => f.write_str("system"),
            Actor::Plugin(name) => write!(f, "plugin:{name}"),
            Actor::Player(login) => write!(f, "player:{login}"),
            Actor::Console => f.write_str("console"),
            Actor::Web(user) => write!(f, "web:{user}"),
            Actor::Discord(user) => write!(f, "discord:{user}"),
        }
    }
}

/// An admin action, recorded by the storage plugin.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    pub at: i64,
    pub actor: Actor,
    pub action: String,
    pub details: serde_json::Value,
    pub success: bool,
}

/// Everything plugins can react to.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Connected (or reconnected) and state synchronized.
    Connected {
        version: VersionInfo,
    },
    /// Connection to the game server lost; the runtime retries.
    Disconnected,
    /// Every raw callback, for plugins that need more than the derived events.
    Callback {
        callback: Arc<Callback>,
    },
    PlayerJoined {
        player: Player,
    },
    PlayerLeft {
        player: Player,
        reason: String,
    },
    /// Team, spectator status or language changed.
    PlayerUpdated {
        player: Player,
    },
    /// Plain chat message (commands are dispatched as [`Event::Command`]).
    Chat {
        login: String,
        text: String,
    },
    /// A command from chat, console, web or Discord, already permission-checked.
    Command {
        invocation: Invocation,
    },
    /// A click on a manialink action.
    ManialinkAnswer {
        login: String,
        action: String,
        entries: Vec<(String, String)>,
    },
    Checkpoint {
        waypoint: Arc<WayPoint>,
    },
    Finish {
        waypoint: Arc<WayPoint>,
    },
    GiveUp {
        event: PlayerEvent,
    },
    Respawn {
        event: Arc<Respawn>,
    },
    StartLine {
        event: PlayerEvent,
    },
    MatchStart,
    MatchEnd,
    MapStart {
        map: Arc<MapInfo>,
        restarted: bool,
    },
    MapEnd {
        map: Option<Arc<MapInfo>>,
    },
    RoundStart {
        count: i32,
    },
    RoundEnd {
        count: i32,
    },
    Scores {
        scores: Arc<Scores>,
    },
    WarmUp {
        active: bool,
    },
    Pause {
        active: bool,
    },
    /// The mode script effectively changed (after a map load).
    ModeChanged {
        script: String,
    },
    /// An admin action was executed.
    Audit {
        entry: AuditEntry,
    },
    /// Plugin-to-plugin events (cup progress, UI requests, ...).
    Custom {
        source: String,
        name: String,
        payload: serde_json::Value,
    },
}

impl Event {
    /// Builds a custom event.
    pub fn custom(source: &str, name: &str, payload: serde_json::Value) -> Event {
        Event::Custom {
            source: source.to_owned(),
            name: name.to_owned(),
            payload,
        }
    }

    /// Short name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Connected { .. } => "connected",
            Event::Disconnected => "disconnected",
            Event::Callback { .. } => "callback",
            Event::PlayerJoined { .. } => "player_joined",
            Event::PlayerLeft { .. } => "player_left",
            Event::PlayerUpdated { .. } => "player_updated",
            Event::Chat { .. } => "chat",
            Event::Command { .. } => "command",
            Event::ManialinkAnswer { .. } => "manialink_answer",
            Event::Checkpoint { .. } => "checkpoint",
            Event::Finish { .. } => "finish",
            Event::GiveUp { .. } => "give_up",
            Event::Respawn { .. } => "respawn",
            Event::StartLine { .. } => "start_line",
            Event::MatchStart => "match_start",
            Event::MatchEnd => "match_end",
            Event::MapStart { .. } => "map_start",
            Event::MapEnd { .. } => "map_end",
            Event::RoundStart { .. } => "round_start",
            Event::RoundEnd { .. } => "round_end",
            Event::Scores { .. } => "scores",
            Event::WarmUp { .. } => "warm_up",
            Event::Pause { .. } => "pause",
            Event::ModeChanged { .. } => "mode_changed",
            Event::Audit { .. } => "audit",
            Event::Custom { .. } => "custom",
        }
    }
}
