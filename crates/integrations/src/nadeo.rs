//! Nadeo web services with a dedicated server account.
//!
//! - token: `POST https://prod.trackmania.core.nadeo.online/v2/authentication/token/basic`
//!   (basic auth `login:password`, body `{"audience": ...}`), refresh with
//!   `POST .../v2/authentication/token/refresh` and `Authorization: nadeo_v1 t=<refresh>`;
//! - map info: `GET https://prod.trackmania.core.nadeo.online/maps/by-uid/?mapUidList=` (NadeoServices),
//!   whose `fileUrl` points to the `.Map.Gbx`;
//! - club campaign: `GET https://live-services.trackmania.nadeo.live/api/token/club/{club}/campaign/{campaign}`
//!   (NadeoLiveServices), maps in `campaign.playlist[].mapUid`.
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

/// A club campaign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClubCampaign {
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

    /// Maps of a club campaign, in playlist order.
    pub async fn club_campaign(
        &self,
        club_id: i64,
        campaign_id: i64,
    ) -> Result<ClubCampaign, IntegrationError> {
        let url = format!("{LIVE}/api/token/club/{club_id}/campaign/{campaign_id}");
        let response: CampaignResponse = self.get_json(Audience::NadeoLiveServices, &url).await?;
        let mut playlist = response.campaign.playlist;
        playlist.sort_by_key(|p| p.position);
        Ok(ClubCampaign {
            name: response.campaign.name,
            map_uids: playlist.into_iter().map(|p| p.map_uid).collect(),
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
