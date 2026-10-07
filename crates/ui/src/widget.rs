//! Widget identifiers.

use serde::{Deserialize, Serialize};

/// Every PitWall widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetId {
    /// Live ranking of the match.
    Ranking,
    /// Checkpoint difference against the reference run.
    CpDiff,
    /// Cup name, phase, status and ready button.
    CupInfo,
    /// Best times on the map.
    Records,
    /// Buttons for referees and admins.
    AdminPanel,
    /// Toggles to show/hide the other widgets.
    Menu,
}

impl WidgetId {
    pub const ALL: [WidgetId; 6] = [
        WidgetId::Ranking,
        WidgetId::CpDiff,
        WidgetId::CupInfo,
        WidgetId::Records,
        WidgetId::AdminPanel,
        WidgetId::Menu,
    ];

    /// Widgets players can hide from the menu.
    pub const HIDEABLE: [WidgetId; 5] = [
        WidgetId::Ranking,
        WidgetId::CpDiff,
        WidgetId::CupInfo,
        WidgetId::Records,
        WidgetId::AdminPanel,
    ];

    pub fn key(self) -> &'static str {
        match self {
            WidgetId::Ranking => "ranking",
            WidgetId::CpDiff => "cp_diff",
            WidgetId::CupInfo => "cup_info",
            WidgetId::Records => "records",
            WidgetId::AdminPanel => "admin_panel",
            WidgetId::Menu => "menu",
        }
    }

    pub fn parse(key: &str) -> Option<WidgetId> {
        WidgetId::ALL.into_iter().find(|w| w.key() == key)
    }

    /// Manialink id of the view layer.
    pub fn view_id(self) -> String {
        format!("PitWall_{}", self.camel())
    }

    /// Manialink id of the data layer.
    pub fn data_id(self) -> String {
        format!("PitWall_{}_Data", self.camel())
    }

    /// Prefix of the `LocalUser` variables shared between the two layers.
    pub fn var_prefix(self) -> String {
        format!("PitWall_{}", self.camel())
    }

    fn camel(self) -> &'static str {
        match self {
            WidgetId::Ranking => "Ranking",
            WidgetId::CpDiff => "CpDiff",
            WidgetId::CupInfo => "CupInfo",
            WidgetId::Records => "Records",
            WidgetId::AdminPanel => "AdminPanel",
            WidgetId::Menu => "Menu",
        }
    }

    /// Has a data layer (the admin panel and the menu are static views).
    pub fn has_data(self) -> bool {
        !matches!(self, WidgetId::AdminPanel | WidgetId::Menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for w in WidgetId::ALL {
            assert_eq!(WidgetId::parse(w.key()), Some(w));
        }
        assert_eq!(WidgetId::CpDiff.view_id(), "PitWall_CpDiff");
        assert_eq!(WidgetId::CpDiff.data_id(), "PitWall_CpDiff_Data");
    }
}
