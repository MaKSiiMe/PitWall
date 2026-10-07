//! PitWall core: event bus, server state, roles, translations and the command API.
//!
//! Plugins receive [`Event`]s and send [`Command`]s through a [`PluginContext`]; they never touch
//! the GbxRemote socket. The [`Controller`] owns the connection, keeps the [`ServerState`] up to
//! date and executes commands.

pub mod builtin;
pub mod chat;
pub mod command;
pub mod event;
pub mod i18n;
pub mod matchsettings;
pub mod plugin;
pub mod roles;
pub mod runtime;
pub mod state;
pub mod tmtext;

pub use chat::{CommandRegistry, CommandSpec, Invocation, ReplyTo};
pub use command::{Command, CommandError, CommandHandle, ModeSwitch, Recipients, SpectatorModeDto};
pub use event::{Actor, AuditEntry, Event};
pub use i18n::{Lang, Text, Translator};
pub use plugin::{EventBus, Plugin, PluginContext, PluginError, PluginFuture, Replier};
pub use roles::{Permission, Role, RoleBook, RolesConfig};
pub use runtime::{Controller, ControllerOptions, CoreError, Handles, ServerConfig};
pub use state::{Player, ServerState, StateHandle, now_ms};

/// Re-export of the protocol crate for plugins.
pub use pitwall_gbx as gbx;
