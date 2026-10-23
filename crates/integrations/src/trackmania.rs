//! Public Trackmania API with an OAuth application (client credentials, no user).
//!
//! - token: `POST https://api.trackmania.com/api/access_token`, form
//!   `grant_type=client_credentials&client_id=&client_secret=`, gives `access_token` and
//!   `expires_in` (about an hour), no refresh token;
//! - display names: `GET https://api.trackmania.com/api/display-names?accountId[]=…` (at most 50
//!   ids), `Authorization: Bearer <token>`, answers `{ "<account id>": "<name>" }`.
//!
//! Applications are created at <https://api.trackmania.com/manager>.
//! Source: <https://webservices.openplanet.dev/oauth/summary>.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{IntegrationError, check};

const API: &str = "https://api.trackmania.com/api";
/// Account ids per display-names request.
const NAMES_PER_REQUEST: usize = 50;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Debug)]
struct Token {
    access: String,
    expires: Instant,
}

/// Trackmania API client for an OAuth application.
#[derive(Debug)]
pub struct TrackmaniaApi {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    token: Mutex<Option<Token>>,
}

impl TrackmaniaApi {
    pub fn new(
        http: reqwest::Client,
        client_id: &str,
        client_secret: &str,
    ) -> Result<TrackmaniaApi, IntegrationError> {
        if client_id.is_empty() || client_secret.is_empty() {
            return Err(IntegrationError::MissingCredentials(
                "Trackmania API (OAuth application)",
            ));
        }
        Ok(TrackmaniaApi {
            http,
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            token: Mutex::new(None),
        })
    }

    async fn token(&self) -> Result<String, IntegrationError> {
        let mut guard = self.token.lock().await;
        if let Some(t) = guard.as_ref().filter(|t| Instant::now() < t.expires) {
            return Ok(t.access.clone());
        }
        let response = self
            .http
            .post(format!("{API}/access_token"))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
            ])
            .send()
            .await?;
        let token: TokenResponse = check("Trackmania API auth", response).await?.json().await?;
        // Renew a few minutes early.
        let lifetime = Duration::from_secs(token.expires_in.max(600) - 300);
        let access = token.access_token.clone();
        *guard = Some(Token {
            access: token.access_token,
            expires: Instant::now() + lifetime,
        });
        Ok(access)
    }

    /// Display names by account id. Unknown ids are missing from the result.
    pub async fn display_names(
        &self,
        account_ids: &[String],
    ) -> Result<HashMap<String, String>, IntegrationError> {
        let mut names = HashMap::new();
        for chunk in account_ids.chunks(NAMES_PER_REQUEST) {
            let token = self.token().await?;
            let query: Vec<(&str, &str)> = chunk.iter().map(|id| ("accountId[]", &**id)).collect();
            let response = self
                .http
                .get(format!("{API}/display-names"))
                .query(&query)
                .bearer_auth(token)
                .send()
                .await?;
            // Parsed loosely: anything but an object yields no names.
            let value: serde_json::Value = check("Trackmania API", response).await?.json().await?;
            names.extend(parse_names(value));
        }
        Ok(names)
    }
}

fn parse_names(value: serde_json::Value) -> HashMap<String, String> {
    match value {
        serde_json::Value::Object(map) => map
            .into_iter()
            .filter_map(|(id, name)| Some((id, name.as_str()?.to_owned())))
            .collect(),
        _ => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_display_names() {
        let names = parse_names(
            serde_json::from_str(r#"{"5b4d42f4-c2de-407d-b367-cbff3fe817bc":"tooInfinite"}"#)
                .unwrap(),
        );
        assert_eq!(
            names
                .get("5b4d42f4-c2de-407d-b367-cbff3fe817bc")
                .map(String::as_str),
            Some("tooInfinite")
        );
        assert!(parse_names(serde_json::json!([])).is_empty());
    }

    #[test]
    fn requires_credentials() {
        assert!(matches!(
            TrackmaniaApi::new(reqwest::Client::new(), "id", ""),
            Err(IntegrationError::MissingCredentials(_))
        ));
    }
}
