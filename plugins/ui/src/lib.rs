//! In-game widgets: live ranking, checkpoint difference, cup banner with ready button, best times,
//! admin panel and a menu to show/hide them. Preferences are per player and saved on disk.
//!
//! Updates are targeted: views are sent once per player; afterwards only the small data layers are
//! sent, to the players displaying the widget, at most every 250 ms, and only when they changed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use pitwall_core::gbx::Value;
use pitwall_core::gbx::callbacks::Scores;
use pitwall_core::gbx::types::MapInfo;
use pitwall_core::{
    Command, CommandSpec, Event, Invocation, Lang, Plugin, PluginContext, PluginError,
    PluginFuture, Recipients, Role, Text, now_ms,
};
use pitwall_formats::FormatRegistry;
use pitwall_formats::cup::{CupSnapshot, CupStatus};
use pitwall_ui::model::{CupInfoData, RankingData, RankingRow, RecordRow, RecordsData};
use pitwall_ui::render::{AdminContext, MapEntry, MapsWindow, MenuContext, TmxEntry};
use pitwall_ui::{Layout, Renderer, Theme, WidgetId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Shared format registry (for the admin panel's format buttons).
pub type SharedFormats = Arc<RwLock<FormatRegistry>>;

/// `[plugins.ui]`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UiConfig {
    theme: Theme,
    layout: Layout,
}

/// Per-player display preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub hidden: BTreeSet<WidgetId>,
    pub all_hidden: bool,
}

/// The UI plugin.
#[derive(Debug)]
pub struct UiPlugin {
    formats: SharedFormats,
    data_dir: PathBuf,
}

impl UiPlugin {
    /// Preferences are saved in `<data_dir>/ui_prefs.json`.
    pub fn new(formats: SharedFormats, data_dir: PathBuf) -> UiPlugin {
        UiPlugin { formats, data_dir }
    }
}

impl Plugin for UiPlugin {
    fn name(&self) -> &'static str {
        "ui"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("ui", "ui", Role::Player).usage("/ui <widget|all> [on|off]"),
            CommandSpec::new("ui", "maps", Role::Admin).usage("/maps [page]"),
        ]
    }

    fn translations(&self) -> Vec<(Lang, &'static str)> {
        pitwall_ui::catalogs().to_vec()
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let config: UiConfig = ctx.config()?;
            let prefs_file = self.data_dir.join("ui_prefs.json");
            let prefs = std::fs::read_to_string(&prefs_file)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
                .unwrap_or_default();
            let (playlist_tx, mut playlist_rx) = mpsc::unbounded_channel();
            let mut ui = Ui {
                renderer: Renderer::new(config.theme, config.layout),
                formats: self.formats,
                prefs_file,
                prefs,
                best: HashMap::new(),
                finished: BTreeSet::new(),
                scores: None,
                cup: None,
                ready: None,
                records: None,
                dirty: BTreeSet::new(),
                sent: HashMap::new(),
                last_data: HashMap::new(),
                maps: MapsWindows {
                    open: HashMap::new(),
                    searches: HashMap::new(),
                    playlist: Vec::new(),
                    tx: playlist_tx,
                },
            };
            let mut flush = tokio::time::interval(Duration::from_millis(250));
            flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    event = ctx.next_event() => match event {
                        Some(event) => ui.on_event(&ctx, event),
                        None => break,
                    },
                    _ = flush.tick() => ui.flush(&ctx),
                    Some(playlist) = playlist_rx.recv() => ui.on_playlist(&ctx, playlist),
                }
            }
            Ok::<(), PluginError>(())
        })
    }
}

/// A finished run.
#[derive(Debug, Clone)]
struct Run {
    name: String,
    time: i32,
}

/// What a player currently has on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sent {
    lang: Lang,
    role: Role,
    widgets: BTreeSet<WidgetId>,
}

struct Ui {
    renderer: Renderer,
    formats: SharedFormats,
    prefs_file: PathBuf,
    prefs: BTreeMap<String, Prefs>,
    /// Best run per login on the current map.
    best: HashMap<String, Run>,
    /// Logins that finished the current round (points modes).
    finished: BTreeSet<String>,
    scores: Option<Arc<Scores>>,
    cup: Option<CupSnapshot>,
    /// Ready check: logins and deadline (Unix ms).
    ready: Option<(Vec<String>, i64)>,
    /// Best times provided by the storage plugin, if any.
    records: Option<Vec<RecordRow>>,
    dirty: BTreeSet<WidgetId>,
    sent: HashMap<String, Sent>,
    /// Last JSON sent per data layer (and language), to skip identical updates.
    last_data: HashMap<(WidgetId, Option<Lang>), String>,
    maps: MapsWindows,
}

/// Map manager windows (`/maps`), open for some admins.
struct MapsWindows {
    /// Login → page shown.
    open: HashMap<String, usize>,
    /// Login → last TMX search (query, results).
    searches: HashMap<String, (String, Vec<TmxEntry>)>,
    /// Playlist as last fetched.
    playlist: Vec<MapInfo>,
    /// Fetched playlists come back to the event loop through this channel.
    tx: mpsc::UnboundedSender<Vec<MapInfo>>,
}

/// Audited commands after which the open map windows are refreshed.
const PLAYLIST_ACTIONS: [&str; 6] = [
    "upload_map",
    "add_map",
    "remove_map",
    "set_playlist",
    "choose_next_map",
    "jump_to_map",
];

impl Ui {
    fn prefs(&self, login: &str) -> Prefs {
        self.prefs.get(login).cloned().unwrap_or_default()
    }

    fn save_prefs(&self) {
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.prefs_file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let json = serde_json::to_vec_pretty(&self.prefs).map_err(std::io::Error::other)?;
            std::fs::write(&self.prefs_file, json)
        })();
        if let Err(err) = result {
            tracing::warn!(%err, "cannot save UI preferences");
        }
    }

    /// Widgets sent to a player. Hidden ones are sent too: hiding happens on the client, from
    /// the menu, so that it is instant.
    fn wanted(role: Role) -> BTreeSet<WidgetId> {
        WidgetId::ALL
            .into_iter()
            .filter(|w| *w != WidgetId::AdminPanel || role >= Role::Referee)
            .collect()
    }

    fn send(ctx: &PluginContext, logins: Vec<String>, xml: String) {
        if logins.is_empty() {
            return;
        }
        ctx.commands.send(Command::Manialink {
            to: Recipients::Logins(logins),
            xml,
            timeout_ms: 0,
            hide_on_click: false,
        });
    }

    fn admin_context(&self, ctx: &PluginContext, role: Role, lang: Lang) -> AdminContext {
        let formats = self
            .formats
            .read()
            .map(|f| {
                f.all()
                    .map(|d| (d.id.clone(), d.display_name(lang).to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let _ = ctx;
        AdminContext {
            role,
            formats,
            cup_loaded: self.cup.is_some(),
        }
    }

    fn menu_context(&self, login: &str, role: Role) -> MenuContext {
        let prefs = self.prefs(login);
        MenuContext {
            widgets: WidgetId::HIDEABLE
                .into_iter()
                .filter(|w| *w != WidgetId::AdminPanel || role >= Role::Referee)
                .map(|w| (w, !prefs.hidden.contains(&w)))
                .collect(),
            all_hidden: prefs.all_hidden,
        }
    }

    /// (Re)sends every view of a player and the current data.
    fn send_views(&mut self, ctx: &PluginContext, login: &str) {
        let Some(player) = ctx.state.player(login) else {
            return;
        };
        let (lang, role) = (player.lang, ctx.roles.get(login));
        let wanted = Self::wanted(role);
        let previous = self
            .sent
            .get(login)
            .map(|s| s.widgets.clone())
            .unwrap_or_default();
        for widget in WidgetId::ALL {
            let xml = if wanted.contains(&widget) {
                match widget {
                    WidgetId::AdminPanel => {
                        let admin = self.admin_context(ctx, role, lang);
                        self.renderer.admin_panel(lang, &ctx.i18n, &admin)
                    }
                    WidgetId::Menu => {
                        self.renderer
                            .menu(lang, &ctx.i18n, &self.menu_context(login, role))
                    }
                    _ => self.renderer.view(widget, lang, &ctx.i18n),
                }
            } else if previous.contains(&widget) {
                self.renderer.hide(widget)
            } else {
                continue;
            };
            match xml {
                Ok(xml) => Self::send(ctx, vec![login.to_owned()], xml),
                Err(err) => tracing::error!(%err, widget = widget.key(), "cannot render widget"),
            }
        }
        tracing::info!(
            login,
            lang = %lang,
            widgets = ?wanted.iter().map(|w| w.key()).collect::<Vec<_>>(),
            "widgets sent"
        );
        self.sent.insert(
            login.to_owned(),
            Sent {
                lang,
                role,
                widgets: wanted.clone(),
            },
        );
        // Current data for this player only.
        for widget in wanted.into_iter().filter(|w| w.has_data()) {
            if let Some(xml) = self.data_xml(ctx, widget, Some(lang)) {
                Self::send(ctx, vec![login.to_owned()], xml);
            }
        }
    }

    fn is_time_mode(ctx: &PluginContext) -> bool {
        let script = ctx.state.read(|s| s.script.clone());
        !pitwall_formats::mode::mode_of_script(&script).is_some_and(|m| m.rounds)
    }

    fn name_of(ctx: &PluginContext, login: &str, fallback: &str) -> String {
        ctx.state
            .player(login)
            .map(|p| p.nickname)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                if fallback.is_empty() {
                    login.to_owned()
                } else {
                    fallback.to_owned()
                }
            })
    }

    /// Live ranking with its localized header.
    fn ranking(&self, ctx: &PluginContext, lang: Lang) -> RankingData {
        let mut data = self.ranking_rows(ctx);
        data.title = self.ranking_title(ctx, lang);
        data.subtitle = self
            .map_position(ctx)
            .map(|(current, total)| {
                ctx.i18n.render(
                    lang,
                    &Text::tr("ui.map_position")
                        .arg("current", current)
                        .arg("total", total),
                )
            })
            .unwrap_or_default();
        data
    }

    /// Phase of the running cup, else the name of the current mode.
    fn ranking_title(&self, ctx: &PluginContext, lang: Lang) -> String {
        let phase = self
            .cup
            .as_ref()
            .filter(|c| !matches!(c.status, CupStatus::Finished | CupStatus::Aborted))
            .and_then(|c| c.phase.as_ref());
        if let Some(phase) = phase {
            let name = phase.name.get(lang);
            let name = if name.is_empty() { &phase.id } else { name };
            return name.to_uppercase();
        }
        let script = ctx.state.read(|s| s.script.clone());
        pitwall_formats::mode::mode_of_script(&script)
            .and_then(|mode| {
                let formats = self.formats.read().ok()?;
                Some(formats.get(mode.id)?.display_name(lang).to_uppercase())
            })
            .unwrap_or_default()
    }

    /// Position of the current map in the playlist, from 1.
    fn map_position(&self, ctx: &PluginContext) -> Option<(usize, usize)> {
        let playing = ctx
            .state
            .read(|s| s.map.as_ref().map(|m| m.file_name.clone()))?;
        let playlist = &self.maps.playlist;
        let index = playlist.iter().position(|m| m.file_name == playing)?;
        Some((index + 1, playlist.len()))
    }

    fn ranking_rows(&self, ctx: &PluginContext) -> RankingData {
        let cutline = self
            .cup
            .as_ref()
            .filter(|c| {
                matches!(
                    c.status,
                    CupStatus::Running { .. } | CupStatus::Paused { .. }
                )
            })
            .and_then(|c| c.phase.as_ref()?.qualify_top)
            .and_then(|n| i32::try_from(n).ok())
            .unwrap_or(0);
        if Self::is_time_mode(ctx) {
            let mut runs: Vec<(&String, &Run)> = self.best.iter().collect();
            runs.sort_by_key(|(login, run)| (run.time, (*login).clone()));
            let first = runs.first().map(|(_, r)| r.time).unwrap_or_default();
            let rows = runs
                .iter()
                .enumerate()
                .map(|(i, (login, run))| RankingRow {
                    login: (*login).clone(),
                    name: Self::name_of(ctx, login, &run.name),
                    rank: i32::try_from(i + 1).unwrap_or(i32::MAX),
                    time: run.time,
                    points: 0,
                    gap: run.time - first,
                    finished: true,
                })
                .collect();
            return RankingData {
                mode: "time".into(),
                title: String::new(),
                subtitle: String::new(),
                cutline,
                rows,
            };
        }
        let Some(scores) = &self.scores else {
            return RankingData {
                mode: "points".into(),
                title: String::new(),
                subtitle: String::new(),
                cutline,
                rows: Vec::new(),
            };
        };
        let use_match = scores.players.iter().any(|p| p.matchpoints != 0);
        let mut players: Vec<_> = scores.players.iter().collect();
        players.sort_by_key(|p| (p.rank <= 0, p.rank));
        let rows = players
            .iter()
            .enumerate()
            .map(|(i, p)| RankingRow {
                login: p.login.clone(),
                name: Self::name_of(ctx, &p.login, &p.name),
                rank: i32::try_from(i + 1).unwrap_or(i32::MAX),
                time: p.bestracetime.max(0),
                points: if use_match {
                    p.matchpoints
                } else {
                    p.mappoints
                },
                gap: 0,
                finished: self.finished.contains(&p.login),
            })
            .collect();
        RankingData {
            mode: "points".into(),
            title: String::new(),
            subtitle: String::new(),
            cutline,
            rows,
        }
    }

    fn records(&self, ctx: &PluginContext) -> RecordsData {
        let rows = match &self.records {
            Some(rows) => rows.clone(),
            None => {
                let mut runs: Vec<(&String, &Run)> = self.best.iter().collect();
                runs.sort_by_key(|(login, run)| (run.time, (*login).clone()));
                runs.into_iter()
                    .map(|(login, run)| RecordRow {
                        login: login.clone(),
                        name: Self::name_of(ctx, login, &run.name),
                        time: run.time,
                    })
                    .collect()
            }
        };
        RecordsData {
            rows: rows
                .into_iter()
                .take(self.renderer.layout().max_rows)
                .collect(),
        }
    }

    fn cup_info(&self, ctx: &PluginContext, lang: Lang) -> CupInfoData {
        let Some(cup) = &self.cup else {
            return CupInfoData {
                cup: String::new(),
                phase: String::new(),
                status: String::new(),
                ready_check: false,
                ready_logins: Vec::new(),
                ready_done: Vec::new(),
                remaining_ms: 0,
            };
        };
        let phase = match &cup.phase {
            Some(p) => {
                let step = ctx.i18n.render(
                    lang,
                    &Text::tr("ui.phase_step")
                        .arg("current", p.index + 1)
                        .arg("total", cup.phase_count),
                );
                let name = p.name.get(lang);
                format!("{step} · {}", if name.is_empty() { &p.id } else { name })
            }
            None => String::new(),
        };
        let waiting = matches!(cup.status, CupStatus::WaitingStart { .. });
        let (ready_logins, remaining_ms) = match (&self.ready, waiting) {
            (Some((logins, deadline)), true) => (
                logins.clone(),
                i32::try_from((deadline - now_ms()).max(0)).unwrap_or(i32::MAX),
            ),
            _ => (Vec::new(), 0),
        };
        let name = cup.name.get(lang);
        CupInfoData {
            cup: if name.is_empty() {
                cup.id.clone()
            } else {
                name.to_owned()
            },
            phase,
            status: ctx
                .i18n
                .t(lang, &format!("ui.status_{}", cup.status.name())),
            ready_check: !ready_logins.is_empty(),
            ready_logins,
            ready_done: cup.ready.clone(),
            remaining_ms,
        }
    }

    /// Data page of a widget (CupInfo depends on the language).
    fn data_xml(
        &self,
        ctx: &PluginContext,
        widget: WidgetId,
        lang: Option<Lang>,
    ) -> Option<String> {
        let result = match widget {
            WidgetId::Ranking => self
                .renderer
                .data(widget, &self.ranking(ctx, lang.unwrap_or(ctx.default_lang))),
            WidgetId::Records => self.renderer.data(widget, &self.records(ctx)),
            WidgetId::CupInfo => self.renderer.data(
                widget,
                &self.cup_info(ctx, lang.unwrap_or(ctx.default_lang)),
            ),
            WidgetId::AdminPanel | WidgetId::Menu => return None,
        };
        match result {
            Ok(xml) => Some(xml),
            Err(err) => {
                tracing::error!(%err, widget = widget.key(), "cannot render widget data");
                None
            }
        }
    }

    /// Sends the dirty data layers to the players displaying them.
    fn flush(&mut self, ctx: &PluginContext) {
        let dirty = std::mem::take(&mut self.dirty);
        for widget in dirty {
            if widget == WidgetId::AdminPanel {
                // Staff panels change with the cup (buttons): re-send views to staff.
                let staff: Vec<String> = self
                    .sent
                    .iter()
                    .filter(|(_, s)| s.widgets.contains(&WidgetId::AdminPanel))
                    .map(|(l, _)| l.clone())
                    .collect();
                for login in staff {
                    self.send_views(ctx, &login);
                }
                continue;
            }
            // Widgets with localized data get one layer per language.
            let langs: Vec<Option<Lang>> =
                if matches!(widget, WidgetId::CupInfo | WidgetId::Ranking) {
                    Lang::ALL.into_iter().map(Some).collect()
                } else {
                    vec![None]
                };
            for lang in langs {
                let logins: Vec<String> = self
                    .sent
                    .iter()
                    .filter(|(_, s)| {
                        s.widgets.contains(&widget) && lang.is_none_or(|l| s.lang == l)
                    })
                    .map(|(l, _)| l.clone())
                    .collect();
                let Some(xml) = self.data_xml(ctx, widget, lang) else {
                    continue;
                };
                if self.last_data.get(&(widget, lang)) == Some(&xml) {
                    continue;
                }
                self.last_data.insert((widget, lang), xml.clone());
                Self::send(ctx, logins, xml);
            }
        }
    }

    fn mark(&mut self, widgets: &[WidgetId]) {
        self.dirty.extend(widgets.iter().copied());
    }

    fn on_event(&mut self, ctx: &PluginContext, event: Event) {
        match event {
            Event::Connected { .. } => {
                self.fetch_playlist(ctx);
                self.sent.clear();
                self.last_data.clear();
                for login in ctx
                    .state
                    .read(|s| s.players.keys().cloned().collect::<Vec<_>>())
                {
                    self.send_views(ctx, &login);
                }
            }
            Event::PlayerJoined { player } => self.send_views(ctx, &player.login),
            Event::PlayerLeft { player, .. } => {
                self.sent.remove(&player.login);
                self.maps.open.remove(&player.login);
                self.maps.searches.remove(&player.login);
            }
            Event::PlayerUpdated { player } => {
                let role = ctx.roles.get(&player.login);
                let changed = self
                    .sent
                    .get(&player.login)
                    .is_some_and(|s| s.lang != player.lang || s.role != role);
                if changed {
                    self.send_views(ctx, &player.login);
                }
            }
            Event::Finish { waypoint } => {
                if waypoint.racetime > 0 {
                    let better = self
                        .best
                        .get(&waypoint.login)
                        .is_none_or(|run| waypoint.racetime < run.time);
                    if better {
                        let name = Self::name_of(ctx, &waypoint.login, "");
                        self.best.insert(
                            waypoint.login.clone(),
                            Run {
                                name,
                                time: waypoint.racetime,
                            },
                        );
                        self.mark(&[WidgetId::Ranking, WidgetId::Records]);
                    }
                }
                if !Self::is_time_mode(ctx) {
                    self.finished.insert(waypoint.login.clone());
                    self.mark(&[WidgetId::Ranking]);
                }
            }
            Event::Scores { scores } => {
                self.scores = Some(scores);
                self.mark(&[WidgetId::Ranking]);
            }
            Event::RoundStart { .. } => {
                self.finished.clear();
                self.mark(&[WidgetId::Ranking]);
            }
            Event::MapStart { .. } => {
                self.fetch_playlist(ctx);
                self.best.clear();
                self.finished.clear();
                self.records = None;
                self.mark(&[WidgetId::Ranking, WidgetId::Records]);
            }
            Event::ModeChanged { .. } => {
                self.scores = None;
                self.finished.clear();
                self.mark(&[WidgetId::Ranking]);
            }
            Event::Audit { entry } if PLAYLIST_ACTIONS.contains(&entry.action.as_str()) => {
                self.fetch_playlist(ctx);
            }
            Event::Custom {
                source,
                name,
                payload,
            } if source == "maps" && name == "search" => self.on_search(ctx, &payload),
            Event::Custom {
                source,
                name,
                payload,
            } => self.on_custom(&source, &name, payload),
            Event::ManialinkAnswer { login, action, .. } => self.on_click(ctx, &login, &action),
            Event::Command { invocation } if invocation.name == "ui" => {
                self.on_ui_command(ctx, &invocation)
            }
            Event::Command { invocation } if invocation.name == "maps" => {
                self.on_maps_command(ctx, &invocation)
            }
            _ => {}
        }
    }

    fn on_custom(&mut self, source: &str, name: &str, payload: serde_json::Value) {
        match (source, name) {
            ("cup", "state") => {
                let snapshot: Option<CupSnapshot> = serde_json::from_value(payload).unwrap_or(None);
                let loaded_changed = snapshot.is_some() != self.cup.is_some();
                self.cup = snapshot;
                self.mark(&[WidgetId::CupInfo, WidgetId::Ranking]);
                if loaded_changed {
                    self.mark(&[WidgetId::AdminPanel]);
                }
            }
            ("cup", "ready_check") => {
                let logins = payload["logins"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let deadline = payload["deadline_ms"].as_i64().unwrap_or_default();
                self.ready = Some((logins, deadline));
                self.mark(&[WidgetId::CupInfo]);
            }
            ("cup", "ready_check_end") => {
                self.ready = None;
                self.mark(&[WidgetId::CupInfo]);
            }
            ("storage", "records") => {
                let rows: Option<Vec<RecordRow>> = payload["rows"].as_array().map(|rows| {
                    rows.iter()
                        .filter_map(|r| {
                            Some(RecordRow {
                                login: r["login"].as_str()?.to_owned(),
                                name: r["name"].as_str().unwrap_or_default().to_owned(),
                                time: i32::try_from(r["time"].as_i64()?).ok()?,
                            })
                        })
                        .collect()
                });
                self.records = rows;
                self.mark(&[WidgetId::Records]);
            }
            _ => {}
        }
    }

    /// Saves a preference change. The client already applied it.
    fn set_prefs(&mut self, login: &str, update: impl FnOnce(&mut Prefs)) {
        let prefs = self.prefs.entry(login.to_owned()).or_default();
        update(prefs);
        let saved = prefs.clone();
        if saved == Prefs::default() {
            self.prefs.remove(login);
        }
        self.save_prefs();
        tracing::info!(
            login,
            hidden = ?saved.hidden.iter().map(|w| w.key()).collect::<Vec<_>>(),
            all_hidden = saved.all_hidden,
            "UI preferences saved"
        );
    }

    /// Re-sends the menu: it pushes the saved preferences to the client's widgets.
    fn send_menu(&self, ctx: &PluginContext, login: &str) {
        let Some(player) = ctx.state.player(login) else {
            return;
        };
        let role = ctx.roles.get(login);
        match self
            .renderer
            .menu(player.lang, &ctx.i18n, &self.menu_context(login, role))
        {
            Ok(xml) => Self::send(ctx, vec![login.to_owned()], xml),
            Err(err) => tracing::error!(%err, "cannot render the menu"),
        }
    }

    fn on_click(&mut self, ctx: &PluginContext, login: &str, action: &str) {
        if let Some(line) = action.strip_prefix("pw:cmd:") {
            // Role-checked by the command registry, so forged actions are harmless.
            ctx.invoke_as_player(login, line);
            return;
        }
        if action == "pw:maps:close" {
            if self.maps.open.remove(login).is_some() {
                match self.renderer.hide_maps_window() {
                    Ok(xml) => Self::send(ctx, vec![login.to_owned()], xml),
                    Err(err) => tracing::error!(%err, "cannot render the map window"),
                }
            }
            return;
        }
        if let Some(page) = action.strip_prefix("pw:maps:page:") {
            // Only re-renders the window of an admin who has it open.
            if let (Some(open), Ok(page)) = (self.maps.open.get_mut(login), page.parse()) {
                *open = page;
                self.send_maps_window(ctx, login);
            }
            return;
        }
        if let Some(state) = action.strip_prefix("pw:ui:all:") {
            let hidden = state == "1";
            self.set_prefs(login, |p| p.all_hidden = hidden);
        } else if let Some(rest) = action.strip_prefix("pw:ui:set:")
            && let Some((key, state)) = rest.rsplit_once(':')
            && let Some(widget) = WidgetId::parse(key).filter(|w| *w != WidgetId::Menu)
        {
            let hidden = state == "1";
            self.set_prefs(login, |p| {
                if hidden {
                    p.hidden.insert(widget);
                } else {
                    p.hidden.remove(&widget);
                }
            });
        }
    }

    fn on_ui_command(&mut self, ctx: &PluginContext, inv: &Invocation) {
        let Some(login) = inv.login().map(str::to_owned) else {
            return;
        };
        let target = inv.arg(0).unwrap_or("all");
        let on = match inv.arg(1) {
            Some("on") => Some(true),
            Some("off") => Some(false),
            _ => None,
        };
        if target == "all" {
            self.set_prefs(&login, |p| p.all_hidden = !on.unwrap_or(p.all_hidden));
            self.send_menu(ctx, &login);
            return;
        }
        let Some(widget) = WidgetId::parse(target).filter(|w| *w != WidgetId::Menu) else {
            ctx.reply(
                inv,
                Text::tr("core.usage").arg(
                    "usage",
                    "/ui <ranking|cup_info|records|admin_panel|all> [on|off]",
                ),
            );
            return;
        };
        self.set_prefs(&login, |p| {
            let visible = on.unwrap_or(p.hidden.contains(&widget));
            if visible {
                p.hidden.remove(&widget);
            } else {
                p.hidden.insert(widget);
            }
            p.all_hidden = false;
        });
        self.send_menu(ctx, &login);
    }

    /// `/maps [page]`: opens the map manager.
    fn on_maps_command(&mut self, ctx: &PluginContext, inv: &Invocation) {
        let Some(login) = inv.login().map(str::to_owned) else {
            ctx.reply(inv, Text::tr("ui.maps_in_game"));
            return;
        };
        let page = inv
            .arg(0)
            .and_then(|p| p.parse::<usize>().ok())
            .map_or(0, |p| p.saturating_sub(1));
        self.maps.open.insert(login, page);
        self.fetch_playlist(ctx);
    }

    /// Fetches the playlist in the background (map windows, map counter of the ranking); it
    /// comes back through the channel to [`Ui::on_playlist`].
    fn fetch_playlist(&self, ctx: &PluginContext) {
        let commands = ctx.commands.clone();
        let tx = self.maps.tx.clone();
        tokio::spawn(async move {
            let list = commands
                .call(Command::Raw {
                    method: "GetMapList".into(),
                    params: vec![Value::Int(1000), Value::Int(0)],
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| v.deserialize::<Vec<MapInfo>>().map_err(|e| e.to_string()));
            match list {
                Ok(list) => {
                    let _ = tx.send(list);
                }
                Err(err) => tracing::warn!(%err, "cannot fetch the playlist"),
            }
        });
    }

    fn on_playlist(&mut self, ctx: &PluginContext, playlist: Vec<MapInfo>) {
        self.maps.playlist = playlist;
        self.mark(&[WidgetId::Ranking]);
        let logins: Vec<String> = self.maps.open.keys().cloned().collect();
        for login in logins {
            self.send_maps_window(ctx, &login);
        }
    }

    fn on_search(&mut self, ctx: &PluginContext, payload: &serde_json::Value) {
        let Some(login) = payload["login"].as_str() else {
            return;
        };
        let results = payload["results"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        Some(TmxEntry {
                            id: r["id"].as_u64()?,
                            name: r["name"].as_str().unwrap_or_default().to_owned(),
                            author: r["author"].as_str().unwrap_or_default().to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let query = payload["query"].as_str().unwrap_or_default().to_owned();
        self.maps
            .searches
            .insert(login.to_owned(), (query, results));
        if self.maps.open.contains_key(login) {
            self.send_maps_window(ctx, login);
        }
    }

    fn send_maps_window(&mut self, ctx: &PluginContext, login: &str) {
        let Some(player) = ctx.state.player(login) else {
            self.maps.open.remove(login);
            return;
        };
        if ctx.roles.get(login) < Role::Admin {
            self.maps.open.remove(login);
            return;
        }
        let playing = ctx
            .state
            .read(|s| s.map.as_ref().map(|m| m.file_name.clone()));
        let playlist = &self.maps.playlist;
        let (query, results) = self.maps.searches.get(login).cloned().unwrap_or_default();
        let window = MapsWindow {
            maps: playlist
                .iter()
                .map(|m| MapEntry {
                    name: m.name.clone(),
                    author: if m.author_nickname.is_empty() {
                        m.author.clone()
                    } else {
                        m.author_nickname.clone()
                    },
                })
                .collect(),
            current: playing.and_then(|p| playlist.iter().position(|m| m.file_name == p)),
            page: self.maps.open.get(login).copied().unwrap_or_default(),
            query,
            results,
        };
        match self.renderer.maps_window(player.lang, &ctx.i18n, &window) {
            Ok(xml) => Self::send(ctx, vec![login.to_owned()], xml),
            Err(err) => tracing::error!(%err, "cannot render the map window"),
        }
    }
}
