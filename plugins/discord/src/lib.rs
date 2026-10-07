//! Posts cup announcements to a Discord channel webhook: start, phase results, final podium.

use pitwall_core::{Event, Plugin, PluginContext, PluginError, PluginFuture};
use pitwall_formats::cup::{CupSnapshot, CupStatus, PhaseResult, Standing};
use pitwall_integrations::discord::{Embed, EmbedField, Webhook, plain};
use pitwall_integrations::export::format_time;

const ACCENT: u32 = 0x00aaff;
const GOLD: u32 = 0xffcc00;

/// The Discord plugin.
#[derive(Debug, Clone)]
pub struct DiscordPlugin {
    webhook: Webhook,
}

impl DiscordPlugin {
    pub fn new(webhook: Webhook) -> DiscordPlugin {
        DiscordPlugin { webhook }
    }
}

fn standings_fields(standings: &[Standing], limit: usize) -> Vec<EmbedField> {
    standings
        .iter()
        .take(limit)
        .map(|s| EmbedField {
            name: format!("{}.", s.rank),
            value: match s.best_time {
                Some(t) if s.points == 0 => format!("{} — {}", plain(&s.name), format_time(t)),
                _ => format!("{} — {} pts", plain(&s.name), s.points),
            },
            inline: false,
        })
        .collect()
}

impl Plugin for DiscordPlugin {
    fn name(&self) -> &'static str {
        "discord"
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let mut cup: Option<CupSnapshot> = None;
            while let Some(event) = ctx.next_event().await {
                let Event::Custom {
                    source,
                    name,
                    payload,
                } = event
                else {
                    continue;
                };
                if source != "cup" {
                    continue;
                }
                let message = match name.as_str() {
                    "state" => {
                        let snapshot: Option<CupSnapshot> =
                            serde_json::from_value(payload).unwrap_or(None);
                        let started = match (&cup, &snapshot) {
                            (Some(before), Some(now)) => {
                                matches!(before.status, CupStatus::Draft | CupStatus::Registration)
                                    && !matches!(
                                        now.status,
                                        CupStatus::Draft | CupStatus::Registration
                                    )
                            }
                            _ => false,
                        };
                        let opened = match (&cup, &snapshot) {
                            (Some(before), Some(now)) => {
                                before.status == CupStatus::Draft
                                    && now.status == CupStatus::Registration
                            }
                            _ => false,
                        };
                        cup = snapshot;
                        let Some(current) = &cup else { continue };
                        let name = if current.name.en.is_empty() {
                            &current.id
                        } else {
                            &current.name.en
                        };
                        if started {
                            Some(Embed {
                                title: format!("🏁 {} starts!", plain(name)),
                                description: format!("{} participants", current.participants.len()),
                                color: ACCENT,
                                fields: Vec::new(),
                            })
                        } else if opened {
                            Some(Embed {
                                title: format!("📝 Registrations open: {}", plain(name)),
                                description: String::new(),
                                color: ACCENT,
                                fields: Vec::new(),
                            })
                        } else {
                            None
                        }
                    }
                    "phase_completed" => {
                        let result: Option<PhaseResult> =
                            serde_json::from_value(payload["result"].clone()).ok();
                        result.map(|r| Embed {
                            title: format!("Results — {}", r.phase_id),
                            description: format!("{} qualified", r.qualified.len()),
                            color: ACCENT,
                            fields: standings_fields(&r.standings, 10),
                        })
                    }
                    "finished" => {
                        let standings: Vec<Standing> =
                            serde_json::from_value(payload["standings"].clone())
                                .unwrap_or_default();
                        let name = cup
                            .as_ref()
                            .map(|c| {
                                if c.name.en.is_empty() {
                                    c.id.clone()
                                } else {
                                    c.name.en.clone()
                                }
                            })
                            .unwrap_or_default();
                        Some(Embed {
                            title: format!("🏆 {} — final podium", plain(&name)),
                            description: String::new(),
                            color: GOLD,
                            fields: standings_fields(&standings, 3),
                        })
                    }
                    _ => None,
                };
                if let Some(embed) = message {
                    let webhook = self.webhook.clone();
                    tokio::spawn(async move {
                        if let Err(err) = webhook.send("", &[embed]).await {
                            tracing::warn!(%err, "Discord webhook failed");
                        }
                    });
                }
            }
            Ok::<(), PluginError>(())
        })
    }
}
