//! Typed structures returned by XML-RPC methods and legacy callbacks.
//!
//! Field names follow the server (PascalCase). Unknown fields are ignored and most fields default,
//! so a server update adding or removing a field does not break parsing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::value::Value;

/// `GetVersion`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct VersionInfo {
    pub name: String,
    pub title_id: String,
    pub version: String,
    pub build: String,
    pub api_version: String,
}

/// `GetStatus` / `ManiaPlanet.StatusChanged`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ServerStatus {
    pub code: i32,
    pub name: String,
}

/// `GetPlayerList` entries and `ManiaPlanet.PlayerInfoChanged` (`SPlayerInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlayerInfo {
    pub login: String,
    pub nick_name: String,
    pub player_id: i32,
    pub team_id: i32,
    /// Digits encode spectator flags: units = spectator, tens = temporary spectator,
    /// hundreds = pure spectator, ... (see Nadeo documentation).
    pub spectator_status: i32,
    pub ladder_ranking: i32,
    /// Digits encode `ForceSpectator` state, server flags, etc.
    pub flags: i32,
}

impl PlayerInfo {
    /// The dedicated server itself appears in player lists with `PlayerId == 0`.
    pub fn is_server(&self) -> bool {
        self.player_id == 0
    }

    /// `true` when the player is currently spectating.
    pub fn is_spectator(&self) -> bool {
        self.spectator_status % 10 != 0
    }
}

/// Subset of `GetDetailedPlayerInfo` that the controller uses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DetailedPlayerInfo {
    pub login: String,
    pub nick_name: String,
    pub player_id: i32,
    pub team_id: i32,
    /// Zone path, e.g. `World|Europe|France|Ile-de-France`.
    pub path: String,
    pub language: String,
    pub client_version: String,
    pub is_spectator: bool,
    pub is_referee: bool,
}

/// `SMapInfo` (callbacks) and `GetCurrentMapInfo` / `GetMapList` entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct MapInfo {
    /// The server spells it `UId` in method responses and `Uid` in callbacks.
    #[serde(alias = "UId", alias = "Uid")]
    pub uid: String,
    pub name: String,
    pub file_name: String,
    pub author: String,
    pub author_nickname: String,
    pub environnement: String,
    pub mood: String,
    pub bronze_time: i32,
    pub silver_time: i32,
    pub gold_time: i32,
    pub author_time: i32,
    pub copper_price: i32,
    pub lap_race: bool,
    pub nb_laps: i32,
    pub nb_checkpoints: i32,
    pub map_type: String,
    pub map_style: String,
}

/// `GetScriptName`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CurrentAndNext<T> {
    pub current_value: T,
    pub next_value: T,
}

/// Parameter description of a mode script (`GetModeScriptInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ScriptParamDesc {
    pub name: String,
    pub desc: String,
    #[serde(rename = "Type")]
    pub kind: String,
    pub default: String,
}

/// `GetModeScriptInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ModeScriptInfo {
    pub name: String,
    pub compatible_map_types: String,
    pub description: String,
    pub version: String,
    pub param_descs: Vec<ScriptParamDesc>,
    pub command_descs: Vec<ScriptParamDesc>,
}

/// Value of a mode script setting (`S_*`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ScriptSetting {
    Bool(bool),
    Int(i32),
    Real(f64),
    Text(String),
}

impl From<ScriptSetting> for Value {
    fn from(setting: ScriptSetting) -> Self {
        match setting {
            ScriptSetting::Bool(b) => Value::Bool(b),
            ScriptSetting::Int(i) => Value::Int(i),
            ScriptSetting::Real(r) => Value::Double(r),
            ScriptSetting::Text(t) => Value::String(t),
        }
    }
}

/// `GetModeScriptSettings` / `SetModeScriptSettings`.
pub type ScriptSettings = BTreeMap<String, ScriptSetting>;

/// `SPlayerRanking` from `ManiaPlanet.EndMatch`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PlayerRanking {
    pub login: String,
    pub nick_name: String,
    pub player_id: i32,
    pub rank: i32,
    pub best_time: i32,
    pub best_checkpoints: Vec<i32>,
    pub score: i32,
    pub nbr_laps_finished: i32,
    pub ladder_score: f64,
}

/// `SEntryVal` from manialink page answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EntryVal {
    pub name: String,
    pub value: String,
}

/// `GetServerOptions` (subset).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ServerOptions {
    pub name: String,
    pub comment: String,
    pub password: String,
    pub password_for_spectator: String,
    pub current_max_players: i32,
    pub next_max_players: i32,
    pub current_max_spectators: i32,
    pub next_max_spectators: i32,
}

/// `GetSystemInfo` (subset).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SystemInfo {
    pub published_ip: String,
    pub port: i32,
    pub p2_p_port: i32,
    pub title_id: String,
    pub server_login: String,
    pub server_player_id: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_info_accepts_both_uid_spellings() {
        let mut m = BTreeMap::new();
        m.insert("UId".to_owned(), Value::from("abc"));
        m.insert("Name".to_owned(), Value::from("A01"));
        let a: MapInfo = Value::Struct(m).deserialize().unwrap();
        assert_eq!(a.uid, "abc");

        let mut m = BTreeMap::new();
        m.insert("Uid".to_owned(), Value::from("def"));
        let b: MapInfo = Value::Struct(m).deserialize().unwrap();
        assert_eq!(b.uid, "def");
    }

    #[test]
    fn script_settings_keep_their_types() {
        let json = serde_json::json!({"S_TimeLimit": 300, "S_UseTieBreak": true, "S_PointsRepartition": "10,6,4", "S_Ratio": 0.5});
        let settings: ScriptSettings = serde_json::from_value(json).unwrap();
        assert_eq!(settings["S_TimeLimit"], ScriptSetting::Int(300));
        assert_eq!(settings["S_UseTieBreak"], ScriptSetting::Bool(true));
        assert_eq!(
            settings["S_PointsRepartition"],
            ScriptSetting::Text("10,6,4".into())
        );
        assert_eq!(settings["S_Ratio"], ScriptSetting::Real(0.5));
    }

    #[test]
    fn spectator_status_digits() {
        let p = PlayerInfo {
            spectator_status: 2_551_011,
            ..PlayerInfo::default()
        };
        assert!(p.is_spectator());
        let p = PlayerInfo {
            spectator_status: 0,
            player_id: 0,
            ..PlayerInfo::default()
        };
        assert!(!p.is_spectator());
        assert!(p.is_server());
    }
}
