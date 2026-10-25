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
///
/// Serialized (logs, WebSocket) with its arguments masked when the command declares them
/// secret ([`CommandSpec::secret`]).
#[derive(Debug, Clone)]
pub struct Invocation {
    pub actor: Actor,
    pub role: Role,
    pub lang: Lang,
    /// Canonical command name (aliases resolved).
    pub name: String,
    pub args: Vec<String>,
    /// Everything after the command name, untouched (for free text such as kick reasons).
    pub raw_args: String,
    /// The arguments are secrets (a password): never logged.
    pub secret: bool,
    pub reply: ReplyTo,
}

impl Serialize for Invocation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct View<'a> {
            actor: &'a Actor,
            role: Role,
            lang: Lang,
            name: &'a str,
            args: Vec<&'a str>,
            raw_args: &'a str,
        }
        let (args, raw_args) = if self.secret && !self.args.is_empty() {
            (vec!["***"], "***")
        } else {
            (
                self.args.iter().map(String::as_str).collect(),
                self.raw_args.as_str(),
            )
        };
        View {
            actor: &self.actor,
            role: self.role,
            lang: self.lang,
            name: &self.name,
            args,
            raw_args,
        }
        .serialize(serializer)
    }
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
    /// Role needed for the command and every sub-command without its own role.
    pub min_role: Role,
    /// Sub-command (first argument, lowercase; `""` = no argument) → role needed, when it
    /// differs from `min_role`.
    pub sub_roles: BTreeMap<String, Role>,
    /// e.g. `/mode <name>`.
    pub usage: String,
    /// Translation key of the one-line description.
    pub description_key: String,
    /// Plugin that handles it.
    pub owner: String,
    /// The arguments are secrets, masked in logs and events.
    pub secret: bool,
}

impl CommandSpec {
    pub fn new(owner: &str, name: &str, min_role: Role) -> CommandSpec {
        CommandSpec {
            name: name.to_owned(),
            aliases: Vec::new(),
            min_role,
            sub_roles: BTreeMap::new(),
            usage: format!("/{name}"),
            description_key: format!("{owner}.cmd.{name}"),
            owner: owner.to_owned(),
            secret: false,
        }
    }

    /// Marks the arguments as secrets (masked in logs and events).
    pub fn secret(mut self) -> CommandSpec {
        self.secret = true;
        self
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

    /// Gives the sub-command `name` (first argument, `""` for none) its own role, checked by
    /// the registry.
    pub fn sub(mut self, name: &str, role: Role) -> CommandSpec {
        self.sub_roles.insert(name.to_ascii_lowercase(), role);
        self
    }

    /// Role needed to run the command with this first argument.
    pub fn role_for(&self, sub: Option<&str>) -> Role {
        self.sub_roles
            .get(&sub.unwrap_or_default().to_ascii_lowercase())
            .copied()
            .unwrap_or(self.min_role)
    }

    /// Lowest role that can run at least one form of the command.
    pub fn lowest_role(&self) -> Role {
        self.sub_roles
            .values()
            .copied()
            .fold(self.min_role, Role::min)
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

    /// Resolves a command for `role`, allowed if `role` can run at least one of its forms.
    pub fn resolve(&self, name: &str, role: Role) -> Resolution {
        self.resolve_with(name, role, CommandSpec::lowest_role)
    }

    /// Resolves a command line's name and first argument for `role`.
    pub fn resolve_invocation(&self, name: &str, sub: Option<&str>, role: Role) -> Resolution {
        self.resolve_with(name, role, |spec| spec.role_for(sub))
    }

    fn resolve_with(
        &self,
        name: &str,
        role: Role,
        needed: impl Fn(&CommandSpec) -> Role,
    ) -> Resolution {
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
        if role >= needed(spec) {
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
            .filter(|s| role >= s.lowest_role())
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
    match d
        .registry
        .resolve_invocation(&name, args.first().map(String::as_str), role)
    {
        Resolution::Allowed(spec) => d.bus.publish(crate::Event::Command {
            invocation: Invocation {
                actor,
                role,
                lang,
                name: spec.name,
                args,
                raw_args,
                secret: spec.secret,
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

    #[test]
    fn sub_commands_have_their_own_role() {
        let registry = CommandRegistry::default();
        registry
            .register(
                CommandSpec::new("cup", "cup", Role::Admin)
                    .sub("status", Role::Referee)
                    .sub("", Role::Referee)
                    .sub("join", Role::Player),
            )
            .unwrap();
        let allowed = |sub: Option<&str>, role| {
            matches!(
                registry.resolve_invocation("cup", sub, role),
                Resolution::Allowed(_)
            )
        };
        assert!(allowed(Some("STATUS"), Role::Referee));
        assert!(!allowed(Some("start"), Role::Referee));
        assert!(allowed(None, Role::Referee));
        assert!(!allowed(None, Role::Player));
        assert!(allowed(Some("start"), Role::Admin));
        assert!(allowed(Some("join"), Role::Player));
        assert!(!allowed(Some("status"), Role::Player));
        // Listed in /help for whoever can run one of its forms.
        assert_eq!(registry.available_to(Role::Player).len(), 1);
        assert!(matches!(
            registry.resolve("cup", Role::Player),
            Resolution::Allowed(_)
        ));
    }

    #[test]
    fn secret_arguments_are_masked_when_serialized() {
        let invocation = |secret| Invocation {
            actor: Actor::Console,
            role: Role::Admin,
            lang: Lang::En,
            name: "password".into(),
            args: vec!["hunter2".into()],
            raw_args: "hunter2".into(),
            secret,
            reply: ReplyTo::Nowhere,
        };
        let json = serde_json::to_string(&invocation(true)).unwrap();
        assert!(!json.contains("hunter2") && json.contains("***"), "{json}");
        let json = serde_json::to_string(&invocation(false)).unwrap();
        assert!(json.contains("hunter2"));
    }
}
