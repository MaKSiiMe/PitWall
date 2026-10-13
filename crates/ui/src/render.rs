//! Renders widget pages.

use askama::Template;
use pitwall_core::{Lang, Role, Translator};
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
#[template(path = "cp_diff.xml")]
struct CpDiffTemplate {
    id: String,
    prefix: String,
    x: String,
    y: String,
    theme: Theme,
    bg_opacity: String,
    vs_label: String,
    good_vec3: String,
    bad_vec3: String,
}

#[derive(Template)]
#[template(path = "cup_info.xml")]
struct CupInfoTemplate {
    id: String,
    prefix: String,
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
    width: String,
    label: String,
    action: String,
    on: bool,
}

#[derive(Template)]
#[template(path = "admin_panel.xml")]
struct AdminTemplate {
    id: String,
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

#[derive(Template)]
#[template(path = "menu.xml")]
struct MenuTemplate {
    id: String,
    x: String,
    y: String,
    theme: Theme,
    bg_rgba: String,
    buttons: Vec<Button>,
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
            WidgetId::CpDiff => CpDiffTemplate {
                id: c.id,
                prefix: c.prefix,
                x: c.x,
                y: c.y,
                theme: c.theme,
                bg_opacity: c.bg_opacity,
                vs_label: ms_string(&t("ui.cp_vs")),
                good_vec3: Theme::vec3(&self.theme.good),
                bad_vec3: Theme::vec3(&self.theme.bad),
            }
            .render()?,
            WidgetId::CupInfo => {
                let pos = self.layout.position(widget);
                CupInfoTemplate {
                    id: c.id,
                    prefix: c.prefix,
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
                    width: real(button_width),
                    label,
                    action: format!("pw:cmd:{command}"),
                    on: true,
                }
            })
            .collect();
        let rows = buttons.len().div_ceil(2) as f32;
        Ok(AdminTemplate {
            id: c.id,
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

    /// Show/hide toggles for one player.
    pub fn menu(
        &self,
        lang: Lang,
        i18n: &Translator,
        ctx: &MenuContext,
    ) -> Result<String, RenderError> {
        let t = |key: &str| i18n.t(lang, key);
        let c = self.common(WidgetId::Menu);
        let mut buttons = Vec::new();
        let mut x = 0.0f32;
        let mut push = |label: String, action: String, on: bool, width: f32| {
            buttons.push(Button {
                x: real(x),
                y: real(0.0),
                width: real(width),
                label,
                action,
                on,
            });
            x += width + 0.6;
        };
        if ctx.all_hidden {
            push(t("ui.menu_show_all"), "pw:ui:showall".into(), true, 24.0);
        } else {
            push(t("ui.menu_hide_all"), "pw:ui:hideall".into(), true, 19.0);
            for (widget, visible) in &ctx.widgets {
                let mark = if *visible { "●" } else { "○" };
                push(
                    format!("{mark} {}", t(&format!("ui.widget.{}", widget.key()))),
                    format!("pw:ui:toggle:{}", widget.key()),
                    *visible,
                    18.0,
                );
            }
        }
        Ok(MenuTemplate {
            id: c.id,
            x: c.x,
            y: c.y,
            theme: c.theme,
            bg_rgba: c.bg_rgba,
            buttons,
        }
        .render()?)
    }
}
