//! Map management: Trackmania Exchange, Nadeo (map uid, club campaigns) and local files.
//!
//! Files are uploaded to the server with `WriteFile` (no shared volume needed) under
//! `PitWall/` in its Maps directory, then added to the playlist.

use std::path::PathBuf;
use std::sync::Arc;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::types::MapInfo;
use pitwall_core::{
    Command, CommandSpec, Event, EventBus, Invocation, Lang, Plugin, PluginContext, PluginError,
    PluginFuture, Replier, Role, StateHandle, Text, now_ms,
};
use pitwall_integrations::nadeo::Nadeo;
use pitwall_integrations::tmx::{self, Tmx};
use pitwall_storage::{MapRecord, Storage};
use source::MapSource;

mod source;

/// Where uploaded maps go, relative to the server's Maps directory.
const UPLOAD_DIR: &str = "PitWall";

/// The maps plugin.
pub struct MapsPlugin {
    tmx: Tmx,
    nadeo: Option<Arc<Nadeo>>,
    storage: Option<Storage>,
    /// Controller-side directory of local `.Map.Gbx` files.
    local_dir: PathBuf,
    /// Players may add maps (`/map add|tmx|uid|search`), not only admins.
    players_can_add: bool,
}

/// Sub-commands opened to players by [`MapsPlugin::players_can_add`].
const PLAYER_SUB_COMMANDS: [&str; 4] = ["add", "tmx", "uid", "search"];

impl std::fmt::Debug for MapsPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapsPlugin")
            .field("nadeo", &self.nadeo.is_some())
            .field("local_dir", &self.local_dir)
            .field("players_can_add", &self.players_can_add)
            .finish_non_exhaustive()
    }
}

impl MapsPlugin {
    pub fn new(
        tmx: Tmx,
        nadeo: Option<Arc<Nadeo>>,
        storage: Option<Storage>,
        local_dir: PathBuf,
    ) -> MapsPlugin {
        MapsPlugin {
            tmx,
            nadeo,
            storage,
            local_dir,
            players_can_add: false,
        }
    }

    /// Lets players add maps from Trackmania Exchange or by uid (`[plugins.maps]
    /// players_can_add`); the rest of the map manager stays for admins.
    pub fn players_can_add(mut self, allowed: bool) -> MapsPlugin {
        self.players_can_add = allowed;
        self
    }
}

impl Plugin for MapsPlugin {
    fn name(&self) -> &'static str {
        "maps"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        let spec = CommandSpec::new("maps", "map", Role::Admin).usage(
            "/map <list|move|next|play|remove|clear|add|tmx|search|uid|club|room|local> ...",
        );
        let spec = if self.players_can_add {
            PLAYER_SUB_COMMANDS
                .into_iter()
                .fold(spec, |spec, sub| spec.sub(sub, Role::Player))
        } else {
            spec
        };
        vec![spec]
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
                        state: ctx.state.clone(),
                        bus: ctx.bus.clone(),
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

/// Kind of club map list.
#[derive(Debug, Clone, Copy)]
enum ClubList {
    Campaign,
    Room,
}

/// What a command task needs.
struct Job {
    replier: Replier,
    commands: pitwall_core::CommandHandle,
    state: StateHandle,
    bus: EventBus,
}

impl Job {
    /// File of the map being played.
    fn playing(&self) -> Option<String> {
        self.state
            .read(|s| s.map.as_ref().map(|m| m.file_name.clone()))
            .filter(|f| !f.is_empty())
    }
}

impl MapsPlugin {
    async fn handle(&self, job: &Job, inv: &Invocation) -> Result<(), String> {
        let usage = || {
            Text::tr("core.usage").arg(
                "usage",
                "/map <list|move|next|play|remove|clear|add|tmx|search|uid|club|room|local> ...",
            )
        };
        match inv.arg(0) {
            Some("add") => {
                let input = inv
                    .arg(1)
                    .ok_or("expected a TMX id or URL, a trackmania.io campaign URL or a map uid")?;
                let next = inv.arg(2) == Some("next");
                match source::parse(input) {
                    Some(MapSource::Tmx(id)) => self.add_tmx(job, id, next).await?,
                    Some(MapSource::Uid(uid)) => self.add_uid(job, &uid, next).await?,
                    Some(MapSource::Campaign { club, campaign }) => {
                        self.add_club(job, ClubList::Campaign, club, campaign)
                            .await?;
                    }
                    None => {
                        return Err(format!(
                            "{input}: not a TMX id/URL, a trackmania.io campaign URL or a map uid"
                        ));
                    }
                }
            }
            Some("tmx") => {
                let id = inv
                    .arg(1)
                    .and_then(tmx::parse_id)
                    .ok_or("expected a TMX id or URL")?;
                self.add_tmx(job, id, inv.arg(2) == Some("next")).await?;
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
                // For the map manager window of the UI plugin.
                if let Some(login) = inv.login() {
                    let results: Vec<_> = found
                        .iter()
                        .map(|m| {
                            serde_json::json!({
                                "id": m.map_id,
                                "name": m.gbx_map_name,
                                "author": m.uploader.name,
                            })
                        })
                        .collect();
                    job.bus.publish(Event::custom(
                        "maps",
                        "search",
                        serde_json::json!({ "login": login, "query": query, "results": results }),
                    ));
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
                let uid = inv.arg(1).ok_or("expected a map uid")?;
                self.add_uid(job, uid, inv.arg(2) == Some("next")).await?;
            }
            Some(kind @ ("club" | "room")) => {
                let list = if kind == "club" {
                    ClubList::Campaign
                } else {
                    ClubList::Room
                };
                // A trackmania.io campaign URL works too.
                let ids = match inv.arg(1).and_then(source::parse) {
                    Some(MapSource::Campaign { club, campaign }) if kind == "club" => {
                        Some((club, campaign))
                    }
                    _ => inv
                        .arg(1)
                        .and_then(|v| v.parse().ok())
                        .zip(inv.arg(2).and_then(|v| v.parse().ok())),
                };
                let Some((club, id)) = ids else {
                    let usage = if kind == "club" {
                        "/map club <club id> <campaign id>"
                    } else {
                        "/map room <club id> <room id>"
                    };
                    job.replier.say(Text::tr("core.usage").arg("usage", usage));
                    return Ok(());
                };
                self.add_club(job, list, club, id).await?;
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
                let playing = job.playing();
                for (i, map) in maps.iter().enumerate() {
                    let key = if Some(&map.file_name) == playing.as_ref() {
                        "maps.list_current"
                    } else {
                        "maps.list_entry"
                    };
                    job.replier.say(
                        Text::tr(key)
                            .arg("index", i + 1)
                            .arg("name", &map.name)
                            .arg("file", &map.file_name),
                    );
                }
            }
            Some("remove") => {
                let file = match inv.arg(1) {
                    Some(arg) => self.find(job, arg).await?.file_name,
                    None => job.playing().ok_or("no map is being played")?,
                };
                job.commands
                    .call(Command::RemoveMap { file })
                    .await
                    .map_err(|e| e.to_string())?;
                job.replier.say(Text::tr("maps.removed"));
            }
            Some("move") => {
                let (Some(from), Some(to)) = (
                    inv.arg(1).and_then(|v| v.parse::<usize>().ok()),
                    inv.arg(2).and_then(|v| v.parse::<usize>().ok()),
                ) else {
                    job.replier
                        .say(Text::tr("core.usage").arg("usage", "/map move <from> <to>"));
                    return Ok(());
                };
                let mut files: Vec<String> = self
                    .map_list(job)
                    .await?
                    .into_iter()
                    .map(|m| m.file_name)
                    .collect();
                let range = 1..=files.len();
                if !range.contains(&from) || !range.contains(&to) {
                    return Err(format!("positions go from 1 to {}", files.len()));
                }
                let file = files.remove(from - 1);
                files.insert(to - 1, file);
                job.commands
                    .call(Command::SetPlaylist { files })
                    .await
                    .map_err(|e| e.to_string())?;
                job.replier
                    .say(Text::tr("maps.moved").arg("from", from).arg("to", to));
            }
            Some("next") => {
                let map = self
                    .find(job, inv.arg(1).ok_or("expected a map number or file")?)
                    .await?;
                job.commands
                    .call(Command::ChooseNextMap {
                        file: map.file_name,
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                job.replier
                    .say(Text::tr("maps.queued").arg("name", &map.name));
            }
            Some("play") => {
                let map = self
                    .find(job, inv.arg(1).ok_or("expected a map number or file")?)
                    .await?;
                job.commands
                    .call(Command::JumpToMap { uid: map.uid })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Some("clear") => {
                let playing = job.playing().ok_or("no map is being played")?;
                job.commands
                    .call(Command::SetPlaylist {
                        files: vec![playing],
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                job.replier.say(Text::tr("maps.cleared"));
            }
            _ => job.replier.say(usage()),
        }
        Ok(())
    }

    async fn add_tmx(&self, job: &Job, id: i64, next: bool) -> Result<(), String> {
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
        Ok(())
    }

    fn nadeo(&self) -> Result<&Nadeo, String> {
        self.nadeo
            .as_deref()
            .ok_or_else(|| "Nadeo API not configured".to_owned())
    }

    async fn add_uid(&self, job: &Job, uid: &str, next: bool) -> Result<(), String> {
        let nadeo = self.nadeo()?;
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
        Ok(())
    }

    /// Every map of a club campaign or room, in order; one failure does not stop the others.
    async fn add_club(&self, job: &Job, list: ClubList, club: i64, id: i64) -> Result<(), String> {
        let nadeo = self.nadeo()?;
        let list = match list {
            ClubList::Campaign => nadeo.club_campaign(club, id).await,
            ClubList::Room => nadeo.club_room(club, id).await,
        }
        .map_err(|e| e.to_string())?;
        if list.map_uids.is_empty() {
            return Err(format!("{}: no map (room linked to a server?)", list.name));
        }
        job.replier.say(
            Text::tr("maps.campaign")
                .arg("count", list.map_uids.len())
                .arg("name", &list.name),
        );
        let maps = nadeo
            .maps(&list.map_uids)
            .await
            .map_err(|e| e.to_string())?;
        for map in maps {
            let file = format!("{UPLOAD_DIR}/{}.Map.Gbx", map.map_uid);
            let result = match nadeo.download(&map).await {
                Ok(data) => {
                    self.upload(job, &file, data, false, "nadeo", &map.map_uid)
                        .await
                }
                Err(err) => Err(err.to_string()),
            };
            match result {
                Ok(()) => job.replier.say(
                    Text::tr("maps.added")
                        .arg("name", &map.name)
                        .arg("file", &file),
                ),
                Err(message) => job.replier.say(
                    Text::tr("maps.failed").arg("message", format!("{}: {message}", map.name)),
                ),
            }
        }
        Ok(())
    }

    /// A playlist map by its number in `/map list` or its file name.
    async fn find(&self, job: &Job, arg: &str) -> Result<MapInfo, String> {
        let maps = self.map_list(job).await?;
        let found = match arg.parse::<usize>() {
            Ok(n) => n.checked_sub(1).and_then(|i| maps.get(i)).cloned(),
            Err(_) => maps.iter().find(|m| m.file_name == arg).cloned(),
        };
        found.ok_or_else(|| format!("{arg}: not in the playlist (see /map list)"))
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
                .call(Command::ChooseNextMap {
                    file: file.to_owned(),
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
