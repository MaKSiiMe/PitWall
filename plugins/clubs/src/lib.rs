//! Roles from Trackmania club membership: members of the clubs in `[roles] admin_clubs` are
//! admins. The member lists come from the Nadeo API (dedicated server account), at startup,
//! every few minutes and when an unknown player joins; a failed refresh keeps the last lists.
//! Leaving the club removes the role at the next refresh. Configured roles still apply: a login
//! gets the higher of its configured role and its club role.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pitwall_core::{Event, Plugin, PluginContext, PluginError, PluginFuture, Role};
use pitwall_integrations::nadeo::{Nadeo, login_of_account_id};

/// Time between two refreshes of the member lists.
const REFRESH: Duration = Duration::from_secs(5 * 60);
/// A joining player who is not staff triggers a refresh, at most this often.
const MIN_REFRESH: Duration = Duration::from_secs(60);

/// The clubs plugin.
pub struct ClubsPlugin {
    nadeo: Arc<Nadeo>,
    admin_clubs: Vec<i64>,
}

impl std::fmt::Debug for ClubsPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClubsPlugin")
            .field("admin_clubs", &self.admin_clubs)
            .finish_non_exhaustive()
    }
}

impl ClubsPlugin {
    pub fn new(nadeo: Arc<Nadeo>, admin_clubs: Vec<i64>) -> ClubsPlugin {
        ClubsPlugin { nadeo, admin_clubs }
    }

    /// Logins of every member, or `None` if a club could not be read.
    async fn members(&self) -> Option<HashMap<String, Role>> {
        let mut granted = HashMap::new();
        for &club in &self.admin_clubs {
            match self.nadeo.club_members(club).await {
                Ok(members) => {
                    if members.is_empty() {
                        tracing::warn!(club, "club without members (wrong id?)");
                    }
                    for account in members {
                        if let Some(login) = login_of_account_id(&account) {
                            granted.insert(login, Role::Admin);
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(club, %err, "cannot read the club members, roles unchanged");
                    return None;
                }
            }
        }
        Some(granted)
    }

    async fn refresh(&self, ctx: &PluginContext) {
        let Some(granted) = self.members().await else {
            return;
        };
        let members = granted.len();
        let changed = ctx.roles.set_granted(granted);
        tracing::info!(
            clubs = ?self.admin_clubs,
            members,
            changed = changed.len(),
            "club roles refreshed"
        );
        for login in changed {
            let role = ctx.roles.get(&login);
            tracing::info!(login, role = role.as_str(), "role from club membership");
            let player = ctx.state.write(|s| {
                let p = s.players.get_mut(&login)?;
                p.role = role;
                Some(p.clone())
            });
            if let Some(player) = player {
                ctx.bus.publish(Event::PlayerUpdated { player });
            }
        }
    }
}

impl Plugin for ClubsPlugin {
    fn name(&self) -> &'static str {
        "clubs"
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            if self.admin_clubs.is_empty() {
                return Ok::<(), PluginError>(());
            }
            let mut tick = tokio::time::interval(REFRESH);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last: Option<Instant> = None;
            loop {
                tokio::select! {
                    event = ctx.next_event() => match event {
                        Some(Event::PlayerJoined { player })
                            if player.role < Role::Admin
                                && last.is_none_or(|t| t.elapsed() >= MIN_REFRESH) =>
                        {
                            last = Some(Instant::now());
                            self.refresh(&ctx).await;
                        }
                        Some(_) => {}
                        None => break,
                    },
                    _ = tick.tick() => {
                        last = Some(Instant::now());
                        self.refresh(&ctx).await;
                    }
                }
            }
            Ok(())
        })
    }
}
