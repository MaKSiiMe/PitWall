//! Records what happens on the server: players, maps, finished runs (with checkpoints), cup runs and
//! phase results, ratings and the audit log. Publishes the best times of the current map as the
//! custom event `storage/records` for the UI.

use pitwall_core::{Event, Player, Plugin, PluginContext, PluginError, PluginFuture, now_ms};
use pitwall_formats::cup::{CupKind, CupSnapshot, CupStatus, Standing};
use pitwall_storage::{MapRecord, PlayerRecord, RunRecord, Storage, StorageError};

/// The storage plugin.
#[derive(Debug, Clone)]
pub struct StoragePlugin {
    storage: Storage,
    /// Rows of the records widget.
    records_limit: i64,
}

impl StoragePlugin {
    pub fn new(storage: Storage) -> StoragePlugin {
        StoragePlugin {
            storage,
            records_limit: 12,
        }
    }
}

impl Plugin for StoragePlugin {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let mut recorder = Recorder {
                db: self.storage,
                records_limit: self.records_limit,
                cup: None,
                cup_run: None,
            };
            while let Some(event) = ctx.next_event().await {
                if let Err(err) = recorder.on_event(&ctx, event).await {
                    tracing::error!(%err, "storage error");
                }
            }
            Ok::<(), PluginError>(())
        })
    }
}

struct Recorder {
    db: Storage,
    records_limit: i64,
    cup: Option<CupSnapshot>,
    /// Database id of the running cup.
    cup_run: Option<i64>,
}

fn player_record(p: &Player) -> PlayerRecord {
    PlayerRecord {
        login: p.login.clone(),
        nickname: p.nickname.clone(),
        account_id: p.account_id.clone(),
        zone: p.zone.clone(),
        lang: p.lang.code().to_owned(),
    }
}

impl Recorder {
    async fn publish_records(&self, ctx: &PluginContext) -> Result<(), StorageError> {
        let Some(uid) = ctx.state.read(|s| s.map.as_ref().map(|m| m.uid.clone())) else {
            return Ok(());
        };
        let rows = self.db.best_times(&uid, self.records_limit).await?;
        ctx.emit(
            "records",
            serde_json::json!({ "map_uid": uid, "rows": rows }),
        );
        Ok(())
    }

    async fn save_current_map(&self, ctx: &PluginContext) -> Result<(), StorageError> {
        let Some(map) = ctx.state.read(|s| s.map.clone()) else {
            return Ok(());
        };
        if map.uid.is_empty() {
            return Ok(());
        }
        self.db
            .upsert_map(
                &MapRecord {
                    uid: map.uid,
                    name: map.name,
                    author: map.author,
                    file: map.file_name,
                    source: "server".into(),
                    source_id: String::new(),
                    author_time: map.author_time,
                },
                now_ms(),
            )
            .await
    }

    async fn on_event(&mut self, ctx: &PluginContext, event: Event) -> Result<(), StorageError> {
        match event {
            Event::Connected { .. } => {
                for player in ctx
                    .state
                    .read(|s| s.players.values().cloned().collect::<Vec<_>>())
                {
                    self.db
                        .upsert_player(&player_record(&player), now_ms())
                        .await?;
                }
                self.save_current_map(ctx).await?;
                self.publish_records(ctx).await?;
            }
            Event::PlayerJoined { player } | Event::PlayerUpdated { player } => {
                self.db
                    .upsert_player(&player_record(&player), now_ms())
                    .await?;
            }
            Event::MapStart { .. } => {
                self.save_current_map(ctx).await?;
                self.publish_records(ctx).await?;
            }
            Event::Finish { waypoint } if waypoint.racetime > 0 => {
                let (map_uid, mode) = ctx.state.read(|s| {
                    (
                        s.map.as_ref().map(|m| m.uid.clone()).unwrap_or_default(),
                        s.script.clone(),
                    )
                });
                if map_uid.is_empty() {
                    return Ok(());
                }
                let running = self.cup.as_ref().filter(|c| {
                    matches!(
                        c.status,
                        CupStatus::Running { .. } | CupStatus::Paused { .. }
                    )
                });
                let run = RunRecord {
                    map_uid,
                    login: waypoint.login.clone(),
                    time: waypoint.racetime,
                    checkpoints: waypoint.curracecheckpoints.clone(),
                    mode,
                    cup_id: running.map(|c| c.id.clone()),
                    phase_id: running.and_then(|c| c.phase.as_ref().map(|p| p.id.clone())),
                    at: now_ms(),
                };
                self.db.record_run(&run).await?;
                self.publish_records(ctx).await?;
            }
            Event::Audit { entry } => {
                self.db
                    .audit(
                        entry.at,
                        &entry.actor.to_string(),
                        &entry.action,
                        &entry.details,
                        entry.success,
                    )
                    .await?;
            }
            Event::Custom {
                source,
                name,
                payload,
            } if source == "cup" => {
                self.on_cup_event(&name, payload).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_cup_event(
        &mut self,
        name: &str,
        payload: serde_json::Value,
    ) -> Result<(), StorageError> {
        match name {
            "state" => {
                let snapshot: Option<CupSnapshot> = serde_json::from_value(payload).unwrap_or(None);
                if let Some(cup) = &snapshot {
                    let started = !matches!(cup.status, CupStatus::Draft | CupStatus::Registration);
                    if started && self.cup_run.is_none() {
                        self.cup_run = match self.db.current_cup_run(&cup.id).await? {
                            Some(id) => Some(id),
                            None => {
                                let name = if cup.name.en.is_empty() {
                                    &cup.id
                                } else {
                                    &cup.name.en
                                };
                                Some(self.db.start_cup_run(&cup.id, name, now_ms()).await?)
                            }
                        };
                    }
                    if matches!(cup.status, CupStatus::Aborted)
                        && let Some(id) = self.cup_run.take()
                    {
                        self.db.finish_cup_run(id, "aborted", now_ms()).await?;
                    }
                } else {
                    self.cup_run = None;
                }
                self.cup = snapshot;
            }
            "phase_completed" => {
                let Some(run_id) = self.cup_run else {
                    return Ok(());
                };
                let result = &payload["result"];
                let qualified: Vec<String> =
                    serde_json::from_value(result["qualified"].clone()).unwrap_or_default();
                self.db
                    .record_phase(
                        run_id,
                        result["phase_index"].as_i64().unwrap_or_default(),
                        result["phase_id"].as_str().unwrap_or_default(),
                        &result["standings"],
                        &qualified,
                        result["finished_at"].as_i64().unwrap_or_else(now_ms),
                    )
                    .await?;
            }
            "finished" => {
                if let Some(run_id) = self.cup_run.take() {
                    self.db.finish_cup_run(run_id, "finished", now_ms()).await?;
                }
                // Ratings only make sense for individual cups.
                if self
                    .cup
                    .as_ref()
                    .is_some_and(|c| c.kind == CupKind::Players)
                {
                    let standings: Vec<Standing> =
                        serde_json::from_value(payload["standings"].clone()).unwrap_or_default();
                    let ranking: Vec<(String, u32)> =
                        standings.into_iter().map(|s| (s.id, s.rank)).collect();
                    self.db.update_ratings(&ranking).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
