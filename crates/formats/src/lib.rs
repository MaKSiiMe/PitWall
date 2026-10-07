//! Declarative game formats and the cup engine.
//!
//! - [`format`]: a format = mode script + settings + points + maps, described in TOML;
//!   [`FormatRegistry::apply_format`] turns it into a [`pitwall_core::ModeSwitch`].
//! - [`mode`]: official mode scripts and their short names.

pub mod format;
pub mod mode;

pub use format::{FormatDef, FormatError, FormatRegistry, Localized};
pub use mode::{KNOWN_MODES, KnownMode, find_mode, resolve_script};
