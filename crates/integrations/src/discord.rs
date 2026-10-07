//! Discord channel webhooks (announcements, results).

use serde::Serialize;

use crate::{IntegrationError, check};

/// An embed of a webhook message.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Embed {
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// RGB color.
    pub color: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Serialize)]
struct WebhookMessage<'a> {
    username: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    content: &'a str,
    #[serde(skip_serializing_if = "<[Embed]>::is_empty")]
    embeds: &'a [Embed],
}

/// Sends messages to one webhook URL.
#[derive(Debug, Clone)]
pub struct Webhook {
    http: reqwest::Client,
    url: String,
    username: String,
}

impl Webhook {
    pub fn new(
        http: reqwest::Client,
        url: &str,
        username: &str,
    ) -> Result<Webhook, IntegrationError> {
        if !url.starts_with("https://") {
            return Err(IntegrationError::MissingCredentials(
                "Discord webhook (https URL)",
            ));
        }
        Ok(Webhook {
            http,
            url: url.to_owned(),
            username: username.to_owned(),
        })
    }

    pub async fn send(&self, content: &str, embeds: &[Embed]) -> Result<(), IntegrationError> {
        let response = self
            .http
            .post(&self.url)
            .json(&WebhookMessage {
                username: &self.username,
                content,
                embeds,
            })
            .send()
            .await?;
        check("Discord", response).await?;
        Ok(())
    }
}

/// Removes Trackmania `$` formatting for Discord.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('$') => out.push('$'),
            Some(h) if h.is_ascii_hexdigit() => {
                for _ in 0..2 {
                    if chars.peek().is_some_and(char::is_ascii_hexdigit) {
                        chars.next();
                    }
                }
            }
            Some('l' | 'L' | 'h' | 'H' | 'p' | 'P') if chars.peek() == Some(&'[') => {
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    // Discord markdown: escape the characters that would format the text.
    out.replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace('~', "\\~")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_for_discord() {
        assert_eq!(plain("$f00Red$z $o*bold*"), "Red \\*bold\\*");
        assert_eq!(plain("a_b"), "a\\_b");
    }

    #[test]
    fn webhook_needs_https() {
        assert!(Webhook::new(reqwest::Client::new(), "http://x", "PitWall").is_err());
        assert!(
            Webhook::new(
                reqwest::Client::new(),
                "https://discord.com/api/webhooks/1/x",
                "PitWall"
            )
            .is_ok()
        );
    }

    #[test]
    fn message_shape() {
        let embeds = [Embed {
            title: "Mini cup".into(),
            color: 0x00aaff,
            fields: vec![EmbedField {
                name: "1.".into(),
                value: "Alice".into(),
                inline: true,
            }],
            ..Embed::default()
        }];
        let json = serde_json::to_value(WebhookMessage {
            username: "PitWall",
            content: "",
            embeds: &embeds,
        })
        .unwrap();
        assert!(json.get("content").is_none());
        assert_eq!(json["embeds"][0]["fields"][0]["value"], "Alice");
    }
}
