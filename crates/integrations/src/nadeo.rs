//! Nadeo web services with a dedicated server account.
//!
//! - token: `POST https://prod.trackmania.core.nadeo.online/v2/authentication/token/basic`
//!   (basic auth `login:password`, body `{"audience": ...}`), refresh with
//!   `POST .../v2/authentication/token/refresh` and `Authorization: nadeo_v1 t=<refresh>`;
//! - map info: `GET https://prod.trackmania.core.nadeo.online/maps/by-uid/?mapUidList=` (NadeoServices),
//!   whose `fileUrl` points to the `.Map.Gbx`;
//! - club campaign: `GET https://live-services.trackmania.nadeo.live/api/token/club/{club}/campaign/{campaign}`
//!   (NadeoLiveServices), maps in `campaign.playlist[].mapUid`;
//! - club room: `GET https://live-services.trackmania.nadeo.live/api/token/club/{club}/room/{room}`
//!   (NadeoLiveServices), maps in `room.maps` (uids; rarely an object with keys "0", "1"…);
//!   403 `clubMemberRole:error-notMember` for a private room;
//! - world top: `GET {LIVE}/api/token/leaderboard/group/Personal_Best/map/{uid}/top?length=&onlyWorld=true`
//!   (NadeoLiveServices; server accounts cannot use zones, so `onlyWorld=true`);
//! - personal bests: `GET {CORE}/v2/mapRecords/by-account/?accountIdList=&mapId=&gameMode=TimeAttack`
//!   (NadeoServices; one map, map *id* not uid, secret thresholds give time 4294967295);
//! - world position of a time: `POST {LIVE}/api/token/leaderboard/group/map?scores[{uid}]={time}`
//!   with body `{"maps":[{"mapUid","groupUid":"Personal_Best"}]}` (one score per map and request,
//!   positions are those a new record of that time would get, up to ~3 h late).
//!
//! Source: <https://webservices.openplanet.dev>.

use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{IntegrationError, check};

const CORE: &str = "https://prod.trackmania.core.nadeo.online";
const LIVE: &str = "https://live-services.trackmania.nadeo.live";
/// Access tokens last about one hour; refresh a bit earlier.
const TOKEN_LIFETIME: Duration = Duration::from_secs(50 * 60);

/// Token audiences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    NadeoServices,
    NadeoLiveServices,
}

impl Audience {
    fn name(self) -> &'static str {
        match self {
            Audience::NadeoServices => "NadeoServices",
            Audience::NadeoLiveServices => "NadeoLiveServices",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Clone)]
struct Token {
    access: String,
    refresh: String,
    obtained: Instant,
}

/// Map information from the core API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NadeoMap {
    pub map_uid: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub author_score: i64,
    pub file_url: String,
    #[serde(default)]
    pub filename: String,
    /// Id used by the records API.
    #[serde(default)]
    pub map_id: String,
}

/// One line of a map's world leaderboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardEntry {
    pub account_id: String,
    pub position: i64,
    /// Time in ms; -1 below a secret threshold.
    pub score: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TopResponse {
    tops: Vec<TopZone>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TopZone {
    top: Vec<LeaderboardEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MapRecordEntry {
    account_id: String,
    record_score: RecordScore,
}

#[derive(Debug, Deserialize)]
struct RecordScore {
    time: i64,
}

/// Time of a record hidden by a secret threshold.
const SECRET_TIME: i64 = 4_294_967_295;

#[derive(Debug, Deserialize)]
struct PositionResponse {
    #[serde(default)]
    zones: Vec<PositionZone>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionZone {
    #[serde(default)]
    zone_name: String,
    ranking: Ranking,
}

#[derive(Debug, Deserialize)]
struct Ranking {
    position: i64,
}

/// Login of a TM2020 account: its id's 16 bytes in unpadded base64url.
pub fn account_id_of_login(login: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(login)
        .ok()?;
    if bytes.len() != 16 {
        return None;
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

#[derive(Debug, Deserialize)]
struct CampaignResponse {
    campaign: Campaign,
}

#[derive(Debug, Deserialize)]
struct Campaign {
    #[serde(default)]
    name: String,
    #[serde(default)]
    playlist: Vec<PlaylistEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistEntry {
    #[serde(default)]
    position: i64,
    map_uid: String,
}

#[derive(Debug, Deserialize)]
struct RoomResponse {
    #[serde(default)]
    name: String,
    room: Room,
}

#[derive(Debug, Deserialize)]
struct Room {
    #[serde(default)]
    maps: RoomMaps,
}

/// `room.maps`: an array of uids, or in rare cases an object keyed "0", "1"…
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RoomMaps {
    List(Vec<String>),
    Indexed(std::collections::BTreeMap<String, String>),
}

impl Default for RoomMaps {
    fn default() -> Self {
        RoomMaps::List(Vec::new())
    }
}

impl RoomMaps {
    fn into_uids(self) -> Vec<String> {
        match self {
            RoomMaps::List(uids) => uids,
            RoomMaps::Indexed(map) => {
                let mut entries: Vec<(u64, String)> = map
                    .into_iter()
                    .filter_map(|(k, v)| Some((k.parse().ok()?, v)))
                    .collect();
                entries.sort_by_key(|(k, _)| *k);
                entries.into_iter().map(|(_, v)| v).collect()
            }
        }
    }
}

/// Maps of a club campaign or room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClubMaps {
    pub name: String,
    pub map_uids: Vec<String>,
}

/// Nadeo API client authenticated with a dedicated server account.
#[derive(Debug)]
pub struct Nadeo {
    http: reqwest::Client,
    login: String,
    password: String,
    core_token: Mutex<Option<Token>>,
    live_token: Mutex<Option<Token>>,
}

impl Nadeo {
    pub fn new(
        http: reqwest::Client,
        login: &str,
        password: &str,
    ) -> Result<Nadeo, IntegrationError> {
        if login.is_empty() || password.is_empty() {
            return Err(IntegrationError::MissingCredentials(
                "Nadeo API (dedicated server account)",
            ));
        }
        Ok(Nadeo {
            http,
            login: login.to_owned(),
            password: password.to_owned(),
            core_token: Mutex::new(None),
            live_token: Mutex::new(None),
        })
    }

    async fn request_token(&self, audience: Audience) -> Result<Token, IntegrationError> {
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", self.login, self.password));
        let response = self
            .http
            .post(format!("{CORE}/v2/authentication/token/basic"))
            .header("Authorization", format!("Basic {basic}"))
            .json(&serde_json::json!({ "audience": audience.name() }))
            .send()
            .await?;
        let tokens: TokenResponse = check("Nadeo auth", response).await?.json().await?;
        Ok(Token {
            access: tokens.access_token,
            refresh: tokens.refresh_token,
            obtained: Instant::now(),
        })
    }

    async fn refresh(&self, token: &Token) -> Result<Token, IntegrationError> {
        let response = self
            .http
            .post(format!("{CORE}/v2/authentication/token/refresh"))
            .header("Authorization", format!("nadeo_v1 t={}", token.refresh))
            .send()
            .await?;
        let tokens: TokenResponse = check("Nadeo refresh", response).await?.json().await?;
        Ok(Token {
            access: tokens.access_token,
            refresh: tokens.refresh_token,
            obtained: Instant::now(),
        })
    }

    /// A valid access token for an audience (cached, refreshed, re-requested if needed).
    async fn token(&self, audience: Audience) -> Result<String, IntegrationError> {
        let slot = match audience {
            Audience::NadeoServices => &self.core_token,
            Audience::NadeoLiveServices => &self.live_token,
        };
        let mut guard = slot.lock().await;
        let fresh = match guard.as_ref() {
            Some(t) if t.obtained.elapsed() < TOKEN_LIFETIME => return Ok(t.access.clone()),
            Some(t) => match self.refresh(t).await {
                Ok(token) => token,
                Err(err) => {
                    tracing::debug!(%err, "token refresh failed, requesting a new one");
                    self.request_token(audience).await?
                }
            },
            None => self.request_token(audience).await?,
        };
        let access = fresh.access.clone();
        *guard = Some(fresh);
        Ok(access)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        audience: Audience,
        url: &str,
    ) -> Result<T, IntegrationError> {
        let token = self.token(audience).await?;
        let response = self
            .http
            .get(url)
            .header("Authorization", format!("nadeo_v1 t={token}"))
            .send()
            .await?;
        Ok(check("Nadeo API", response).await?.json().await?)
    }

    /// Map information by uids (core API).
    pub async fn maps(&self, uids: &[String]) -> Result<Vec<NadeoMap>, IntegrationError> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{CORE}/maps/by-uid/?mapUidList={}", uids.join(","));
        self.get_json(Audience::NadeoServices, &url).await
    }

    /// Downloads a map file from its `fileUrl`.
    pub async fn download(&self, map: &NadeoMap) -> Result<Vec<u8>, IntegrationError> {
        let token = self.token(Audience::NadeoServices).await?;
        let response = self
            .http
            .get(&map.file_url)
            .header("Authorization", format!("nadeo_v1 t={token}"))
            .send()
            .await?;
        Ok(check("Nadeo files", response)
            .await?
            .bytes()
            .await?
            .to_vec())
    }

    /// The first `length` world records of a map (at most 100).
    pub async fn world_top(
        &self,
        map_uid: &str,
        length: u32,
    ) -> Result<Vec<LeaderboardEntry>, IntegrationError> {
        let url = format!(
            "{LIVE}/api/token/leaderboard/group/Personal_Best/map/{map_uid}/top?length={}&onlyWorld=true&offset=0",
            length.min(100)
        );
        let response: TopResponse = self.get_json(Audience::NadeoLiveServices, &url).await?;
        Ok(response
            .tops
            .into_iter()
            .next()
            .map(|z| z.top)
            .unwrap_or_default())
    }

    /// Personal bests (ms) of some accounts on a map, by account id. Hidden times are left out.
    pub async fn personal_bests(
        &self,
        map_id: &str,
        account_ids: &[String],
    ) -> Result<Vec<(String, i64)>, IntegrationError> {
        if account_ids.is_empty() || map_id.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!(
            "{CORE}/v2/mapRecords/by-account/?accountIdList={}&mapId={map_id}&gameMode=TimeAttack",
            account_ids.join(",")
        );
        let records: Vec<MapRecordEntry> = self.get_json(Audience::NadeoServices, &url).await?;
        Ok(records
            .into_iter()
            .filter(|r| r.record_score.time > 0 && r.record_score.time != SECRET_TIME)
            .map(|r| (r.account_id, r.record_score.time))
            .collect())
    }

    /// World position a record of `time` ms gets on a map, if its leaderboard is open.
    pub async fn world_position(
        &self,
        map_uid: &str,
        time: i64,
    ) -> Result<Option<i64>, IntegrationError> {
        let token = self.token(Audience::NadeoLiveServices).await?;
        let response = self
            .http
            .post(format!("{LIVE}/api/token/leaderboard/group/map"))
            .query(&[(format!("scores[{map_uid}]"), time.to_string())])
            .header("Authorization", format!("nadeo_v1 t={token}"))
            .json(&serde_json::json!({
                "maps": [{ "mapUid": map_uid, "groupUid": "Personal_Best" }]
            }))
            .send()
            .await?;
        let maps: Vec<PositionResponse> = check("Nadeo API", response).await?.json().await?;
        Ok(maps.into_iter().next().and_then(|m| {
            let world = m.zones.iter().position(|z| z.zone_name == "World");
            m.zones
                .into_iter()
                .nth(world.unwrap_or(0))
                .map(|z| z.ranking.position)
        }))
    }

    /// Maps of a club campaign, in playlist order.
    pub async fn club_campaign(
        &self,
        club_id: i64,
        campaign_id: i64,
    ) -> Result<ClubMaps, IntegrationError> {
        let url = format!("{LIVE}/api/token/club/{club_id}/campaign/{campaign_id}");
        let response: CampaignResponse = self.get_json(Audience::NadeoLiveServices, &url).await?;
        let mut playlist = response.campaign.playlist;
        playlist.sort_by_key(|p| p.position);
        Ok(ClubMaps {
            name: response.campaign.name,
            map_uids: playlist.into_iter().map(|p| p.map_uid).collect(),
        })
    }

    /// Maps of a club room, in order. Rooms linked to a dedicated server list no maps.
    pub async fn club_room(
        &self,
        club_id: i64,
        room_id: i64,
    ) -> Result<ClubMaps, IntegrationError> {
        let url = format!("{LIVE}/api/token/club/{club_id}/room/{room_id}");
        let response: RoomResponse = self.get_json(Audience::NadeoLiveServices, &url).await?;
        Ok(ClubMaps {
            name: response.name,
            map_uids: response.room.maps.into_uids(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_documented_payloads() {
        let maps: Vec<NadeoMap> = serde_json::from_str(
            r#"[{"author":"73eba009","authorScore":102399,"bronzeScore":154000,"collectionName":"Stadium",
                "fileUrl":"https://core.trackmania.nadeo.live/maps/a747/file","filename":"Laserhawk.Map.Gbx",
                "mapId":"a747","mapUid":"cmJJhEUYqesM6Tqpeds0lQudvOb","name":"$i$a1bLaserhawk"}]"#,
        )
        .unwrap();
        assert_eq!(maps[0].map_uid, "cmJJhEUYqesM6Tqpeds0lQudvOb");
        assert!(maps[0].file_url.ends_with("/file"));

        let campaign: CampaignResponse = serde_json::from_str(
            r#"{"campaign":{"name":"Cup maps","playlist":[{"id":2,"position":1,"mapUid":"B"},{"id":1,"position":0,"mapUid":"A"}]}}"#,
        )
        .unwrap();
        assert_eq!(campaign.campaign.playlist.len(), 2);

        let room: RoomResponse = serde_json::from_str(
            r#"{"activityId":381929,"name":"TM Exchange Pack 1","room":{"id":137873,"name":"TM Exchange Pack 1","maps":["gXTE8","7AHlB"]}}"#,
        )
        .unwrap();
        assert_eq!(room.room.maps.into_uids(), vec!["gXTE8", "7AHlB"]);
        let room: RoomResponse = serde_json::from_str(
            r#"{"name":"Odd","room":{"maps":{"1":"B","0":"A","10":"K","2":"C"}}}"#,
        )
        .unwrap();
        assert_eq!(room.room.maps.into_uids(), vec!["A", "B", "C", "K"]);
    }

    #[test]
    fn decodes_leaderboards() {
        let top: TopResponse = serde_json::from_str(
            r#"{"groupUid":"Personal_Best","mapUid":"ZJw6","tops":[{"zoneId":"301e","zoneName":"World","top":[
                {"accountId":"6ec3711d","zoneId":"301f","zoneName":"Corrèze","position":1,"score":53145,"timestamp":1658783398}]}]}"#,
        )
        .unwrap();
        assert_eq!(top.tops[0].top[0].score, 53145);
        let empty: TopResponse = serde_json::from_str("{}").unwrap();
        assert!(empty.tops.is_empty());

        let records: Vec<MapRecordEntry> = serde_json::from_str(
            r#"[{"accountId":"2fd1f664","gameMode":"TimeAttack","mapId":"11f0","medal":4,
                "recordScore":{"respawnCount":4294967295,"score":0,"time":22318},"removed":false}]"#,
        )
        .unwrap();
        assert_eq!(records[0].record_score.time, 22318);

        let positions: Vec<PositionResponse> = serde_json::from_str(
            r#"[{"groupUid":"Personal_Best","mapUid":"Kn63","score":19976,"zones":[
                {"zoneId":"301e","zoneName":"World","ranking":{"position":7062,"length":0}}]}]"#,
        )
        .unwrap();
        assert_eq!(positions[0].zones[0].ranking.position, 7062);
    }

    #[test]
    fn logins_are_account_ids() {
        // 2fd1f664-85e4-4429-89ca-d49e4d2083a1 in unpadded base64url.
        assert_eq!(
            account_id_of_login("L9H2ZIXkRCmJytSeTSCDoQ").as_deref(),
            Some("2fd1f664-85e4-4429-89ca-d49e4d2083a1")
        );
        assert_eq!(account_id_of_login("not a login"), None);
        assert_eq!(account_id_of_login("*fakeplayer1*"), None);
    }

    #[test]
    fn requires_credentials() {
        let http = reqwest::Client::new();
        assert!(matches!(
            Nadeo::new(http, "", "x"),
            Err(IntegrationError::MissingCredentials(_))
        ));
    }
}
