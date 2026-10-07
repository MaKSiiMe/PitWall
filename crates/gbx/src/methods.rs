//! Typed wrappers for the XML-RPC methods the controller uses.
//!
//! Signatures follow the official method list (`system.listMethods` / Trackmania wiki).

use crate::client::GbxClient;
use crate::error::GbxError;
use crate::types::{
    CurrentAndNext, DetailedPlayerInfo, MapInfo, ModeScriptInfo, PlayerInfo, ScriptSettings,
    ServerOptions, ServerStatus, SystemInfo, VersionInfo,
};
use crate::value::Value;

/// Spectator mode accepted by `ForceSpectator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectatorMode {
    /// The player chooses.
    UserSelectable = 0,
    /// Forced spectator.
    Spectator = 1,
    /// Forced player.
    Player = 2,
    /// Spectator, but the player may switch back.
    SpectatorSelectable = 3,
}

impl GbxClient {
    // --- session -------------------------------------------------------------------------------

    /// `Authenticate(login, password)`.
    pub async fn authenticate(&self, login: &str, password: &str) -> Result<(), GbxError> {
        self.call_ok("Authenticate", vec![login.into(), password.into()])
            .await
    }

    /// `SetApiVersion(version)`.
    pub async fn set_api_version(&self, version: &str) -> Result<(), GbxError> {
        self.call_ok("SetApiVersion", vec![version.into()]).await
    }

    /// `EnableCallbacks(enable)`.
    pub async fn enable_callbacks(&self, enable: bool) -> Result<(), GbxError> {
        self.call_ok("EnableCallbacks", vec![enable.into()]).await
    }

    /// `GetVersion()`.
    pub async fn get_version(&self) -> Result<VersionInfo, GbxError> {
        self.call_as("GetVersion", vec![]).await
    }

    /// `GetStatus()`.
    pub async fn get_status(&self) -> Result<ServerStatus, GbxError> {
        self.call_as("GetStatus", vec![]).await
    }

    /// `GetSystemInfo()`.
    pub async fn get_system_info(&self) -> Result<SystemInfo, GbxError> {
        self.call_as("GetSystemInfo", vec![]).await
    }

    /// `GetServerOptions()`.
    pub async fn get_server_options(&self) -> Result<ServerOptions, GbxError> {
        self.call_as("GetServerOptions", vec![]).await
    }

    /// `GetMapsDirectory()`.
    pub async fn get_maps_directory(&self) -> Result<String, GbxError> {
        self.call_as("GetMapsDirectory", vec![]).await
    }

    // --- mode script -----------------------------------------------------------------------------

    /// `TriggerModeScriptEventArray(method, params)`: calls a mode script XmlRpc method.
    pub async fn trigger_script(&self, method: &str, params: &[&str]) -> Result<(), GbxError> {
        let params: Vec<Value> = params.iter().map(|p| Value::from(*p)).collect();
        self.call_ok(
            "TriggerModeScriptEventArray",
            vec![method.into(), Value::Array(params)],
        )
        .await
    }

    /// Enables the mode script callbacks (`XmlRpc.EnableCallbacks`).
    pub async fn enable_script_callbacks(&self) -> Result<(), GbxError> {
        self.trigger_script("XmlRpc.EnableCallbacks", &["true"])
            .await
    }

    /// Selects the mode script XmlRpc API version (`XmlRpc.SetApiVersion`).
    pub async fn set_script_api_version(&self, version: &str) -> Result<(), GbxError> {
        self.trigger_script("XmlRpc.SetApiVersion", &[version])
            .await
    }

    /// `GetScriptName()`: current and next mode script.
    pub async fn get_script_name(&self) -> Result<CurrentAndNext<String>, GbxError> {
        self.call_as("GetScriptName", vec![]).await
    }

    /// `SetScriptName(name)`: takes effect at the next map (re)start.
    pub async fn set_script_name(&self, name: &str) -> Result<(), GbxError> {
        self.call_ok("SetScriptName", vec![name.into()]).await
    }

    /// `GetModeScriptInfo()`.
    pub async fn get_mode_script_info(&self) -> Result<ModeScriptInfo, GbxError> {
        self.call_as("GetModeScriptInfo", vec![]).await
    }

    /// `GetModeScriptSettings()`.
    pub async fn get_mode_script_settings(&self) -> Result<ScriptSettings, GbxError> {
        self.call_as("GetModeScriptSettings", vec![]).await
    }

    /// `SetModeScriptSettings(settings)`.
    pub async fn set_mode_script_settings(
        &self,
        settings: &ScriptSettings,
    ) -> Result<(), GbxError> {
        let members = settings
            .iter()
            .map(|(k, v)| (k.clone(), Value::from(v.clone())))
            .collect();
        self.call_ok("SetModeScriptSettings", vec![Value::Struct(members)])
            .await
    }

    /// `LoadMatchSettings(file)`: file relative to the Maps directory. Returns the number of maps.
    pub async fn load_match_settings(&self, file: &str) -> Result<i32, GbxError> {
        self.call_as("LoadMatchSettings", vec![file.into()]).await
    }

    /// `SaveMatchSettings(file)`. Returns the number of maps.
    pub async fn save_match_settings(&self, file: &str) -> Result<i32, GbxError> {
        self.call_as("SaveMatchSettings", vec![file.into()]).await
    }

    // --- game flow ---------------------------------------------------------------------------------

    /// `RestartMap()`.
    pub async fn restart_map(&self) -> Result<(), GbxError> {
        self.call_ok("RestartMap", vec![]).await
    }

    /// `NextMap()`.
    pub async fn next_map(&self) -> Result<(), GbxError> {
        self.call_ok("NextMap", vec![]).await
    }

    /// `JumpToMapIdent(uid)`.
    pub async fn jump_to_map(&self, uid: &str) -> Result<(), GbxError> {
        self.call_ok("JumpToMapIdent", vec![uid.into()]).await
    }

    /// `ChooseNextMap(file)`.
    pub async fn choose_next_map(&self, file: &str) -> Result<(), GbxError> {
        self.call_ok("ChooseNextMap", vec![file.into()]).await
    }

    /// Ends the current round in round-based modes (`Trackmania.ForceEndRound`).
    pub async fn force_end_round(&self) -> Result<(), GbxError> {
        self.trigger_script("Trackmania.ForceEndRound", &[]).await
    }

    /// Starts or stops the mode's pause (`Maniaplanet.Pause.SetActive`).
    pub async fn set_pause(&self, active: bool) -> Result<(), GbxError> {
        self.trigger_script(
            "Maniaplanet.Pause.SetActive",
            &[if active { "true" } else { "false" }],
        )
        .await
    }

    /// Stops the whole warm-up (`Trackmania.WarmUp.ForceStop`).
    pub async fn force_stop_warmup(&self) -> Result<(), GbxError> {
        self.trigger_script("Trackmania.WarmUp.ForceStop", &[])
            .await
    }

    /// Requests a `Trackmania.Scores` callback (`Trackmania.GetScores`).
    pub async fn request_scores(&self, response_id: &str) -> Result<(), GbxError> {
        self.trigger_script("Trackmania.GetScores", &[response_id])
            .await
    }

    /// Overrides a player's points (`Trackmania.SetPlayerPoints`). `None` keeps the value.
    pub async fn set_player_points(
        &self,
        login: &str,
        round: Option<i32>,
        map: Option<i32>,
        matc: Option<i32>,
    ) -> Result<(), GbxError> {
        let fmt = |v: Option<i32>| v.map(|v| v.to_string()).unwrap_or_default();
        let (r, m, t) = (fmt(round), fmt(map), fmt(matc));
        self.trigger_script("Trackmania.SetPlayerPoints", &[login, &r, &m, &t])
            .await
    }

    /// Overrides a team's points (`Trackmania.SetTeamPoints`, team 0 = blue, 1 = red).
    pub async fn set_team_points(
        &self,
        team: i32,
        round: Option<i32>,
        map: Option<i32>,
        matc: Option<i32>,
    ) -> Result<(), GbxError> {
        let fmt = |v: Option<i32>| v.map(|v| v.to_string()).unwrap_or_default();
        let (id, r, m, t) = (team.to_string(), fmt(round), fmt(map), fmt(matc));
        self.trigger_script("Trackmania.SetTeamPoints", &[&id, &r, &m, &t])
            .await
    }

    /// Sets the points repartition of round-based modes.
    pub async fn set_points_repartition(&self, points: &[i32]) -> Result<(), GbxError> {
        let points: Vec<String> = points.iter().map(ToString::to_string).collect();
        let refs: Vec<&str> = points.iter().map(String::as_str).collect();
        self.trigger_script("Trackmania.SetPointsRepartition", &refs)
            .await
    }

    // --- maps --------------------------------------------------------------------------------------

    /// `GetCurrentMapInfo()`.
    pub async fn get_current_map_info(&self) -> Result<MapInfo, GbxError> {
        self.call_as("GetCurrentMapInfo", vec![]).await
    }

    /// `GetMapList(max, start)`.
    pub async fn get_map_list(&self, max: i32, start: i32) -> Result<Vec<MapInfo>, GbxError> {
        self.call_as("GetMapList", vec![max.into(), start.into()])
            .await
    }

    /// `GetCurrentMapIndex()`.
    pub async fn get_current_map_index(&self) -> Result<i32, GbxError> {
        self.call_as("GetCurrentMapIndex", vec![]).await
    }

    /// `WriteFile(path, data)`: path relative to the Maps directory.
    pub async fn write_file(&self, path: &str, data: Vec<u8>) -> Result<(), GbxError> {
        self.call_ok("WriteFile", vec![path.into(), Value::Base64(data)])
            .await
    }

    /// `AddMap(file)`.
    pub async fn add_map(&self, file: &str) -> Result<(), GbxError> {
        self.call_ok("AddMap", vec![file.into()]).await
    }

    /// `InsertMap(file)`: after the current map.
    pub async fn insert_map(&self, file: &str) -> Result<(), GbxError> {
        self.call_ok("InsertMap", vec![file.into()]).await
    }

    /// `RemoveMap(file)`.
    pub async fn remove_map(&self, file: &str) -> Result<(), GbxError> {
        self.call_ok("RemoveMap", vec![file.into()]).await
    }

    /// `GetMapInfo(file)`.
    pub async fn get_map_info(&self, file: &str) -> Result<MapInfo, GbxError> {
        self.call_as("GetMapInfo", vec![file.into()]).await
    }

    // --- players -------------------------------------------------------------------------------------

    /// `GetPlayerList(max, start, 1)`: includes the server itself (`PlayerId == 0`).
    pub async fn get_player_list(&self, max: i32, start: i32) -> Result<Vec<PlayerInfo>, GbxError> {
        self.call_as("GetPlayerList", vec![max.into(), start.into(), 1.into()])
            .await
    }

    /// `GetDetailedPlayerInfo(login)`.
    pub async fn get_detailed_player_info(
        &self,
        login: &str,
    ) -> Result<DetailedPlayerInfo, GbxError> {
        self.call_as("GetDetailedPlayerInfo", vec![login.into()])
            .await
    }

    /// `Kick(login, message)`.
    pub async fn kick(&self, login: &str, message: &str) -> Result<(), GbxError> {
        self.call_ok("Kick", vec![login.into(), message.into()])
            .await
    }

    /// `Ban(login, message)`.
    pub async fn ban(&self, login: &str, message: &str) -> Result<(), GbxError> {
        self.call_ok("Ban", vec![login.into(), message.into()])
            .await
    }

    /// `BanAndBlackList(login, message, save)`.
    pub async fn ban_and_blacklist(
        &self,
        login: &str,
        message: &str,
        save: bool,
    ) -> Result<(), GbxError> {
        self.call_ok(
            "BanAndBlackList",
            vec![login.into(), message.into(), save.into()],
        )
        .await
    }

    /// `UnBan(login)`.
    pub async fn unban(&self, login: &str) -> Result<(), GbxError> {
        self.call_ok("UnBan", vec![login.into()]).await
    }

    /// `Ignore(login)`: mutes the player in the chat.
    pub async fn ignore(&self, login: &str) -> Result<(), GbxError> {
        self.call_ok("Ignore", vec![login.into()]).await
    }

    /// `UnIgnore(login)`.
    pub async fn unignore(&self, login: &str) -> Result<(), GbxError> {
        self.call_ok("UnIgnore", vec![login.into()]).await
    }

    /// `ForceSpectator(login, mode)`.
    pub async fn force_spectator(&self, login: &str, mode: SpectatorMode) -> Result<(), GbxError> {
        self.call_ok("ForceSpectator", vec![login.into(), (mode as i32).into()])
            .await
    }

    /// `SpectatorReleasePlayerSlot(login)`.
    pub async fn spectator_release_player_slot(&self, login: &str) -> Result<(), GbxError> {
        self.call_ok("SpectatorReleasePlayerSlot", vec![login.into()])
            .await
    }

    /// `ForcePlayerTeam(login, team)` (0 = blue, 1 = red).
    pub async fn force_player_team(&self, login: &str, team: i32) -> Result<(), GbxError> {
        self.call_ok("ForcePlayerTeam", vec![login.into(), team.into()])
            .await
    }

    /// `AddGuest(login)`.
    pub async fn add_guest(&self, login: &str) -> Result<(), GbxError> {
        self.call_ok("AddGuest", vec![login.into()]).await
    }

    /// `RemoveGuest(login)`.
    pub async fn remove_guest(&self, login: &str) -> Result<(), GbxError> {
        self.call_ok("RemoveGuest", vec![login.into()]).await
    }

    /// `CleanGuestList()`.
    pub async fn clean_guest_list(&self) -> Result<(), GbxError> {
        self.call_ok("CleanGuestList", vec![]).await
    }

    /// `SetServerPassword(password)`.
    pub async fn set_server_password(&self, password: &str) -> Result<(), GbxError> {
        self.call_ok("SetServerPassword", vec![password.into()])
            .await
    }

    /// `ConnectFakePlayer()`: debug bot, returns its login.
    pub async fn connect_fake_player(&self) -> Result<String, GbxError> {
        self.call_as("ConnectFakePlayer", vec![]).await
    }

    /// `DisconnectFakePlayer(login)`; `"*"` disconnects all bots.
    pub async fn disconnect_fake_player(&self, login: &str) -> Result<(), GbxError> {
        self.call_ok("DisconnectFakePlayer", vec![login.into()])
            .await
    }

    // --- chat ------------------------------------------------------------------------------------------

    /// `ChatSendServerMessage(text)`.
    pub async fn chat_send(&self, text: &str) -> Result<(), GbxError> {
        self.call_ok("ChatSendServerMessage", vec![text.into()])
            .await
    }

    /// `ChatSendServerMessageToLogin(text, logins)`: comma-separated logins.
    pub async fn chat_send_to(&self, text: &str, logins: &str) -> Result<(), GbxError> {
        self.call_ok(
            "ChatSendServerMessageToLogin",
            vec![text.into(), logins.into()],
        )
        .await
    }

    // --- manialinks ------------------------------------------------------------------------------------

    /// `SendDisplayManialinkPage(xml, timeout_ms, hide_on_click)` to everyone.
    pub async fn send_manialink(
        &self,
        xml: &str,
        timeout_ms: i32,
        hide_on_click: bool,
    ) -> Result<(), GbxError> {
        self.call_ok(
            "SendDisplayManialinkPage",
            vec![xml.into(), timeout_ms.into(), hide_on_click.into()],
        )
        .await
    }

    /// `SendDisplayManialinkPageToLogin(logins, xml, timeout_ms, hide_on_click)`.
    pub async fn send_manialink_to(
        &self,
        logins: &str,
        xml: &str,
        timeout_ms: i32,
        hide_on_click: bool,
    ) -> Result<(), GbxError> {
        self.call_ok(
            "SendDisplayManialinkPageToLogin",
            vec![
                logins.into(),
                xml.into(),
                timeout_ms.into(),
                hide_on_click.into(),
            ],
        )
        .await
    }

    /// `SendHideManialinkPage()`: hides every controller manialink for everyone.
    pub async fn hide_all_manialinks(&self) -> Result<(), GbxError> {
        self.call_ok("SendHideManialinkPage", vec![]).await
    }
}
