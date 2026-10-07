//! Map management: Trackmania Exchange, Nadeo (map uid, club campaigns) and local files.
//!
//! Files are uploaded to the server with `WriteFile` (no shared volume needed) under
//! `PitWall/` in its Maps directory, then added to the playlist.

use std::path::PathBuf;
use std::sync::Arc;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::types::MapInfo;
use pitwall_core::{
    Command, CommandSpec, Event, Invocation, Lang, Plugin, PluginContext, PluginError,
    PluginFuture, Replier, Role, Text, now_ms,
};
use pitwall_integrations::nadeo::Nadeo;
use pitwall_integrations::tmx::{self, Tmx};
use pitwall_storage::{MapRecord, Storage};

/// Where uploaded maps go, relative to the server's Maps directory.
const UPLOAD_DIR: &str = "PitWall";

/// The maps plugin.
pub struct MapsPlugin {
    tmx: Tmx,
    nadeo: Option<Arc<Nadeo>>,
    storage: Option<Storage>,
    /// Controller-side directory of local `.Map.Gbx` files.
    local_dir: PathBuf,
}

impl std::fmt::Debug for MapsPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapsPlugin")
            .field("nadeo", &self.nadeo.is_some())
            .field("local_dir", &self.local_dir)
            .finish_non_exhaustive()
    }
}

impl MapsPlugin {
    pub fn new(
        tmx: Tmx,
        nadeo: Option<Nadeo>,
        storage: Option<Storage>,
        local_dir: PathBuf,
    ) -> MapsPlugin {
        MapsPlugin {
            tmx,
            nadeo: nadeo.map(Arc::new),
            storage,
            local_dir,
        }
    }
}

impl Plugin for MapsPlugin {
    fn name(&self) -> &'static str {
        "maps"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("maps", "map", Role::Admin)
                .alias("maps")
                .usage("/map <tmx|search|uid|club|local|list|remove> ..."),
        ]
    }

    fn translations(&self) -> Vec<(Lang, &'static str)> {
        vec![
            (Lang::En, include_str!("../locales/en.toml")),
            (Lang::Fr, include_str!("../locales/fr.toml")),
        ]
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let this = Arc::new(*self);
            while let Some(event) = ctx.next_event().await {
                if let Event::Command { invocation } = event
                    && invocation.name == "map"
                {
                    let this = Arc::clone(&this);
                    let job = Job {
                        replier: ctx.replier(&invocation),
                        commands: ctx.commands_for(&invocation),
                    };
                    tokio::spawn(async move {
                        if let Err(message) = this.handle(&job, &invocation).await {
                            job.replier
                                .say(Text::tr("maps.failed").arg("message", message));
                        }
                    });
                }
            }
            Ok::<(), PluginError>(())
        })
    }
}

/// What a command task needs.
struct Job {
    replier: Replier,
    commands: pitwall_core::CommandHandle,
}

impl MapsPlugin {
    async fn handle(&self, job: &Job, inv: &Invocation) -> Result<(), String> {
        let usage = || {
            Text::tr("core.usage").arg("usage", "/map <tmx|search|uid|club|local|list|remove> ...")
        };
        match inv.arg(0) {
            Some("tmx") => {
                let id = inv
                    .arg(1)
                    .and_then(tmx::parse_id)
                    .ok_or("expected a TMX id or URL")?;
                let next = inv.arg(2) == Some("next");
                job.replier
                    .say(Text::tr("maps.downloading").arg("what", format!("TMX #{id}")));
                let info = self.tmx.map(id).await.map_err(|e| e.to_string())?;
                let data = self.tmx.download(id).await.map_err(|e| e.to_string())?;
                let file = format!("{UPLOAD_DIR}/tmx-{id}.Map.Gbx");
                self.upload(job, &file, data, next, "tmx", &id.to_string())
                    .await?;
                job.replier.say(
                    Text::tr("maps.added")
                        .arg("name", &info.gbx_map_name)
                        .arg("file", &file),
                );
            }
            Some("search") => {
                let query = inv.rest(1);
                if query.is_empty() {
                    job.replier.say(usage());
                    return Ok(());
                }
                let found = self
                    .tmx
                    .search(&query, 5)
                    .await
                    .map_err(|e| e.to_string())?;
                if found.is_empty() {
                    job.replier.say(Text::tr("maps.search_none"));
                }
                for map in found {
                    job.replier.say(
                        Text::tr("maps.search_entry")
                            .arg("id", map.map_id)
                            .arg("name", &map.gbx_map_name)
                            .arg("author", &map.uploader.name),
                    );
                }
            }
            Some("uid") => {
                let nadeo = self
                    .nadeo
                    .as_ref()
                    .ok_or_else(|| "Nadeo API not configured".to_owned())?;
                let uid = inv.arg(1).ok_or("expected a map uid")?;
                let next = inv.arg(2) == Some("next");
                job.replier
                    .say(Text::tr("maps.downloading").arg("what", uid));
                let maps = nadeo
                    .maps(&[uid.to_owned()])
                    .await
                    .map_err(|e| e.to_string())?;
                let map = maps.into_iter().next().ok_or("unknown map uid")?;
                let data = nadeo.download(&map).await.map_err(|e| e.to_string())?;
                let file = format!("{UPLOAD_DIR}/{}.Map.Gbx", map.map_uid);
                self.upload(job, &file, data, next, "nadeo", &map.map_uid)
                    .await?;
                job.replier.say(
                    Text::tr("maps.added")
                        .arg("name", &map.name)
                        .arg("file", &file),
                );
            }
            Some("club") => {
                let nadeo = self
                    .nadeo
                    .as_ref()
                    .ok_or_else(|| "Nadeo API not configured".to_owned())?;
                let (Some(club), Some(campaign)) = (
                    inv.arg(1).and_then(|v| v.parse().ok()),
                    inv.arg(2).and_then(|v| v.parse().ok()),
                ) else {
                    job.replier.say(
                        Text::tr("core.usage").arg("usage", "/map club <club id> <campaign id>"),
                    );
                    return Ok(());
                };
                let campaign = nadeo
                    .club_campaign(club, campaign)
                    .await
                    .map_err(|e| e.to_string())?;
                job.replier.say(
                    Text::tr("maps.campaign")
                        .arg("count", campaign.map_uids.len())
                        .arg("name", &campaign.name),
                );
                let maps = nadeo
                    .maps(&campaign.map_uids)
                    .await
                    .map_err(|e| e.to_string())?;
                for map in maps {
                    let data = nadeo.download(&map).await.map_err(|e| e.to_string())?;
                    let file = format!("{UPLOAD_DIR}/{}.Map.Gbx", map.map_uid);
                    match self
                        .upload(job, &file, data, false, "nadeo", &map.map_uid)
                        .await
                    {
                        Ok(()) => job.replier.say(
                            Text::tr("maps.added")
                                .arg("name", &map.name)
                                .arg("file", &file),
                        ),
                        Err(message) => job
                            .replier
                            .say(Text::tr("maps.failed").arg("message", message)),
                    }
                }
            }
            Some("local") => match inv.arg(1) {
                None => {
                    let files = self.local_files();
                    if files.is_empty() {
                        job.replier
                            .say(Text::tr("maps.local_none").arg("dir", self.local_dir.display()));
                    }
                    for file in files {
                        job.replier
                            .say(Text::tr("maps.local_entry").arg("file", file));
                    }
                }
                Some(name) => {
                    // Only plain file names from the local directory (no path traversal).
                    if name.contains(['/', '\\']) || name.starts_with('.') {
                        return Err("expected a file name from the local maps directory".to_owned());
                    }
                    let data = tokio::fs::read(self.local_dir.join(name))
                        .await
                        .map_err(|e| format!("{name}: {e}"))?;
                    let file = format!("{UPLOAD_DIR}/{name}");
                    self.upload(job, &file, data, inv.arg(2) == Some("next"), "upload", name)
                        .await?;
                    job.replier
                        .say(Text::tr("maps.added").arg("name", name).arg("file", &file));
                }
            },
            Some("list") => {
                let maps = self.map_list(job).await?;
                for (i, map) in maps.iter().enumerate() {
                    job.replier.say(
                        Text::tr("maps.list_entry")
                            .arg("index", i + 1)
                            .arg("name", &map.name)
                            .arg("file", &map.file_name),
                    );
                }
            }
            Some("remove") => {
                let file = match inv.arg(1) {
                    Some(file) => file.to_owned(),
                    None => {
                        let current = job
                            .commands
                            .call(Command::Raw {
                                method: "GetCurrentMapInfo".into(),
                                params: vec![],
                            })
                            .await
                            .map_err(|e| e.to_string())?;
                        let info: MapInfo = current.deserialize().map_err(|e| e.to_string())?;
                        info.file_name
                    }
                };
                job.commands
                    .call(Command::RemoveMap { file })
                    .await
                    .map_err(|e| e.to_string())?;
                job.replier.say(Text::tr("maps.removed"));
            }
            _ => job.replier.say(usage()),
        }
        Ok(())
    }

    async fn map_list(&self, job: &Job) -> Result<Vec<MapInfo>, String> {
        let list = job
            .commands
            .call(Command::Raw {
                method: "GetMapList".into(),
                params: vec![Value::Int(1000), Value::Int(0)],
            })
            .await
            .map_err(|e| e.to_string())?;
        list.deserialize().map_err(|e| e.to_string())
    }

    fn local_files(&self) -> Vec<String> {
        let mut files: Vec<String> = std::fs::read_dir(&self.local_dir)
            .map(|dir| {
                dir.filter_map(Result::ok)
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.to_ascii_lowercase().ends_with(".map.gbx"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    async fn upload(
        &self,
        job: &Job,
        file: &str,
        data: Vec<u8>,
        next: bool,
        source: &str,
        source_id: &str,
    ) -> Result<(), String> {
        if data.len() < 3 || &data[..3] != b"GBX" {
            return Err("the downloaded file is not a .Map.Gbx".to_owned());
        }
        job.commands
            .call(Command::UploadMap {
                file: file.to_owned(),
                data,
                insert_next: next,
            })
            .await
            .map_err(|e| e.to_string())?;
        if next {
            job.commands
                .call(Command::Raw {
                    method: "ChooseNextMap".into(),
                    params: vec![file.into()],
                })
                .await
                .map_err(|e| e.to_string())?;
        }
        // Remember where the map comes from.
        let info = job
            .commands
            .call(Command::Raw {
                method: "GetMapInfo".into(),
                params: vec![file.into()],
            })
            .await
            .ok()
            .and_then(|v| v.deserialize::<MapInfo>().ok());
        if let (Some(storage), Some(info)) = (&self.storage, info) {
            let record = MapRecord {
                uid: info.uid,
                name: info.name,
                author: info.author,
                file: file.to_owned(),
                source: source.to_owned(),
                source_id: source_id.to_owned(),
                author_time: info.author_time,
            };
            if let Err(err) = storage.upsert_map(&record, now_ms()).await {
                tracing::warn!(%err, "cannot record the map");
            }
        }
        Ok(())
    }
}
