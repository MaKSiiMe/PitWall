//! Trackmania Exchange (trackmania.exchange): map search, lookup and download.
//!
//! API v2: `GET /api/maps?fields=...&id=|uid=|name=&count=` returns `{ "More": bool, "Results": [...] }`;
//! `GET /maps/download/{id}` returns the `.Map.Gbx` file.

use serde::{Deserialize, Serialize};

use crate::{IntegrationError, check};

const FIELDS: &str = "MapId,MapUid,Name,GbxMapName,Uploader.Name,Length,Medals.Author,AwardCount";

/// A map on TMX.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TmxMap {
    pub map_id: i64,
    pub map_uid: String,
    pub name: String,
    #[serde(default)]
    pub gbx_map_name: String,
    #[serde(default)]
    pub uploader: TmxUser,
    #[serde(default)]
    pub length: i64,
    #[serde(default)]
    pub medals: TmxMedals,
    #[serde(default)]
    pub award_count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TmxUser {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TmxMedals {
    #[serde(default)]
    pub author: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SearchResponse {
    results: Vec<TmxMap>,
}

/// TMX client.
#[derive(Debug, Clone)]
pub struct Tmx {
    http: reqwest::Client,
    base: String,
}

impl Tmx {
    pub fn new(http: reqwest::Client) -> Tmx {
        Tmx {
            http,
            base: "https://trackmania.exchange".to_owned(),
        }
    }

    /// Alternative base URL (tests, mirrors).
    pub fn with_base(http: reqwest::Client, base: &str) -> Tmx {
        Tmx {
            http,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    async fn query(&self, params: &[(&str, &str)]) -> Result<Vec<TmxMap>, IntegrationError> {
        let mut query = vec![("fields", FIELDS)];
        query.extend_from_slice(params);
        let response = self
            .http
            .get(format!("{}/api/maps", self.base))
            .query(&query)
            .send()
            .await?;
        let response = check("TMX", response).await?;
        Ok(response.json::<SearchResponse>().await?.results)
    }

    /// Map by TMX id.
    pub async fn map(&self, id: i64) -> Result<TmxMap, IntegrationError> {
        let id = id.to_string();
        self.query(&[("id", &id)])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| IntegrationError::NotFound(format!("TMX map {id}")))
    }

    /// Map by Nadeo map uid.
    pub async fn map_by_uid(&self, uid: &str) -> Result<Option<TmxMap>, IntegrationError> {
        Ok(self.query(&[("uid", uid)]).await?.into_iter().next())
    }

    /// Search by name.
    pub async fn search(&self, name: &str, count: u32) -> Result<Vec<TmxMap>, IntegrationError> {
        let count = count.clamp(1, 100).to_string();
        self.query(&[("name", name), ("count", &count)]).await
    }

    /// Downloads the `.Map.Gbx` file.
    pub async fn download(&self, id: i64) -> Result<Vec<u8>, IntegrationError> {
        let response = self
            .http
            .get(format!("{}/maps/download/{id}", self.base))
            .send()
            .await?;
        let response = check("TMX", response).await?;
        Ok(response.bytes().await?.to_vec())
    }
}

/// Extracts a TMX id from `123`, `https://trackmania.exchange/mapshow/123` or `.../maps/123/name`.
pub fn parse_id(input: &str) -> Option<i64> {
    let input = input.trim().trim_end_matches('/');
    if let Ok(id) = input.parse() {
        return Some(id);
    }
    let segments: Vec<&str> = input.split('/').collect();
    segments
        .windows(2)
        .filter(|w| matches!(w[0], "mapshow" | "maps" | "s" | "download"))
        .find_map(|w| w[1].parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ids_and_urls() {
        assert_eq!(parse_id("361306"), Some(361_306));
        assert_eq!(
            parse_id("https://trackmania.exchange/mapshow/361306"),
            Some(361_306)
        );
        assert_eq!(
            parse_id("https://trackmania.exchange/maps/361306/purple-winter"),
            Some(361_306)
        );
        assert_eq!(
            parse_id("https://trackmania.exchange/maps/download/42/"),
            Some(42)
        );
        assert_eq!(parse_id("hello"), None);
    }

    #[test]
    fn decodes_search_response() {
        // Shape observed on the live API.
        let json = r#"{"More":true,"Results":[{"MapId":361306,"MapUid":"1P2SbFJYHD09WPoKvmCJu28r2Pl","Name":"purple winter","GbxMapName":"purple winter","Uploader":{"Name":"xcvz"},"Length":31593,"Medals":{"Author":31593},"AwardCount":0}]}"#;
        let response: SearchResponse = serde_json::from_str(json).unwrap();
        let map = &response.results[0];
        assert_eq!(map.map_id, 361_306);
        assert_eq!(map.uploader.name, "xcvz");
        assert_eq!(map.medals.author, 31_593);
    }
}
