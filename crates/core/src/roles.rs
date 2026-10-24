//! Fixed roles and the permissions they grant.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// Roles, ordered from least to most privileged.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Player,
    /// Streamers/commentators: read-only admin views.
    Caster,
    /// Runs matches: pause, scores, placement, kick, mute.
    Referee,
    /// Runs the server: formats, cups, maps, bans.
    Admin,
    /// Everything, including role management.
    SuperAdmin,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        Some(match s.to_ascii_lowercase().as_str() {
            "player" => Role::Player,
            "caster" => Role::Caster,
            "referee" | "ref" => Role::Referee,
            "admin" => Role::Admin,
            "superadmin" | "super_admin" => Role::SuperAdmin,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Player => "player",
            Role::Caster => "caster",
            Role::Referee => "referee",
            Role::Admin => "admin",
            Role::SuperAdmin => "superadmin",
        }
    }

    /// `true` if this role grants `permission`.
    pub fn can(self, permission: Permission) -> bool {
        self >= permission.min_role()
    }
}

/// Actions guarded by a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// See admin widgets and live data meant for staff.
    ViewStaffUi,
    /// Pause/resume, end round, restart map.
    ControlMatch,
    /// Correct points, disqualify, reinstate.
    EditScores,
    /// Force spectator, force team, placement.
    ForcePlacement,
    Kick,
    Mute,
    Ban,
    /// Switch formats and mode settings.
    ManageFormat,
    /// Create, start and drive cups.
    ManageCup,
    /// Add/remove maps.
    ManageMaps,
    /// Grant and revoke roles.
    ManageRoles,
}

impl Permission {
    pub fn min_role(self) -> Role {
        match self {
            Permission::ViewStaffUi => Role::Caster,
            Permission::ControlMatch
            | Permission::EditScores
            | Permission::ForcePlacement
            | Permission::Kick
            | Permission::Mute => Role::Referee,
            Permission::Ban
            | Permission::ManageFormat
            | Permission::ManageCup
            | Permission::ManageMaps => Role::Admin,
            Permission::ManageRoles => Role::SuperAdmin,
        }
    }
}

/// Logins per role, as written in the configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RolesConfig {
    pub superadmins: Vec<String>,
    pub admins: Vec<String>,
    pub referees: Vec<String>,
    pub casters: Vec<String>,
    /// Trackmania club ids whose members are admins (needs the Nadeo API).
    pub admin_clubs: Vec<i64>,
}

/// Login → role table, shared and mutable at runtime.
///
/// Two layers: roles set by the configuration or `/role`, and roles granted by a source that
/// refreshes them as a whole (club membership). A login gets the higher of the two.
#[derive(Debug, Clone, Default)]
pub struct RoleBook {
    roles: Arc<RwLock<HashMap<String, Role>>>,
    granted: Arc<RwLock<HashMap<String, Role>>>,
}

impl RoleBook {
    pub fn from_config(config: &RolesConfig) -> RoleBook {
        let book = RoleBook::default();
        // Lowest first so that the highest role wins for duplicated logins.
        for (logins, role) in [
            (&config.casters, Role::Caster),
            (&config.referees, Role::Referee),
            (&config.admins, Role::Admin),
            (&config.superadmins, Role::SuperAdmin),
        ] {
            for login in logins {
                book.set(login, role);
            }
        }
        book
    }

    pub fn get(&self, login: &str) -> Role {
        let set = self
            .roles
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(login)
            .copied()
            .unwrap_or_default();
        let granted = self
            .granted
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(login)
            .copied()
            .unwrap_or_default();
        set.max(granted)
    }

    /// Replaces the granted layer. Returns the logins whose role changed.
    pub fn set_granted(&self, granted: HashMap<String, Role>) -> Vec<String> {
        let before: HashMap<String, Role> = {
            let current = self
                .granted
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            current
                .keys()
                .chain(granted.keys())
                .map(|login| (login.clone(), self.get(login)))
                .collect()
        };
        *self
            .granted
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = granted;
        let mut changed: Vec<String> = before
            .into_iter()
            .filter(|(login, role)| self.get(login) != *role)
            .map(|(login, _)| login)
            .collect();
        changed.sort();
        changed
    }

    pub fn set(&self, login: &str, role: Role) {
        let mut roles = self
            .roles
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if role == Role::Player {
            roles.remove(login);
        } else {
            roles.insert(login.to_owned(), role);
        }
    }

    /// All logins with a role above `Player`.
    pub fn staff(&self) -> Vec<(String, Role)> {
        let mut logins: Vec<String> = [&self.roles, &self.granted]
            .iter()
            .flat_map(|layer| {
                layer
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        logins.sort();
        logins.dedup();
        logins
            .into_iter()
            .map(|login| {
                let role = self.get(&login);
                (login, role)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_ordered() {
        assert!(Role::SuperAdmin > Role::Admin);
        assert!(Role::Referee.can(Permission::Kick));
        assert!(!Role::Referee.can(Permission::Ban));
        assert!(!Role::Caster.can(Permission::ControlMatch));
        assert!(Role::Caster.can(Permission::ViewStaffUi));
        assert!(Role::SuperAdmin.can(Permission::ManageRoles));
    }

    #[test]
    fn highest_configured_role_wins() {
        let book = RoleBook::from_config(&RolesConfig {
            superadmins: vec!["boss".into()],
            admins: vec!["boss".into(), "adm".into()],
            referees: vec![],
            casters: vec!["cast".into()],
            admin_clubs: vec![],
        });
        assert_eq!(book.get("boss"), Role::SuperAdmin);
        assert_eq!(book.get("adm"), Role::Admin);
        assert_eq!(book.get("cast"), Role::Caster);
        assert_eq!(book.get("random"), Role::Player);
        book.set("adm", Role::Player);
        assert_eq!(book.get("adm"), Role::Player);
    }

    #[test]
    fn granted_roles_add_to_the_configured_ones() {
        let book = RoleBook::from_config(&RolesConfig {
            superadmins: vec!["boss".into()],
            referees: vec!["ref".into()],
            ..RolesConfig::default()
        });
        let club = |logins: &[&str]| {
            logins
                .iter()
                .map(|l| ((*l).to_owned(), Role::Admin))
                .collect::<HashMap<_, _>>()
        };
        let changed = book.set_granted(club(&["boss", "ref", "friend"]));
        assert_eq!(changed, ["friend", "ref"]);
        assert_eq!(book.get("boss"), Role::SuperAdmin);
        assert_eq!(book.get("ref"), Role::Admin);
        assert_eq!(book.get("friend"), Role::Admin);
        // "friend" left the club, "ref" goes back to their configured role.
        let changed = book.set_granted(club(&["boss"]));
        assert_eq!(changed, ["friend", "ref"]);
        assert_eq!(book.get("friend"), Role::Player);
        assert_eq!(book.get("ref"), Role::Referee);
        assert_eq!(
            book.staff(),
            [
                ("boss".to_owned(), Role::SuperAdmin),
                ("ref".to_owned(), Role::Referee)
            ]
        );
    }
}
