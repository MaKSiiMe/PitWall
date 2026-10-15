//! Renders widget pages.

use askama::Template;
use pitwall_core::{Lang, Role, Text, Translator};
use serde::Serialize;

use crate::theme::{Layout, Theme};
use crate::widget::WidgetId;

/// Rendering errors.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("template error: {0}")]
    Template(#[from] askama::Error),
    #[error("cannot serialize widget data: {0}")]
    Json(#[from] serde_json::Error),
}

/// Formats a Manialink/ManiaScript real number (always with a decimal point).
fn real(v: f32) -> String {
    format!("{v:.2}")
}

/// Escapes text for a ManiaScript string literal inside an XML comment.
pub fn ms_string(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ")
        .replace("--", "- -")
}

/// Everything the admin panel needs.
#[derive(Debug, Clone, Default)]
pub struct AdminContext {
    pub role: Role,
    /// `(id, localized name)` of the formats an admin can switch to.
    pub formats: Vec<(String, String)>,
    pub cup_loaded: bool,
}

/// State of the menu for one player.
#[derive(Debug, Clone, Default)]
pub struct MenuContext {
    /// `(widget, visible)` for the widgets this player can toggle.
    pub widgets: Vec<(WidgetId, bool)>,
    pub all_hidden: bool,
}

struct Common {
    id: String,
    prefix: String,
    x: String,
    y: String,
    theme: Theme,
    bg_opacity: String,
    bg_rgba: String,
}

#[derive(Template)]
#[template(path = "data.xml")]
struct DataTemplate<'a> {
    id: String,
    prefix: String,
    json: &'a str,
}

#[derive(Template)]
#[template(path = "hide.xml")]
struct HideTemplate {
    id: String,
}

#[derive(Template)]
#[template(path = "ranking.xml")]
struct RankingTemplate {
    id: String,
    prefix: String,
    hidden_var: String,
    x: String,
    y: String,
    theme: Theme,
    bg_opacity: String,
    title: String,
    width: String,
    title_width: String,
    name_width: String,
    value_x: String,
    row_height: String,
    row_inner: String,
    row_half: String,
    max_rows: usize,
    accent_vec3: String,
    text_vec3: String,
    muted_vec3: String,
}

#[derive(Template)]
#[template(path = "records.xml")]
struct RecordsTemplate {
    id: String,
    prefix: String,
    hidden_var: String,
    x: String,
    y: String,
    theme: Theme,
    bg_opacity: String,
    title: String,
    width: String,
    title_width: String,
    name_width: String,
    value_x: String,
    row_inner: String,
    row_half: String,
    row_offsets: Vec<String>,
    max_rows: usize,
}

#[derive(Template)]
#[template(path = "cup_info.xml")]
struct CupInfoTemplate {
    id: String,
    prefix: String,
    hidden_var: String,
    x: String,
    y: String,
    y_hidden: String,
    theme: Theme,
    bg_opacity: String,
    ready_label: String,
    ready_done_label: String,
    ready_count_label: String,
    good_vec3: String,
    accent_vec3: String,
}

struct Button {
    x: String,
    y: String,
    label: String,
    action: String,
}

#[derive(Template)]
#[template(path = "admin_panel.xml")]
struct AdminTemplate {
    id: String,
    hidden_var: String,
    x: String,
    y: String,
    theme: Theme,
    bg_opacity: String,
    bg_rgba: String,
    title: String,
    width: String,
    title_width: String,
    chevron_x: String,
    body_height: String,
    button_width: String,
    buttons: Vec<Button>,
}

/// One show/hide switch of the menu.
struct Toggle {
    id: String,
    key: &'static str,
    /// `LocalUser` variable read by the widget.
    var: String,
    /// Name as a ManiaScript string literal.
    name_ms: String,
    /// Label shown before the script runs.
    label: String,
    x: String,
    width: String,
    hidden: bool,
}

#[derive(Template)]
#[template(path = "menu.xml")]
struct MenuTemplate {
    id: String,
    x: String,
    y: String,
    theme: Theme,
    bg_rgba: String,
    text_vec3: String,
    muted_vec3: String,
    hide_all_label: String,
    hide_all_width: String,
    show_all_label: String,
    all_hidden: bool,
    toggles: Vec<Toggle>,
}

/// Manialink id of the map manager window.
pub const MAPS_WINDOW_ID: &str = "PitWall_Maps";
/// Playlist rows per page of the map manager.
pub const MAPS_PER_PAGE: usize = 15;
/// TMX search results shown in the map manager.
pub const MAPS_MAX_RESULTS: usize = 7;

/// A playlist map in the map manager.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapEntry {
    pub name: String,
    pub author: String,
}

/// A TMX search result in the map manager.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TmxEntry {
    pub id: u64,
    pub name: String,
    pub author: String,
}

/// Everything the map manager window shows to one admin.
#[derive(Debug, Clone, Default)]
pub struct MapsWindow {
    /// The whole playlist, in order.
    pub maps: Vec<MapEntry>,
    /// Index of the map being played.
    pub current: Option<usize>,
    /// Page shown, from 0 (clamped).
    pub page: usize,
    /// Last TMX search of this admin, if any.
    pub query: String,
    pub results: Vec<TmxEntry>,
}

impl MapsWindow {
    pub fn pages(&self) -> usize {
        self.maps.len().div_ceil(MAPS_PER_PAGE).max(1)
    }
}

struct RowButton {
    x: String,
    label: &'static str,
    action: String,
    color: String,
}

struct MapLine {
    y: String,
    number: usize,
    name: String,
    author: String,
    current: bool,
    odd: bool,
    buttons: Vec<RowButton>,
}

struct ResultLine {
    y: String,
    name: String,
    author: String,
    add: String,
    add_next: String,
}

#[derive(Template)]
#[template(path = "maps_window.xml")]
struct MapsWindowTemplate {
    id: &'static str,
    theme: Theme,
    bg_opacity: String,
    bg_rgba: String,
    title: String,
    rows: Vec<MapLine>,
    empty_label: String,
    pages: usize,
    page_label: String,
    prev_action: Option<String>,
    next_action: Option<String>,
    clear_label: String,
    clear_label_ms: String,
    clear_confirm_ms: String,
    legend: String,
    add_title: String,
    add_label: String,
    add_next_label: String,
    search_title: String,
    search_label: String,
    query: String,
    results: Vec<ResultLine>,
    no_result: bool,
    no_result_label: String,
}

/// Renders pages with a theme and a layout.
#[derive(Debug, Clone, Default)]
pub struct Renderer {
    theme: Theme,
    layout: Layout,
}

const ROW_HEIGHT: f32 = 4.4;
const WIDGET_WIDTH: f32 = 44.0;

impl Renderer {
    pub fn new(theme: Theme, layout: Layout) -> Renderer {
        Renderer {
            theme: theme.sanitized(),
            layout,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    fn common(&self, widget: WidgetId) -> Common {
        let pos = self.layout.position(widget);
        let bg = &self.theme.background;
        // Focus colors take RGBA: expand 3-digit colors, add ~80% alpha.
        let bg_rgba = if bg.len() == 3 {
            format!("{bg}c")
        } else {
            format!("{bg}cc")
        };
        Common {
            id: widget.view_id(),
            prefix: widget.var_prefix(),
            x: real(pos.x),
            y: real(pos.y),
            theme: self.theme.clone(),
            bg_opacity: real(self.theme.background_opacity),
            bg_rgba,
        }
    }

    /// Empty page with the widget's id: removes it from the screen.
    pub fn hide(&self, widget: WidgetId) -> Result<String, RenderError> {
        Ok(HideTemplate {
            id: widget.view_id(),
        }
        .render()?)
    }

    /// Data layer carrying `data` as JSON.
    pub fn data<T: Serialize>(&self, widget: WidgetId, data: &T) -> Result<String, RenderError> {
        let json = serde_json::to_string(data)?;
        Ok(DataTemplate {
            id: widget.data_id(),
            prefix: widget.var_prefix(),
            json: &json,
        }
        .render()?)
    }

    /// View layer of a data widget, in `lang`.
    pub fn view(
        &self,
        widget: WidgetId,
        lang: Lang,
        i18n: &Translator,
    ) -> Result<String, RenderError> {
        let t = |key: &str| i18n.t(lang, key);
        let c = self.common(widget);
        let max_rows = self.layout.max_rows.clamp(1, 32);
        let row_inner = real(ROW_HEIGHT - 0.4);
        let row_half = real((ROW_HEIGHT - 0.4) / 2.0);
        Ok(match widget {
            WidgetId::Ranking => RankingTemplate {
                id: c.id,
                prefix: c.prefix,
                hidden_var: widget.hidden_var(),
                x: c.x,
                y: c.y,
                theme: c.theme,
                bg_opacity: c.bg_opacity,
                title: t("ui.ranking_title"),
                width: real(WIDGET_WIDTH),
                title_width: real(WIDGET_WIDTH - 4.0),
                name_width: real(WIDGET_WIDTH - 6.5 - 14.0),
                value_x: real(WIDGET_WIDTH - 1.0),
                row_height: real(ROW_HEIGHT),
                row_inner,
                row_half,
                max_rows,
                accent_vec3: Theme::vec3(&self.theme.accent),
                text_vec3: Theme::vec3(&self.theme.text),
                muted_vec3: Theme::vec3(&self.theme.muted),
            }
            .render()?,
            WidgetId::Records => RecordsTemplate {
                id: c.id,
                prefix: c.prefix,
                hidden_var: widget.hidden_var(),
                x: c.x,
                y: c.y,
                theme: c.theme,
                bg_opacity: c.bg_opacity,
                title: t("ui.records_title"),
                width: real(WIDGET_WIDTH),
                title_width: real(WIDGET_WIDTH - 4.0),
                name_width: real(WIDGET_WIDTH - 6.5 - 14.0),
                value_x: real(WIDGET_WIDTH - 1.0),
                row_inner,
                row_half,
                row_offsets: (0..max_rows).map(|i| real(ROW_HEIGHT * i as f32)).collect(),
                max_rows,
            }
            .render()?,
            WidgetId::CupInfo => {
                let pos = self.layout.position(widget);
                CupInfoTemplate {
                    id: c.id,
                    prefix: c.prefix,
                    hidden_var: widget.hidden_var(),
                    x: c.x,
                    y: c.y,
                    y_hidden: real(pos.y + 12.0),
                    theme: c.theme,
                    bg_opacity: c.bg_opacity,
                    ready_label: ms_string(&t("ui.ready")),
                    ready_done_label: ms_string(&t("ui.ready_done")),
                    ready_count_label: ms_string(&t("ui.ready_count")),
                    good_vec3: Theme::vec3(&self.theme.good),
                    accent_vec3: Theme::vec3(&self.theme.accent),
                }
                .render()?
            }
            WidgetId::AdminPanel => self.admin_panel(lang, i18n, &AdminContext::default())?,
            WidgetId::Menu => self.menu(lang, i18n, &MenuContext::default())?,
        })
    }

    /// Admin panel for one role. Buttons send `pw:cmd:<command line>`; the command registry checks
    /// the role again when the click arrives, so a forged action cannot bypass permissions.
    pub fn admin_panel(
        &self,
        lang: Lang,
        i18n: &Translator,
        ctx: &AdminContext,
    ) -> Result<String, RenderError> {
        let t = |key: &str| i18n.t(lang, key);
        let c = self.common(WidgetId::AdminPanel);
        let mut entries: Vec<(String, String)> = Vec::new();
        if ctx.role >= Role::Referee {
            entries.push((t("ui.btn_pause"), "pause".into()));
            entries.push((t("ui.btn_resume"), "resume".into()));
            entries.push((t("ui.btn_endround"), "endround".into()));
            entries.push((t("ui.btn_restart"), "restart".into()));
            if ctx.cup_loaded {
                entries.push((t("ui.btn_cup_go"), "cup go".into()));
                entries.push((t("ui.btn_cup_status"), "cup status".into()));
            }
        }
        if ctx.role >= Role::Admin {
            entries.push((t("ui.btn_next"), "next".into()));
            entries.push((t("ui.btn_maps"), "maps".into()));
            entries.push((t("ui.btn_endwarmup"), "endwarmup".into()));
            if ctx.cup_loaded {
                entries.push((t("ui.btn_cup_start"), "cup start".into()));
                entries.push((t("ui.btn_cup_advance"), "cup advance".into()));
            }
            for (id, name) in &ctx.formats {
                entries.push((format!("» {name}"), format!("mode {id}")));
            }
        }
        let width = WIDGET_WIDTH;
        let button_width = (width - 3.0) / 2.0;
        let buttons: Vec<Button> = entries
            .into_iter()
            .enumerate()
            .map(|(i, (label, command))| {
                let col = (i % 2) as f32;
                let row = (i / 2) as f32;
                Button {
                    x: real(1.0 + button_width / 2.0 + col * (button_width + 1.0)),
                    y: real(5.5 + 2.2 + row * 4.9),
                    label,
                    action: format!("pw:cmd:{command}"),
                }
            })
            .collect();
        let rows = buttons.len().div_ceil(2) as f32;
        Ok(AdminTemplate {
            id: c.id,
            hidden_var: WidgetId::AdminPanel.hidden_var(),
            x: c.x,
            y: c.y,
            theme: c.theme,
            bg_opacity: c.bg_opacity,
            bg_rgba: c.bg_rgba,
            title: t("ui.admin_title"),
            width: real(width),
            title_width: real(width - 8.0),
            chevron_x: real(width - 3.0),
            body_height: real(rows * 4.9 + 1.5),
            button_width: real(button_width),
            buttons,
        }
        .render()?)
    }

    /// Map manager window. Buttons send `pw:cmd:map …` (role-checked on arrival) and
    /// `pw:maps:page:<n>` / `pw:maps:close`, handled by the UI plugin.
    pub fn maps_window(
        &self,
        lang: Lang,
        i18n: &Translator,
        window: &MapsWindow,
    ) -> Result<String, RenderError> {
        let t = |key: &str| i18n.t(lang, key);
        let c = self.common(WidgetId::AdminPanel);
        let total = window.maps.len();
        let pages = window.pages();
        let page = window.page.min(pages - 1);
        let first = page * MAPS_PER_PAGE;
        let rows = window
            .maps
            .iter()
            .enumerate()
            .skip(first)
            .take(MAPS_PER_PAGE)
            .map(|(i, map)| {
                let n = i + 1;
                let current = window.current == Some(i);
                let mut buttons = Vec::new();
                let mut button = |slot: usize, label, action: String, color: &str| {
                    buttons.push(RowButton {
                        x: real(81.0 + slot as f32 * 4.2),
                        label,
                        action: format!("pw:cmd:map {action}"),
                        color: color.to_owned(),
                    });
                };
                let accent = &self.theme.accent;
                if n > 1 {
                    button(0, "▲", format!("move {n} {}", n - 1), accent);
                }
                if n < total {
                    button(1, "▼", format!("move {n} {}", n + 1), accent);
                }
                if !current {
                    button(2, "»", format!("next {n}"), accent);
                    button(3, "▶", format!("play {n}"), accent);
                }
                button(4, "✕", format!("remove {n}"), &self.theme.bad);
                MapLine {
                    y: real(-8.0 - (i - first) as f32 * 5.8),
                    number: n,
                    name: map.name.clone(),
                    author: map.author.clone(),
                    current,
                    odd: (i - first) % 2 == 1,
                    buttons,
                }
            })
            .collect();
        let results = window
            .results
            .iter()
            .take(MAPS_MAX_RESULTS)
            .enumerate()
            .map(|(i, r)| ResultLine {
                y: real(-49.0 - i as f32 * 7.0),
                name: r.name.clone(),
                author: r.author.clone(),
                add: format!("pw:cmd:map tmx {}", r.id),
                add_next: format!("pw:cmd:map tmx {} next", r.id),
            })
            .collect();
        let clear_label = t("ui.maps_clear");
        Ok(MapsWindowTemplate {
            id: MAPS_WINDOW_ID,
            theme: c.theme,
            bg_opacity: real(self.theme.background_opacity.max(0.9)),
            bg_rgba: c.bg_rgba,
            title: i18n.render(lang, &Text::tr("ui.maps_title").arg("count", total)),
            rows,
            empty_label: t("ui.maps_empty"),
            pages,
            page_label: i18n.render(
                lang,
                &Text::tr("ui.maps_page")
                    .arg("page", page + 1)
                    .arg("pages", pages),
            ),
            prev_action: (page > 0).then(|| format!("pw:maps:page:{}", page - 1)),
            next_action: (page + 1 < pages).then(|| format!("pw:maps:page:{}", page + 1)),
            clear_label_ms: ms_string(&clear_label),
            clear_label,
            clear_confirm_ms: ms_string(&t("ui.maps_clear_confirm")),
            legend: t("ui.maps_legend"),
            add_title: t("ui.maps_add_title"),
            add_label: t("ui.maps_add"),
            add_next_label: t("ui.maps_add_next"),
            search_title: t("ui.maps_search_title"),
            search_label: t("ui.maps_search"),
            query: window.query.clone(),
            no_result: !window.query.is_empty() && window.results.is_empty(),
            no_result_label: t("ui.maps_no_result"),
            results,
        }
        .render()?)
    }

    /// Empty page closing the map manager.
    pub fn hide_maps_window(&self) -> Result<String, RenderError> {
        Ok(HideTemplate {
            id: MAPS_WINDOW_ID.to_owned(),
        }
        .render()?)
    }

    /// Show/hide switches for one player. Clicks are handled on the client (instant): the
    /// widgets read `LocalUser` variables, and the server is only told so it can save the choice.
    /// Re-sending this page applies the server's saved preferences again.
    pub fn menu(
        &self,
        lang: Lang,
        i18n: &Translator,
        ctx: &MenuContext,
    ) -> Result<String, RenderError> {
        let t = |key: &str| i18n.t(lang, key);
        let c = self.common(WidgetId::Menu);
        const HIDE_ALL_WIDTH: f32 = 19.0;
        const TOGGLE_WIDTH: f32 = 18.0;
        let mut x = HIDE_ALL_WIDTH + 0.6;
        let toggles = ctx
            .widgets
            .iter()
            .map(|(widget, visible)| {
                let name = t(&format!("ui.widget.{}", widget.key()));
                let mark = if *visible { "●" } else { "○" };
                let toggle = Toggle {
                    id: format!("pw-menu-{}", widget.key()),
                    key: widget.key(),
                    var: widget.hidden_var(),
                    name_ms: ms_string(&name),
                    label: format!("{mark} {name}"),
                    x: real(x),
                    width: real(TOGGLE_WIDTH),
                    hidden: !*visible,
                };
                x += TOGGLE_WIDTH + 0.6;
                toggle
            })
            .collect();
        Ok(MenuTemplate {
            id: c.id,
            x: c.x,
            y: c.y,
            theme: c.theme,
            bg_rgba: c.bg_rgba,
            text_vec3: Theme::vec3(&self.theme.text),
            muted_vec3: Theme::vec3(&self.theme.muted),
            hide_all_label: t("ui.menu_hide_all"),
            hide_all_width: real(HIDE_ALL_WIDTH),
            show_all_label: t("ui.menu_show_all"),
            all_hidden: ctx.all_hidden,
            toggles,
        }
        .render()?)
    }
}
