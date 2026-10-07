//! Cup definitions (`config/cups/*.toml`): an ordered list of phases, each one a format.
//!
//! ```toml
//! id = "mini-cup-1"
//! name = { en = "Mini Cup #1", fr = "Mini Cup #1" }
//! registration = ["list", "ingame"]
//! participants = ["login1", "login2"]
//! start = "ready_check"
//! disconnect = { policy = "pause", grace_secs = 120 }
//!
//! [[phase]]
//! id = "quali"
//! name = { en = "Qualifiers", fr = "Qualifications" }
//! format = "timeattack"
//! settings = { S_TimeLimit = 600 }
//! qualify = { top = 4 }
//!
//! [[phase]]
//! id = "final"
//! format = "cup"
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::format::Localized;

/// Individual players or teams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CupKind {
    #[default]
    Players,
    Teams,
}

/// How participants can join a cup. A cup may allow several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    /// Fixed list in the cup file, plus admin additions.
    List,
    /// Every player connected when the cup starts.
    Open,
    /// `/join` in game.
    #[serde(rename = "ingame", alias = "in_game")]
    InGame,
    /// Registration page of the web dashboard.
    Web,
    /// Discord.
    Discord,
}

/// Who starts a phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartPolicy {
    /// An admin types `/cup go`.
    #[default]
    Manual,
    /// Every participant clicks "ready" (or the timeout expires).
    ReadyCheck,
    /// Starts by itself (after `delay_before_secs`).
    Auto,
}

/// What happens when a participant disconnects during a phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectAction {
    /// Nothing: the player keeps their slot and points.
    #[default]
    Keep,
    /// Technical pause until they come back (or the grace period ends).
    Pause,
    /// Disqualified if not back within the grace period.
    Eliminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisconnectPolicy {
    pub policy: DisconnectAction,
    pub grace_secs: u64,
}

impl Default for DisconnectPolicy {
    fn default() -> Self {
        Self {
            policy: DisconnectAction::Keep,
            grace_secs: 120,
        }
    }
}

/// A team of a team cup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TeamDef {
    pub id: String,
    pub name: String,
    pub tag: String,
    /// Hex color without `#` (`0af` or `00aaff`).
    pub color: String,
    pub players: Vec<String>,
}

/// Who plays a phase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseParticipants {
    /// Those qualified by the previous phase (all participants for the first phase).
    #[default]
    Previous,
    /// Every participant still in the cup.
    All,
}

/// Who goes on after a phase.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Qualify {
    #[default]
    All,
    /// The first N of the phase ranking.
    Top(usize),
    /// A function of the cup's Lua rules: `function name(phase, standings) return {ids} end`.
    Lua(String),
}

/// How a phase is played.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseKind {
    /// One match with every phase participant on the server.
    #[default]
    Match,
    /// Single elimination bracket of 1v1 matches (teams cups: blue vs red).
    Bracket,
}

/// One phase of a cup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseDef {
    pub id: String,
    #[serde(default)]
    pub name: Localized,
    /// Format id (see `/formats`).
    pub format: String,
    /// Extra `S_*` settings overriding the format.
    #[serde(default)]
    pub settings: BTreeMap<String, toml::Value>,
    /// Map files for this phase; empty keeps the format's/server's playlist.
    #[serde(default)]
    pub maps: Vec<String>,
    #[serde(default)]
    pub kind: PhaseKind,
    #[serde(default)]
    pub participants: PhaseParticipants,
    #[serde(default)]
    pub qualify: Qualify,
    /// Overrides the cup start policy.
    #[serde(default)]
    pub start: Option<StartPolicy>,
    /// Pause before an `auto` start, to read the results of the previous phase.
    #[serde(default)]
    pub delay_before_secs: u64,
}

impl PhaseDef {
    pub fn display_name(&self, lang: pitwall_core::Lang) -> &str {
        let name = self.name.get(lang);
        if name.is_empty() { &self.id } else { name }
    }
}

fn default_true() -> bool {
    true
}

fn default_ready_timeout() -> u64 {
    120
}

/// A cup definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CupDef {
    pub id: String,
    #[serde(default)]
    pub name: Localized,
    #[serde(default)]
    pub kind: CupKind,
    /// Allowed registration modes (default: the list of the file).
    #[serde(default = "default_registration")]
    pub registration: Vec<RegistrationMode>,
    /// Self-registrations (in game, web, Discord) wait for an admin approval.
    #[serde(default)]
    pub approval: bool,
    #[serde(default)]
    pub max_participants: Option<usize>,
    /// Pre-registered player logins (players cups).
    #[serde(default)]
    pub participants: Vec<String>,
    /// Pre-registered teams (teams cups).
    #[serde(default)]
    pub teams: Vec<TeamDef>,
    #[serde(default)]
    pub start: StartPolicy,
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_secs: u64,
    #[serde(default)]
    pub disconnect: DisconnectPolicy,
    /// Lua rules file, relative to the cup file.
    #[serde(default)]
    pub rules: Option<String>,
    /// Non-participants are forced to spectate during the cup.
    #[serde(default = "default_true")]
    pub lock_server: bool,
    #[serde(rename = "phase")]
    pub phases: Vec<PhaseDef>,
}

fn default_registration() -> Vec<RegistrationMode> {
    vec![RegistrationMode::List]
}

/// Errors in a cup definition.
#[derive(Debug, thiserror::Error)]
pub enum CupDefError {
    #[error("invalid cup file {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("cup {cup}: {message}")]
    Invalid { cup: String, message: String },
}

impl CupDef {
    pub fn from_toml(source: &str, path: &str) -> Result<CupDef, CupDefError> {
        let def: CupDef = toml::from_str(source).map_err(|source| CupDefError::Parse {
            path: path.to_owned(),
            source,
        })?;
        def.validate()?;
        Ok(def)
    }

    fn invalid(&self, message: impl Into<String>) -> CupDefError {
        CupDefError::Invalid {
            cup: self.id.clone(),
            message: message.into(),
        }
    }

    /// Structural checks (formats are checked against the registry by the caller).
    pub fn validate(&self) -> Result<(), CupDefError> {
        if self.id.trim().is_empty() {
            return Err(self.invalid("empty id"));
        }
        if self.phases.is_empty() {
            return Err(self.invalid("at least one [[phase]] is required"));
        }
        let mut ids = BTreeSet::new();
        for phase in &self.phases {
            if !ids.insert(phase.id.as_str()) {
                return Err(self.invalid(format!("duplicate phase id {:?}", phase.id)));
            }
            if phase.kind == PhaseKind::Bracket && self.kind != CupKind::Teams {
                return Err(self.invalid(format!(
                    "phase {:?}: brackets are only supported in teams cups",
                    phase.id
                )));
            }
            if matches!(phase.qualify, Qualify::Lua(_)) && self.rules.is_none() {
                return Err(self.invalid(format!(
                    "phase {:?} uses a Lua qualify rule but the cup has no `rules` file",
                    phase.id
                )));
            }
            if matches!(phase.qualify, Qualify::Top(0)) {
                return Err(self.invalid(format!("phase {:?}: qualify top must be > 0", phase.id)));
            }
        }
        let mut team_ids = BTreeSet::new();
        let mut team_players = BTreeSet::new();
        for team in &self.teams {
            if !team_ids.insert(team.id.as_str()) {
                return Err(self.invalid(format!("duplicate team id {:?}", team.id)));
            }
            for player in &team.players {
                if !team_players.insert(player.as_str()) {
                    return Err(self.invalid(format!("player {player:?} is in several teams")));
                }
            }
        }
        if self.kind == CupKind::Players && !self.teams.is_empty() {
            return Err(self.invalid("`teams` are only allowed with kind = \"teams\""));
        }
        if self.registration.is_empty() {
            return Err(self.invalid("`registration` cannot be empty"));
        }
        Ok(())
    }

    pub fn display_name(&self, lang: pitwall_core::Lang) -> &str {
        let name = self.name.get(lang);
        if name.is_empty() { &self.id } else { name }
    }

    pub fn allows(&self, mode: RegistrationMode) -> bool {
        self.registration.contains(&mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
id = "mini"
registration = ["list", "ingame"]
participants = ["a", "b", "c"]
start = "ready_check"
disconnect = { policy = "pause", grace_secs = 60 }

[[phase]]
id = "quali"
format = "timeattack"
settings = { S_TimeLimit = 600 }
qualify = { top = 2 }

[[phase]]
id = "final"
format = "cup"
start = "auto"
delay_before_secs = 30
"#;

    #[test]
    fn parses_a_cup() {
        let def = CupDef::from_toml(EXAMPLE, "mini.toml").unwrap();
        assert_eq!(def.phases.len(), 2);
        assert_eq!(def.phases[0].qualify, Qualify::Top(2));
        assert_eq!(def.phases[1].qualify, Qualify::All);
        assert_eq!(def.phases[1].start, Some(StartPolicy::Auto));
        assert_eq!(def.disconnect.policy, DisconnectAction::Pause);
        assert!(def.allows(RegistrationMode::InGame));
        assert!(!def.allows(RegistrationMode::Web));
        assert!(def.lock_server);
    }

    #[test]
    fn rejects_mistakes() {
        assert!(CupDef::from_toml("id = \"x\"\nphase = []", "x").is_err());
        let typo = EXAMPLE.replace("qualify = { top = 2 }", "qualifiy = { top = 2 }");
        assert!(
            CupDef::from_toml(&typo, "x").is_err(),
            "unknown fields are rejected"
        );
        let lua = EXAMPLE.replace("{ top = 2 }", "{ lua = \"pick\" }");
        assert!(matches!(
            CupDef::from_toml(&lua, "x"),
            Err(CupDefError::Invalid { .. })
        ));
        let bracket = EXAMPLE.replace("format = \"cup\"", "format = \"teams\"\nkind = \"bracket\"");
        assert!(CupDef::from_toml(&bracket, "x").is_err());
    }
}
