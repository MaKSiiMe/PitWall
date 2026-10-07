//! Server state tracked from callbacks, readable by every plugin.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use pitwall_gbx::callbacks::Scores;
use pitwall_gbx::types::{MapInfo, ScriptSettings, VersionInfo};
use serde::Serialize;

use crate::i18n::Lang;
use crate::roles::Role;

/// A connected player (the server itself is never listed).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Player {
    pub login: String,
    pub nickname: String,
    pub account_id: String,
    /// Zone path, e.g. `World|Europe|France`.
    pub zone: String,
    pub lang: Lang,
    /// 0 = blue, 1 = red, -1 = none.
    pub team: i32,
    pub spectator: bool,
    pub role: Role,
    /// Unix milliseconds.
    pub joined_at: i64,
}

/// Snapshot of what the controller knows about the server.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ServerState {
    pub connected: bool,
    pub version: Option<VersionInfo>,
    pub server_login: String,
    pub players: BTreeMap<String, Player>,
    pub map: Option<MapInfo>,
    /// Current mode script, e.g. `Trackmania/TM_TimeAttack_Online.Script.txt`.
    pub script: String,
    pub script_settings: ScriptSettings,
    pub warmup: bool,
    pub paused: bool,
    /// Round counter of the current map (script flow `count`).
    pub round: i32,
    pub match_running: bool,
    pub last_scores: Option<Arc<Scores>>,
}

impl ServerState {
    /// Logins of the players currently racing (not spectating).
    pub fn racing_logins(&self) -> Vec<String> {
        self.players
            .values()
            .filter(|p| !p.spectator)
            .map(|p| p.login.clone())
            .collect()
    }

    pub fn lang_of(&self, login: &str) -> Option<Lang> {
        self.players.get(login).map(|p| p.lang)
    }
}

/// Shared, cheaply clonable handle on the state.
#[derive(Debug, Clone, Default)]
pub struct StateHandle {
    inner: Arc<RwLock<ServerState>>,
}

impl StateHandle {
    /// A copy of the whole state.
    pub fn snapshot(&self) -> ServerState {
        self.read(Clone::clone)
    }

    /// Reads part of the state without cloning everything.
    pub fn read<T>(&self, f: impl FnOnce(&ServerState) -> T) -> T {
        f(&self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Mutates the state. Only the runtime should call this.
    pub fn write<T>(&self, f: impl FnOnce(&mut ServerState) -> T) -> T {
        f(&mut self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    pub fn player(&self, login: &str) -> Option<Player> {
        self.read(|s| s.players.get(login).cloned())
    }
}

/// Current time in Unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
