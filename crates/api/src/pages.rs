//! Public pages: server status, live cup, registration, history.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use pitwall_core::{Actor, Lang, Translator};
use pitwall_formats::cup::{CupInput, CupKind, CupSnapshot, CupStatus, RegistrationMode, Standing};
use pitwall_storage::Registration;
use serde::Deserialize;

use crate::{ApiState, LANG_COOKIE, request_lang};

/// Translations for templates: `{{ t.get("web.key") }}`.
pub(crate) struct Tr {
    i18n: Translator,
    lang: Lang,
}

impl Tr {
    pub(crate) fn new(state: &ApiState, headers: &HeaderMap) -> Tr {
        Tr {
            i18n: state.handles().i18n.clone(),
            lang: request_lang(state, headers),
        }
    }

    pub(crate) fn get(&self, key: &str) -> String {
        self.i18n.t(self.lang, key)
    }

    pub(crate) fn lang(&self) -> &'static str {
        self.lang.code()
    }

    pub(crate) fn code(&self) -> Lang {
        self.lang
    }
}

/// Renders a template or a 500.
pub(crate) fn render<T: Template>(template: &T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => {
            tracing::error!(%err, "template error");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Converts Trackmania formatting (colors) to safe HTML; other codes are dropped.
pub(crate) fn tm_html(text: &str) -> String {
    let mut out = String::new();
    let mut open = false;
    let mut buffer = String::new();
    let mut chars = text.chars().peekable();
    let flush = |buffer: &mut String, out: &mut String| {
        out.push_str(&escape_html(buffer));
        buffer.clear();
    };
    while let Some(c) = chars.next() {
        if c != '$' {
            buffer.push(c);
            continue;
        }
        match chars.next() {
            Some('$') => buffer.push('$'),
            Some(h) if h.is_ascii_hexdigit() => {
                let mut hex = String::from(h);
                for _ in 0..2 {
                    if let Some(d) = chars.peek().filter(|d| d.is_ascii_hexdigit()) {
                        hex.push(*d);
                        chars.next();
                    }
                }
                flush(&mut buffer, &mut out);
                if open {
                    out.push_str("</span>");
                }
                let full: String = hex.chars().flat_map(|c| [c, c]).collect();
                let full = format!("{full:0<6}");
                out.push_str(&format!("<span style=\"color:#{full}\">"));
                open = true;
            }
            Some('z' | 'Z' | 'g' | 'G') => {
                flush(&mut buffer, &mut out);
                if open {
                    out.push_str("</span>");
                    open = false;
                }
            }
            Some('l' | 'L' | 'h' | 'H' | 'p' | 'P') if chars.peek() == Some(&'[') => {
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut buffer, &mut out);
    if open {
        out.push_str("</span>");
    }
    out
}

/// `YYYY-MM-DD HH:MM` (UTC) from Unix milliseconds.
pub(crate) fn format_date(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn status_label(t: &Tr, status: &CupStatus) -> String {
    t.get(&format!("ui.status_{}", status.name()))
}

// --- index ---------------------------------------------------------------------------------------

pub(crate) struct PlayerView {
    pub(crate) name: String,
    pub(crate) zone: String,
    pub(crate) spectator: bool,
}

pub(crate) struct CupSummary {
    name: String,
    status: String,
    phase: String,
    registration_open: bool,
}

pub(crate) struct RatedView {
    name: String,
    rating: String,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    t: Tr,
    connected: bool,
    map: String,
    mode: String,
    racing: usize,
    spectators: usize,
    players: Vec<PlayerView>,
    cup: Option<CupSummary>,
    leaderboard: Vec<RatedView>,
}

fn registration_open(cup: &CupSnapshot) -> bool {
    cup.registration.contains(&RegistrationMode::Web)
        && matches!(cup.status, CupStatus::Draft | CupStatus::Registration)
}

fn phase_line(t: &Tr, cup: &CupSnapshot) -> String {
    match &cup.phase {
        Some(p) => {
            let name = p.name.get(t.code());
            format!(
                "{} {}/{} · {}",
                t.get("web.phase"),
                p.index + 1,
                cup.phase_count,
                if name.is_empty() { &p.id } else { name }
            )
        }
        None => String::new(),
    }
}

fn cup_name(t: &Tr, cup: &CupSnapshot) -> String {
    let name = cup.name.get(t.code());
    if name.is_empty() {
        cup.id.clone()
    } else {
        name.to_owned()
    }
}

pub(crate) async fn index(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let t = Tr::new(&state, &headers);
    let snapshot = state.handles().state.snapshot();
    let mode = pitwall_formats::mode::mode_of_script(&snapshot.script)
        .map(|m| m.id.to_owned())
        .unwrap_or_else(|| snapshot.script.clone());
    let players: Vec<PlayerView> = snapshot
        .players
        .values()
        .map(|p| PlayerView {
            name: tm_html(&p.nickname),
            zone: p.zone.rsplit('|').next().unwrap_or_default().to_owned(),
            spectator: p.spectator,
        })
        .collect();
    let cup = state.0.cups.snapshot().map(|c| CupSummary {
        name: cup_name(&t, &c),
        status: status_label(&t, &c.status),
        phase: phase_line(&t, &c),
        registration_open: registration_open(&c),
    });
    let leaderboard = state
        .0
        .storage
        .leaderboard(10)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| RatedView {
            name: tm_html(&p.nickname),
            rating: format!("{:.0}", p.rating),
        })
        .collect();
    render(&IndexPage {
        connected: snapshot.connected,
        map: snapshot
            .map
            .as_ref()
            .map(|m| tm_html(&m.name))
            .unwrap_or_else(|| t.get("web.none")),
        mode,
        racing: players.iter().filter(|p| !p.spectator).count(),
        spectators: players.iter().filter(|p| p.spectator).count(),
        players,
        cup,
        leaderboard,
        t,
    })
}

pub(crate) async fn set_lang(Path(lang): Path<String>, headers: HeaderMap) -> Response {
    let lang = Lang::from_code(&lang).unwrap_or_default();
    let back = headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .filter(|r| r.starts_with('/') || r.starts_with("http"))
        .unwrap_or("/")
        .to_owned();
    (
        [(
            header::SET_COOKIE,
            format!(
                "{LANG_COOKIE}={}; Path=/; Max-Age=31536000; SameSite=Lax",
                lang.code()
            ),
        )],
        Redirect::to(&back),
    )
        .into_response()
}

// --- live cup ------------------------------------------------------------------------------------

pub(crate) struct ResultRow {
    pub(crate) rank: u32,
    pub(crate) name: String,
    pub(crate) points: i32,
    pub(crate) time: String,
    pub(crate) qualified: bool,
}

pub(crate) struct PhaseView {
    pub(crate) name: String,
    pub(crate) rows: Vec<ResultRow>,
}

struct MatchView {
    a: String,
    b: String,
    winner: String,
}

struct CupView {
    name: String,
    status: String,
    phase: String,
    racing: Vec<String>,
    ready: Vec<String>,
    bracket: Vec<Vec<MatchView>>,
    results: Vec<PhaseView>,
    participants: Vec<String>,
}

#[derive(Template)]
#[template(path = "cup.html")]
struct CupPage {
    t: Tr,
}

#[derive(Template)]
#[template(path = "cup_live.html")]
struct CupLive {
    t: Tr,
    cup: Option<CupView>,
}

pub(crate) fn result_rows(
    standings: &[Standing],
    qualified: &[String],
    name_of: impl Fn(&str) -> String,
) -> Vec<ResultRow> {
    standings
        .iter()
        .map(|s| ResultRow {
            rank: s.rank,
            name: tm_html(&if s.name.is_empty() || s.name == s.id {
                name_of(&s.id)
            } else {
                s.name.clone()
            }),
            points: s.points,
            time: s
                .best_time
                .map(pitwall_integrations::export::format_time)
                .unwrap_or_default(),
            qualified: qualified.contains(&s.id),
        })
        .collect()
}

pub(crate) async fn cup(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    render(&CupPage {
        t: Tr::new(&state, &headers),
    })
}

pub(crate) async fn cup_live(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let t = Tr::new(&state, &headers);
    let cup = state.0.cups.snapshot().map(|c| {
        let name_of = |id: &str| {
            c.participants
                .iter()
                .find(|p| p.id == id)
                .map_or_else(|| id.to_owned(), |p| p.name.clone())
        };
        let login_name = |login: &str| {
            state
                .handles()
                .state
                .player(login)
                .map_or_else(|| name_of(login), |p| p.nickname)
        };
        let phase_name = |id: &str| {
            c.phase
                .as_ref()
                .filter(|p| p.id == id)
                .map(|p| p.name.get(t.code()).to_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id.to_owned())
        };
        CupView {
            name: cup_name(&t, &c),
            status: status_label(&t, &c.status),
            phase: phase_line(&t, &c),
            racing: c.racing.iter().map(|l| tm_html(&login_name(l))).collect(),
            ready: c.ready.clone(),
            bracket: c
                .bracket
                .as_ref()
                .map(|b| {
                    b.rounds
                        .iter()
                        .map(|round| {
                            round
                                .iter()
                                .map(|m| MatchView {
                                    a: m.a
                                        .as_deref()
                                        .map(|id| tm_html(&name_of(id)))
                                        .unwrap_or_else(|| "—".into()),
                                    b: m.b
                                        .as_deref()
                                        .map(|id| tm_html(&name_of(id)))
                                        .unwrap_or_else(|| "—".into()),
                                    winner: m
                                        .winner
                                        .as_deref()
                                        .map(|id| tm_html(&name_of(id)))
                                        .unwrap_or_default(),
                                })
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default(),
            results: c
                .results
                .iter()
                .rev()
                .map(|r| PhaseView {
                    name: phase_name(&r.phase_id),
                    rows: result_rows(&r.standings, &r.qualified, name_of),
                })
                .collect(),
            participants: c.participants.iter().map(|p| tm_html(&p.name)).collect(),
        }
    });
    render(&CupLive { t, cup })
}

// --- registration --------------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "register.html")]
struct RegisterPage {
    t: Tr,
    cup_name: String,
    open: bool,
    teams: bool,
    message: String,
    success: bool,
}

/// Registration form fields.
#[derive(Debug, Deserialize)]
pub(crate) struct RegisterForm {
    id: String,
    name: String,
    #[serde(default)]
    players: String,
    #[serde(default)]
    contact: String,
}

fn register_page(
    state: &ApiState,
    headers: &HeaderMap,
    message: String,
    success: bool,
) -> Response {
    let t = Tr::new(state, headers);
    let cup = state.0.cups.snapshot();
    render(&RegisterPage {
        cup_name: cup.as_ref().map(|c| cup_name(&t, c)).unwrap_or_default(),
        open: cup.as_ref().is_some_and(registration_open),
        teams: cup.as_ref().is_some_and(|c| c.kind == CupKind::Teams),
        message,
        success,
        t,
    })
}

pub(crate) async fn register_form(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    register_page(&state, &headers, String::new(), false)
}

fn valid_id(id: &str) -> bool {
    (2..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(crate) async fn register(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Response {
    let t = Tr::new(&state, &headers);
    let error = |message: String| {
        t.i18n.render(
            t.code(),
            &pitwall_core::Text::tr("web.register_error").arg("message", message),
        )
    };
    let Some(cup) = state.0.cups.snapshot().filter(registration_open) else {
        return register_page(&state, &headers, t.get("web.register_closed"), false);
    };
    let id = form.id.trim().to_owned();
    let name = form.name.trim().chars().take(40).collect::<String>();
    let players: Vec<String> = if cup.kind == CupKind::Teams {
        form.players
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    if !valid_id(&id)
        || name.is_empty()
        || players.iter().any(|p| !valid_id(p))
        || (cup.kind == CupKind::Teams && players.is_empty())
    {
        return register_page(&state, &headers, error("invalid fields".into()), false);
    }
    let result = state
        .0
        .cups
        .input(
            CupInput::Register {
                id: id.clone(),
                name: name.clone(),
                players: players.clone(),
                via: RegistrationMode::Web,
            },
            Actor::Web(id.clone()),
        )
        .await;
    if let Err(message) = result {
        return register_page(&state, &headers, error(message), false);
    }
    let pending = state
        .0
        .cups
        .snapshot()
        .and_then(|c| c.participants.into_iter().find(|p| p.id == id))
        .is_some_and(|p| p.status == pitwall_formats::cup::engine::ParticipantStatus::Pending);
    let record = Registration {
        id: 0,
        cup_id: cup.id.clone(),
        participant_id: id,
        name,
        players: if players.is_empty() {
            Vec::new()
        } else {
            players
        },
        via: "web".into(),
        status: if pending { "pending" } else { "accepted" }.into(),
        contact: form.contact.chars().take(80).collect(),
        created_at: pitwall_core::now_ms(),
    };
    if let Err(err) = state.0.storage.add_registration(&record).await {
        tracing::warn!(%err, "cannot store the registration");
    }
    let key = if pending {
        "web.registered_pending"
    } else {
        "web.registered_ok"
    };
    register_page(&state, &headers, t.get(key), true)
}

// --- history -------------------------------------------------------------------------------------

struct RunView {
    id: i64,
    name: String,
    status: String,
    date: String,
}

#[derive(Template)]
#[template(path = "history.html")]
struct HistoryPage {
    t: Tr,
    runs: Vec<RunView>,
}

#[derive(Template)]
#[template(path = "history_detail.html")]
struct HistoryDetailPage {
    t: Tr,
    id: i64,
    name: String,
    phases: Vec<PhaseView>,
}

pub(crate) async fn history(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let t = Tr::new(&state, &headers);
    let runs = state
        .0
        .storage
        .cup_history(100)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| RunView {
            id: r.id,
            name: r.name,
            status: r.status,
            date: format_date(r.started_at),
        })
        .collect();
    render(&HistoryPage { t, runs })
}

pub(crate) async fn history_detail(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let t = Tr::new(&state, &headers);
    let Ok(Some(run)) = state.0.storage.cup_run(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let phases = state
        .0
        .storage
        .phase_results(id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let standings: Vec<Standing> = serde_json::from_value(p.standings).unwrap_or_default();
            PhaseView {
                name: p.phase_id,
                rows: result_rows(&standings, &p.qualified, str::to_owned),
            }
        })
        .collect();
    render(&HistoryDetailPage {
        t,
        id,
        name: run.name,
        phases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trackmania_colors_to_html() {
        assert_eq!(
            tm_html("$f00Red$zWhite"),
            "<span style=\"color:#ff0000\">Red</span>White"
        );
        assert_eq!(tm_html("<script>$$"), "&lt;script&gt;$");
        assert_eq!(
            tm_html("$o$0afPit$fffWall"),
            "<span style=\"color:#00aaff\">Pit</span><span style=\"color:#ffffff\">Wall</span>"
        );
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(0), "1970-01-01 00:00");
        assert_eq!(format_date(1_791_386_727_873), "2026-10-07 15:25");
    }
}
