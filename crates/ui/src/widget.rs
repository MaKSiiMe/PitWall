//! Widget identifiers.

use serde::{Deserialize, Serialize};

/// Every PitWall widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetId {
    /// Live ranking of the match.
    Ranking,
    /// Cup name, phase, status and ready button.
    CupInfo,
    /// Best times on the map.
    Records,
    /// World top of the map and the player's world position (Nadeo leaderboards).
    WorldRecords,
    /// Buttons for referees and admins.
    AdminPanel,
    /// Toggles to show/hide the other widgets.
    Menu,
    /// Full ranking on the scores table layer (Tab, end of map), replacing Nadeo's.
    Scoreboard,
}

impl WidgetId {
    pub const ALL: [WidgetId; 7] = [
        WidgetId::Ranking,
        WidgetId::CupInfo,
        WidgetId::Records,
        WidgetId::WorldRecords,
        WidgetId::AdminPanel,
        WidgetId::Menu,
        WidgetId::Scoreboard,
    ];

    /// Widgets players can hide from the menu.
    pub const HIDEABLE: [WidgetId; 5] = [
        WidgetId::Ranking,
        WidgetId::CupInfo,
        WidgetId::Records,
        WidgetId::WorldRecords,
        WidgetId::AdminPanel,
    ];

    pub fn key(self) -> &'static str {
        match self {
            WidgetId::Ranking => "ranking",
            WidgetId::CupInfo => "cup_info",
            WidgetId::Records => "records",
            WidgetId::WorldRecords => "world_records",
            WidgetId::AdminPanel => "admin_panel",
            WidgetId::Menu => "menu",
            WidgetId::Scoreboard => "scoreboard",
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
            WidgetId::CupInfo => "CupInfo",
            WidgetId::Records => "Records",
            WidgetId::WorldRecords => "WorldRecords",
            WidgetId::AdminPanel => "AdminPanel",
            WidgetId::Menu => "Menu",
            WidgetId::Scoreboard => "Scoreboard",
        }
    }

    /// `LocalUser` variable that hides this widget on the client (set by the menu).
    pub fn hidden_var(self) -> String {
        format!("{}_Hidden", self.var_prefix())
    }

    /// Hidden from the menu's flags (the scores table is shown by the game on Tab).
    pub fn hideable(self) -> bool {
        WidgetId::HIDEABLE.contains(&self)
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
        assert_eq!(WidgetId::CupInfo.view_id(), "PitWall_CupInfo");
        assert_eq!(WidgetId::CupInfo.data_id(), "PitWall_CupInfo_Data");
        assert_eq!(WidgetId::Records.hidden_var(), "PitWall_Records_Hidden");
    }
}
