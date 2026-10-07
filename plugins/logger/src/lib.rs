//! Logs every event of the bus. Useful to watch what the server sends while developing.
//!
//! Configuration (`[plugins.logger]`):
//! - `raw_callbacks = true` also logs every raw callback (very verbose).
//! - `skip = ["checkpoint"]` event kinds not to log.

use pitwall_core::{Event, Plugin, PluginContext, PluginFuture};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Config {
    raw_callbacks: bool,
    skip: Vec<String>,
}

/// The logger plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoggerPlugin;

impl Plugin for LoggerPlugin {
    fn name(&self) -> &'static str {
        "logger"
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let config: Config = ctx.config()?;
            while let Some(event) = ctx.next_event().await {
                let kind = event.kind();
                if config.skip.iter().any(|s| s == kind) {
                    continue;
                }
                if let Event::Callback { callback } = &event {
                    if config.raw_callbacks {
                        tracing::debug!(target: "pitwall::events", ?callback, "callback");
                    }
                    continue;
                }
                let payload = serde_json::to_string(&event).unwrap_or_default();
                tracing::info!(target: "pitwall::events", kind, %payload);
            }
            Ok(())
        })
    }
}
