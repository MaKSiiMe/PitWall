//! Plugin API: plugins receive events and send commands.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::chat::{CommandRegistry, CommandSpec, Invocation, ReplyTo};
use crate::command::{CommandHandle, Recipients};
use crate::event::{Actor, Event};
use crate::i18n::{Lang, Text, Translator};
use crate::roles::RoleBook;
use crate::state::StateHandle;

/// Boxed future returned by [`Plugin::run`].
pub type PluginFuture = Pin<Box<dyn Future<Output = Result<(), PluginError>> + Send>>;

/// Errors that stop a plugin.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("invalid plugin configuration: {0}")]
    Config(String),
    #[error("{0}")]
    Failed(String),
}

/// A controller extension.
pub trait Plugin: Send + 'static {
    /// Unique, stable name (also used as translation namespace and config section).
    fn name(&self) -> &'static str;

    /// Chat/console commands this plugin handles.
    fn commands(&self) -> Vec<CommandSpec> {
        Vec::new()
    }

    /// Translation catalogs `(lang, toml)`.
    fn translations(&self) -> Vec<(Lang, &'static str)> {
        Vec::new()
    }

    /// Runs until `ctx.shutdown` is cancelled or the event stream closes.
    fn run(self: Box<Self>, ctx: PluginContext) -> PluginFuture;
}

/// Receives events from the bus; tolerates lag.
#[derive(Debug)]
pub struct EventStream {
    rx: broadcast::Receiver<Event>,
    owner: String,
}

impl EventStream {
    pub(crate) fn new(rx: broadcast::Receiver<Event>, owner: &str) -> EventStream {
        EventStream {
            rx,
            owner: owner.to_owned(),
        }
    }

    /// Next event, or `None` when the bus is closed.
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(plugin = %self.owner, skipped, "plugin is too slow, events were dropped");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// Everything a plugin gets.
#[derive(Debug)]
pub struct PluginContext {
    pub name: &'static str,
    pub events: EventStream,
    pub commands: CommandHandle,
    pub state: StateHandle,
    pub roles: RoleBook,
    pub i18n: Translator,
    pub registry: CommandRegistry,
    pub bus: EventBus,
    pub shutdown: CancellationToken,
    /// This plugin's section of the configuration file (`[plugins.<name>]`), or an empty table.
    pub config: toml::Table,
    pub default_lang: Lang,
}

impl PluginContext {
    /// Next event, or `None` on shutdown / closed bus.
    pub async fn next_event(&mut self) -> Option<Event> {
        tokio::select! {
            event = self.events.recv() => event,
            () = self.shutdown.cancelled() => None,
        }
    }

    /// Deserializes this plugin's configuration section.
    pub fn config<T: serde::de::DeserializeOwned + Default>(&self) -> Result<T, PluginError> {
        if self.config.is_empty() {
            return Ok(T::default());
        }
        T::deserialize(toml::Value::Table(self.config.clone()))
            .map_err(|e| PluginError::Config(format!("[plugins.{}]: {e}", self.name)))
    }

    /// Answers a command where it came from, in the invoker's language.
    pub fn reply(&self, invocation: &Invocation, text: Text) {
        match &invocation.reply {
            ReplyTo::Player(login) => self.commands.chat_to(login, text),
            ReplyTo::Channel(tx) => {
                let _ = tx.send(self.i18n.render(invocation.lang, &text));
            }
            ReplyTo::Nowhere => {}
        }
    }

    /// Command handle acting on behalf of the invoker (for the audit log).
    pub fn commands_for(&self, invocation: &Invocation) -> CommandHandle {
        self.commands.as_actor(invocation.actor.clone())
    }

    /// Publishes a custom event from this plugin.
    pub fn emit(&self, name: &str, payload: serde_json::Value) {
        self.bus.publish(Event::custom(self.name, name, payload));
    }

    /// Chat message to a set of players.
    pub fn chat(&self, to: Recipients, text: Text) {
        self.commands
            .send(crate::command::Command::Chat { to, text });
    }
}

/// The broadcast bus. Cheap to clone.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> EventBus {
        let (tx, _) = broadcast::channel(capacity);
        EventBus { tx }
    }

    /// Sends an event to every subscriber (no-op without subscribers).
    pub fn publish(&self, event: Event) {
        tracing::trace!(kind = event.kind(), "event");
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self, owner: &str) -> EventStream {
        EventStream::new(self.tx.subscribe(), owner)
    }

    /// Raw receiver (for the API WebSocket).
    pub fn raw_subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

/// Actor of a plugin.
pub fn plugin_actor(name: &str) -> Actor {
    Actor::Plugin(name.to_owned())
}
