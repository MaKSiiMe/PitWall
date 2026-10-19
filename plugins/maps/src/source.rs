//! What an admin pastes to add maps: a TMX id or URL, a trackmania.io campaign URL or a map uid.

use pitwall_integrations::tmx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSource {
    Tmx(i64),
    /// `https://trackmania.io/#/campaigns/<club>/<campaign>`.
    Campaign {
        club: i64,
        campaign: i64,
    },
    Uid(String),
}

pub fn parse(input: &str) -> Option<MapSource> {
    let input = input.trim();
    if input.contains("trackmania.io") {
        let segments: Vec<&str> = input.trim_end_matches('/').split('/').collect();
        return segments.windows(3).find_map(|w| {
            (w[0] == "campaigns").then_some(())?;
            Some(MapSource::Campaign {
                club: w[1].parse().ok()?,
                campaign: w[2].parse().ok()?,
            })
        });
    }
    if let Some(id) = tmx::parse_id(input) {
        return Some(MapSource::Tmx(id));
    }
    // Map uids: about 27 characters of [A-Za-z0-9_-], never only digits (those are TMX ids).
    let uid_like = (20..=32).contains(&input.len())
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !input.chars().all(|c| c.is_ascii_digit());
    uid_like.then(|| MapSource::Uid(input.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_what_admins_paste() {
        assert_eq!(
            parse("https://trackmania.io/#/campaigns/120427/162056"),
            Some(MapSource::Campaign {
                club: 120_427,
                campaign: 162_056
            })
        );
        assert_eq!(parse("361306"), Some(MapSource::Tmx(361_306)));
        assert_eq!(
            parse("https://trackmania.exchange/mapshow/361306"),
            Some(MapSource::Tmx(361_306))
        );
        assert_eq!(
            parse("NHpsCuDk3pbxo5ci77q8ugvkVmj"),
            Some(MapSource::Uid("NHpsCuDk3pbxo5ci77q8ugvkVmj".into()))
        );
        assert_eq!(parse("https://trackmania.io/#/totd"), None);
        assert_eq!(parse("hello"), None);
    }
}
