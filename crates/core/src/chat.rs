//! Commands typed in chat (`/mode cup`), in the console, on the web dashboard or on Discord.
//!
//! Plugins declare [`CommandSpec`]s; the runtime parses the input, checks the role and broadcasts
//! an [`crate::Event::Command`] that the owning plugin handles.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use tokio::sync::mpsc;

use crate::event::Actor;
use crate::i18n::Lang;
use crate::roles::Role;

/// Where to send the answer of a command.
#[derive(Debug, Clone, Default)]
pub enum ReplyTo {
    /// Chat message to this login.
    Player(String),
    /// Text lines to a console/web/Discord session.
    Channel(mpsc::UnboundedSender<String>),
    #[default]
    Nowhere,
}

/// A command ready to be executed by its plugin.
#[derive(Debug, Clone, Serialize)]
pub struct Invocation {
    pub actor: Actor,
    pub role: Role,
    pub lang: Lang,
    /// Canonical command name (aliases resolved).
    pub name: String,
    pub args: Vec<String>,
    /// Everything after the command name, untouched (for free text such as kick reasons).
    pub raw_args: String,
    #[serde(skip)]
    pub reply: ReplyTo,
}

impl Invocation {
    /// Login of the invoking player, if any.
    pub fn login(&self) -> Option<&str> {
        match &self.actor {
            Actor::Player(login) => Some(login),
            _ => None,
        }
    }

    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args.get(index).map(String::as_str)
    }

    /// Arguments from `index` joined back with spaces.
    pub fn rest(&self, index: usize) -> String {
        self.args
            .get(index..)
            .map(|a| a.join(" "))
            .unwrap_or_default()
    }
}

/// Declaration of a command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    pub name: String,
    pub aliases: Vec<String>,
    pub min_role: Role,
    /// e.g. `/mode <name>`.
    pub usage: String,
    /// Translation key of the one-line description.
    pub description_key: String,
    /// Plugin that handles it.
    pub owner: String,
}

impl CommandSpec {
    pub fn new(owner: &str, name: &str, min_role: Role) -> CommandSpec {
        CommandSpec {
            name: name.to_owned(),
            aliases: Vec::new(),
            min_role,
            usage: format!("/{name}"),
            description_key: format!("{owner}.cmd.{name}"),
            owner: owner.to_owned(),
        }
    }

    pub fn alias(mut self, alias: &str) -> CommandSpec {
        self.aliases.push(alias.to_owned());
        self
    }

    pub fn usage(mut self, usage: &str) -> CommandSpec {
        self.usage = usage.to_owned();
        self
    }

    pub fn description(mut self, key: &str) -> CommandSpec {
        self.description_key = key.to_owned();
        self
    }
}

/// Errors when registering commands.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("command /{name} is already registered by {owner}")]
    Duplicate { name: String, owner: String },
}

/// Outcome of resolving a command for a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Allowed(CommandSpec),
    Denied(CommandSpec),
    Unknown,
}

/// All registered commands.
#[derive(Debug, Clone, Default)]
pub struct CommandRegistry {
    inner: Arc<RwLock<RegistryInner>>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    specs: BTreeMap<String, CommandSpec>,
    /// alias or name → canonical name
    names: BTreeMap<String, String>,
}

impl CommandRegistry {
    pub fn register(&self, spec: CommandSpec) -> Result<(), RegistryError> {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys: Vec<String> = std::iter::once(&spec.name)
            .chain(&spec.aliases)
            .map(|n| n.to_ascii_lowercase())
            .collect();
        for key in &keys {
            if let Some(existing) = inner.names.get(key) {
                let owner = inner
                    .specs
                    .get(existing)
                    .map(|s| s.owner.clone())
                    .unwrap_or_default();
                return Err(RegistryError::Duplicate {
                    name: key.clone(),
                    owner,
                });
            }
        }
        let canonical = spec.name.to_ascii_lowercase();
        for key in keys {
            inner.names.insert(key, canonical.clone());
        }
        inner.specs.insert(canonical, spec);
        Ok(())
    }

    pub fn resolve(&self, name: &str, role: Role) -> Resolution {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(spec) = inner
            .names
            .get(&name.to_ascii_lowercase())
            .and_then(|canonical| inner.specs.get(canonical))
        else {
            return Resolution::Unknown;
        };
        if role >= spec.min_role {
            Resolution::Allowed(spec.clone())
        } else {
            Resolution::Denied(spec.clone())
        }
    }

    /// Commands usable by `role`, sorted by name.
    pub fn available_to(&self, role: Role) -> Vec<CommandSpec> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .specs
            .values()
            .filter(|s| role >= s.min_role)
            .cloned()
            .collect()
    }
}

/// Commands sent automatically by client-side plugins (Openplanet), answered by nobody:
/// `/chatformat` is how Better Chat asks the controller for its chat format.
pub const IGNORED_COMMANDS: &[&str] = &["chatformat"];

/// What [`dispatch`] needs.
#[derive(Debug, Clone, Copy)]
pub struct Dispatcher<'a> {
    pub registry: &'a CommandRegistry,
    pub bus: &'a crate::plugin::EventBus,
    pub i18n: &'a crate::i18n::Translator,
    /// Used to answer players (unknown command, permission denied).
    pub commands: &'a crate::command::CommandHandle,
}

/// Parses a command line (leading `/` optional), checks the role and publishes an
/// [`crate::Event::Command`], or answers the invoker with an error.
pub fn dispatch(
    d: &Dispatcher<'_>,
    actor: Actor,
    role: Role,
    lang: Lang,
    line: &str,
    reply: ReplyTo,
) {
    use crate::i18n::Text;
    let line = line.trim();
    let line = if line.starts_with('/') {
        line.to_owned()
    } else {
        format!("/{line}")
    };
    let Some((name, args, raw_args)) = parse_command_line(&line) else {
        return;
    };
    let say = |text: Text| match &reply {
        ReplyTo::Player(login) => d.commands.chat_to(login, text),
        ReplyTo::Channel(tx) => {
            let _ = tx.send(d.i18n.render(lang, &text));
        }
        ReplyTo::Nowhere => {}
    };
    match d.registry.resolve(&name, role) {
        Resolution::Allowed(spec) => d.bus.publish(crate::Event::Command {
            invocation: Invocation {
                actor,
                role,
                lang,
                name: spec.name,
                args,
                raw_args,
                reply,
            },
        }),
        Resolution::Denied(spec) => say(Text::tr("core.permission_denied").arg("name", spec.name)),
        Resolution::Unknown if IGNORED_COMMANDS.contains(&name.as_str()) => {
            tracing::debug!(%name, "ignoring a client plugin command");
        }
        Resolution::Unknown => say(Text::tr("core.unknown_command").arg("name", name)),
    }
}

/// Splits `/name arg1 arg2` into `(name, args, raw_args)`. Accepts `//name` too.
/// Double quotes group words: `/kick bob "afk too long"`.
pub fn parse_command_line(text: &str) -> Option<(String, Vec<String>, String)> {
    let text = text.trim();
    let body = text.strip_prefix("//").or_else(|| text.strip_prefix('/'))?;
    let (name, raw) = match body.split_once(char::is_whitespace) {
        Some((name, rest)) => (name, rest.trim()),
        None => (body, ""),
    };
    if name.is_empty() {
        return None;
    }
    Some((name.to_ascii_lowercase(), split_args(raw), raw.to_owned()))
}

fn split_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_token = false;
    for c in raw.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has_token = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_lines() {
        assert_eq!(
            parse_command_line("/mode Cup"),
            Some(("mode".into(), vec!["Cup".into()], "Cup".into()))
        );
        assert_eq!(
            parse_command_line("//kick bob \"afk too long\""),
            Some((
                "kick".into(),
                vec!["bob".into(), "afk too long".into()],
                "bob \"afk too long\"".into()
            ))
        );
        assert_eq!(
            parse_command_line("/help"),
            Some(("help".into(), vec![], String::new()))
        );
        assert_eq!(parse_command_line("hello /mode"), None);
        assert_eq!(parse_command_line("/"), None);
        assert_eq!(
            parse_command_line("/say \"\""),
            Some(("say".into(), vec![String::new()], "\"\"".into()))
        );
    }

    #[test]
    fn registry_resolves_aliases_and_roles() {
        let registry = CommandRegistry::default();
        registry
            .register(CommandSpec::new("formats", "mode", Role::Admin).alias("format"))
            .unwrap();
        registry
            .register(CommandSpec::new("core", "help", Role::Player))
            .unwrap();

        assert!(
            matches!(registry.resolve("FORMAT", Role::Admin), Resolution::Allowed(s) if s.name == "mode")
        );
        assert!(matches!(
            registry.resolve("mode", Role::Referee),
            Resolution::Denied(_)
        ));
        assert_eq!(
            registry.resolve("nope", Role::SuperAdmin),
            Resolution::Unknown
        );
        assert_eq!(registry.available_to(Role::Player).len(), 1);

        let dup = registry.register(CommandSpec::new("other", "format", Role::Player));
        assert!(matches!(dup, Err(RegistryError::Duplicate { owner, .. }) if owner == "formats"));
    }
}
