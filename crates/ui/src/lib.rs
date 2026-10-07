//! In-game UI: Manialink pages rendered from askama templates, animated by ManiaScript.
//!
//! Every widget is made of two layers:
//! - the **view** (`PitWall_<Widget>`): layout, localized labels and a ManiaScript loop. Sent once
//!   per player (on join, language or preference change);
//! - the **data** (`PitWall_<Widget>_Data`): a tiny page holding JSON in a `data-json` attribute.
//!   Its script stores the JSON in a variable shared through `LocalUser` and bumps a serial; the
//!   view notices, parses it with `fromjson` and animates the changes (rows slide to their new
//!   rank, new rows slide in). Updates are therefore small and targeted, and the view is never
//!   rebuilt during a race.

pub mod model;
pub mod render;
pub mod theme;
pub mod widget;

pub use render::{RenderError, Renderer};
pub use theme::{Layout, Position, Theme};
pub use widget::WidgetId;

/// Translation catalogs of the UI (`ui.*` keys).
pub fn catalogs() -> [(pitwall_core::Lang, &'static str); 2] {
    [
        (pitwall_core::Lang::En, include_str!("../locales/en.toml")),
        (pitwall_core::Lang::Fr, include_str!("../locales/fr.toml")),
    ]
}

#[cfg(test)]
mod tests;
