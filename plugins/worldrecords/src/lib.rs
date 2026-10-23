//! World records of the current map, from Nadeo's leaderboards: the world top and the world
//! position of every connected player's personal best. Published as the custom event
//! `worldrecords/records` for the UI:
//!
//! ```json
//! { "top": [{ "account_id", "login", "name", "time", "position" }],
//!   "players": { "<login>": { "time", "position" } } }
//! ```
//!
//! Names of the top come from the Trackmania API (OAuth application) when configured, else from
//! the connected players. Requests run in background tasks; their results come back to the event
//! loop, tagged with the map, so a late answer for a previous map is dropped.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use pitwall_core::{Event, Plugin, PluginContext, PluginError, PluginFuture};
use pitwall_integrations::nadeo::{Nadeo, account_id_of_login};
use pitwall_integrations::trackmania::TrackmaniaApi;
use tokio::sync::mpsc;

/// Lines of the world top.
const TOP: u32 = 5;

/// The world records plugin.
#[derive(Debug)]
pub struct WorldRecordsPlugin {
    nadeo: Arc<Nadeo>,
    names: Option<Arc<TrackmaniaApi>>,
}

impl WorldRecordsPlugin {
    pub fn new(nadeo: Arc<Nadeo>, names: Option<Arc<TrackmaniaApi>>) -> WorldRecordsPlugin {
        WorldRecordsPlugin { nadeo, names }
    }
}

impl Plugin for WorldRecordsPlugin {
    fn name(&self) -> &'static str {
        "worldrecords"
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut wr = WorldRecords {
                nadeo: self.nadeo,
                names: self.names,
                tx,
                map: None,
                top: Vec::new(),
                players: HashMap::new(),
                display_names: HashMap::new(),
            };
            loop {
                tokio::select! {
                    event = ctx.next_event() => match event {
                        Some(event) => wr.on_event(&ctx, event),
                        None => break,
                    },
                    Some(result) = rx.recv() => wr.on_result(&ctx, result),
                }
            }
            Ok::<(), PluginError>(())
        })
    }
}

/// One line of the world top.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TopLine {
    account_id: String,
    position: i64,
    time: i64,
}

/// A connected player's personal best on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Best {
    time: i64,
    position: Option<i64>,
}

/// The map being shown.
#[derive(Debug, Clone)]
struct CurrentMap {
    uid: String,
    /// Records API id, once known.
    id: Option<String>,
}

/// Answers of the background requests.
#[derive(Debug)]
enum Answer {
    Map {
        uid: String,
        id: String,
        top: Vec<TopLine>,
        names: HashMap<String, String>,
    },
    Best {
        uid: String,
        login: String,
        best: Best,
    },
}

struct WorldRecords {
    nadeo: Arc<Nadeo>,
    names: Option<Arc<TrackmaniaApi>>,
    tx: mpsc::UnboundedSender<Answer>,
    map: Option<CurrentMap>,
    top: Vec<TopLine>,
    /// Login → personal best (only players who have one).
    players: HashMap<String, Best>,
    /// Account id → display name, kept across maps.
    display_names: HashMap<String, String>,
}

/// Account id of a connected player.
fn account_id(login: &str, known: &str) -> Option<String> {
    if known.is_empty() {
        account_id_of_login(login)
    } else {
        Some(known.to_owned())
    }
}

impl WorldRecords {
    fn on_event(&mut self, ctx: &PluginContext, event: Event) {
        match event {
            Event::Connected { .. } | Event::MapStart { .. } => self.sync_map(ctx),
            Event::PlayerJoined { player } => {
                if let Some(map) = self.map.clone()
                    && let Some(id) = map.id
                    && let Some(account) = account_id(&player.login, &player.account_id)
                {
                    self.fetch_bests(map.uid, id, vec![(player.login, account)]);
                }
            }
            Event::PlayerLeft { player, .. } => {
                if self.players.remove(&player.login).is_some() {
                    self.publish(ctx);
                }
            }
            Event::Finish { waypoint } if waypoint.racetime > 0 => {
                self.on_finish(
                    ctx,
                    &waypoint.login,
                    &waypoint.accountid,
                    i64::from(waypoint.racetime),
                );
            }
            _ => {}
        }
    }

    /// Loads the map the server is on, unless it is already the one shown.
    fn sync_map(&mut self, ctx: &PluginContext) {
        let Some(uid) = ctx.state.read(|s| s.map.as_ref().map(|m| m.uid.clone())) else {
            return;
        };
        if self.map.as_ref().is_some_and(|m| m.uid == uid) {
            return;
        }
        self.map = Some(CurrentMap {
            uid: uid.clone(),
            id: None,
        });
        self.top.clear();
        self.players.clear();
        self.publish(ctx);
        let (nadeo, names, tx) = (Arc::clone(&self.nadeo), self.names.clone(), self.tx.clone());
        let known = self.display_names.clone();
        tokio::spawn(async move {
            let id = match nadeo.maps(std::slice::from_ref(&uid)).await {
                Ok(maps) => maps.into_iter().next().map(|m| m.map_id),
                Err(err) => {
                    tracing::warn!(%err, uid, "cannot get the map from the Nadeo API");
                    return;
                }
            };
            let Some(id) = id.filter(|id| !id.is_empty()) else {
                tracing::info!(uid, "map unknown to Nadeo: no world records");
                return;
            };
            let top = match nadeo.world_top(&uid, TOP).await {
                Ok(top) => top
                    .into_iter()
                    .filter(|e| e.score > 0)
                    .map(|e| TopLine {
                        account_id: e.account_id,
                        position: e.position,
                        time: e.score,
                    })
                    .collect(),
                Err(err) => {
                    tracing::warn!(%err, uid, "cannot get the world top");
                    Vec::new()
                }
            };
            let missing: Vec<String> = top
                .iter()
                .map(|l: &TopLine| l.account_id.clone())
                .filter(|id| !known.contains_key(id))
                .collect();
            let names = match names {
                Some(api) if !missing.is_empty() => {
                    api.display_names(&missing).await.unwrap_or_else(|err| {
                        tracing::warn!(%err, "cannot get display names");
                        HashMap::new()
                    })
                }
                _ => HashMap::new(),
            };
            let _ = tx.send(Answer::Map {
                uid,
                id,
                top,
                names,
            });
        });
    }

    /// Personal bests of some players on the map, then the world position of each.
    fn fetch_bests(&self, uid: String, id: String, players: Vec<(String, String)>) {
        let (nadeo, tx) = (Arc::clone(&self.nadeo), self.tx.clone());
        tokio::spawn(async move {
            let accounts: Vec<String> = players.iter().map(|(_, a)| a.clone()).collect();
            let bests = match nadeo.personal_bests(&id, &accounts).await {
                Ok(bests) => bests,
                Err(err) => {
                    tracing::warn!(%err, uid, "cannot get personal bests");
                    return;
                }
            };
            for (account, time) in bests {
                let Some((login, _)) = players.iter().find(|(_, a)| *a == account) else {
                    continue;
                };
                let position = Self::position(&nadeo, &uid, time).await;
                let best = Best { time, position };
                let _ = tx.send(Answer::Best {
                    uid: uid.clone(),
                    login: login.clone(),
                    best,
                });
            }
        });
    }

    async fn position(nadeo: &Nadeo, uid: &str, time: i64) -> Option<i64> {
        nadeo.world_position(uid, time).await.unwrap_or_else(|err| {
            tracing::debug!(%err, uid, "cannot get a world position");
            None
        })
    }

    /// A finish that beats the player's personal best: new position, and maybe a place in the top.
    fn on_finish(&mut self, ctx: &PluginContext, login: &str, account: &str, time: i64) {
        let Some(map) = self.map.clone() else {
            return;
        };
        // Only Time Attack runs are taken as personal bests: other modes may not reach the
        // leaderboards.
        if !ctx.state.read(|s| s.time_attack()) {
            return;
        }
        if self.players.get(login).is_some_and(|b| b.time <= time) {
            return;
        }
        // Shown at once; the position follows.
        let position = self.players.get(login).and_then(|b| b.position);
        self.players
            .insert(login.to_owned(), Best { time, position });
        if let Some(account) = account_id(login, account) {
            self.merge_top(&account, time);
        }
        self.publish(ctx);
        let (nadeo, tx, login) = (Arc::clone(&self.nadeo), self.tx.clone(), login.to_owned());
        tokio::spawn(async move {
            let position = Self::position(&nadeo, &map.uid, time).await;
            let _ = tx.send(Answer::Best {
                uid: map.uid,
                login,
                best: Best { time, position },
            });
        });
    }

    /// Puts a new time in the top if it belongs there (leaderboards lag behind the server).
    fn merge_top(&mut self, account: &str, time: i64) {
        let mut top = self.top.clone();
        match top.iter_mut().find(|l| l.account_id == account) {
            Some(line) if line.time <= time => return,
            Some(line) => line.time = time,
            None => {
                let full = top.len() >= TOP as usize;
                if full && top.last().is_some_and(|l| l.time <= time) {
                    return;
                }
                top.push(TopLine {
                    account_id: account.to_owned(),
                    position: 0,
                    time,
                });
            }
        }
        top.sort_by_key(|l| l.time);
        top.truncate(TOP as usize);
        for (i, line) in top.iter_mut().enumerate() {
            line.position = i as i64 + 1;
        }
        self.top = top;
    }

    fn on_result(&mut self, ctx: &PluginContext, answer: Answer) {
        let current = self.map.as_ref().map(|m| m.uid.clone());
        match answer {
            Answer::Map {
                uid,
                id,
                top,
                names,
            } if current.as_deref() == Some(&uid) => {
                self.display_names.extend(names);
                // Finishes driven while the request ran are kept.
                let local: Vec<(String, i64)> = self
                    .top
                    .iter()
                    .map(|l| (l.account_id.clone(), l.time))
                    .collect();
                self.top = top;
                for (account, time) in local {
                    self.merge_top(&account, time);
                }
                if let Some(map) = self.map.as_mut() {
                    map.id = Some(id.clone());
                }
                self.publish(ctx);
                let players: Vec<(String, String)> = ctx.state.read(|s| {
                    s.players
                        .values()
                        .filter_map(|p| {
                            Some((p.login.clone(), account_id(&p.login, &p.account_id)?))
                        })
                        .collect()
                });
                self.fetch_bests(uid, id, players);
            }
            Answer::Best { uid, login, best } if current.as_deref() == Some(&uid) => {
                // A better time driven meanwhile wins.
                if self.players.get(&login).is_some_and(|b| b.time < best.time) {
                    return;
                }
                if ctx.state.player(&login).is_none() {
                    return;
                }
                self.players.insert(login, best);
                self.publish(ctx);
            }
            _ => {}
        }
    }

    fn publish(&self, ctx: &PluginContext) {
        // Connected players by account id, for names and highlighting.
        let connected: HashMap<String, (String, String)> = ctx.state.read(|s| {
            s.players
                .values()
                .filter_map(|p| {
                    let account = account_id(&p.login, &p.account_id)?;
                    Some((account, (p.login.clone(), p.nickname.clone())))
                })
                .collect()
        });
        let top: Vec<serde_json::Value> = self
            .top
            .iter()
            .map(|l| {
                let (login, nickname) = connected.get(&l.account_id).cloned().unwrap_or_default();
                let name = self
                    .display_names
                    .get(&l.account_id)
                    .cloned()
                    .filter(|n| !n.is_empty())
                    .or_else(|| Some(nickname).filter(|n| !n.is_empty()))
                    .unwrap_or_else(|| l.account_id.chars().take(8).collect());
                serde_json::json!({
                    "account_id": l.account_id,
                    "login": login,
                    "name": name,
                    "time": l.time,
                    "position": l.position,
                })
            })
            .collect();
        let players: BTreeMap<&String, serde_json::Value> = self
            .players
            .iter()
            .map(|(login, b)| {
                (
                    login,
                    serde_json::json!({ "time": b.time, "position": b.position }),
                )
            })
            .collect();
        ctx.emit(
            "records",
            serde_json::json!({ "top": top, "players": players }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> WorldRecords {
        let (tx, _rx) = mpsc::unbounded_channel();
        WorldRecords {
            nadeo: Arc::new(Nadeo::new(reqwest::Client::new(), "login", "password").unwrap()),
            names: None,
            tx,
            map: None,
            top: Vec::new(),
            players: HashMap::new(),
            display_names: HashMap::new(),
        }
    }

    fn line(account: &str, position: i64, time: i64) -> TopLine {
        TopLine {
            account_id: account.to_owned(),
            position,
            time,
        }
    }

    #[test]
    fn new_times_enter_the_top_in_order() {
        let mut wr = records();
        wr.top = (1..=5)
            .map(|i| line(&format!("a{i}"), i, 10_000 * i))
            .collect();
        // Too slow for the top.
        wr.merge_top("me", 60_000);
        assert!(!wr.top.iter().any(|l| l.account_id == "me"));
        // Third place: the last one leaves.
        wr.merge_top("me", 25_000);
        let order: Vec<_> = wr
            .top
            .iter()
            .map(|l| (l.account_id.as_str(), l.position))
            .collect();
        assert_eq!(
            order,
            [("a1", 1), ("a2", 2), ("me", 3), ("a3", 4), ("a4", 5)]
        );
        // A player already in the top only moves up.
        wr.merge_top("a4", 45_000);
        assert_eq!(wr.top[4].time, 40_000);
        wr.merge_top("a4", 5_000);
        assert_eq!(wr.top[0].account_id, "a4");
        assert_eq!(wr.top.len(), 5);
    }

    #[test]
    fn short_tops_accept_any_time() {
        let mut wr = records();
        wr.merge_top("me", 99_000);
        assert_eq!(wr.top, vec![line("me", 1, 99_000)]);
    }
}
