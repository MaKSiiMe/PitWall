//! Admin dashboard (token or session).

use std::time::Duration;

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use pitwall_core::tmtext::strip_formatting;
use pitwall_core::{Actor, ReplyTo, Role};
use pitwall_formats::cup::CupInput;
use serde::Deserialize;

use crate::pages::{Tr, format_date, render, tm_html};
use crate::{ApiState, SESSION_COOKIE, cookie};

/// The web actor of admin actions (audit log).
fn web_actor() -> Actor {
    Actor::Web("admin".to_owned())
}

/// JSON for `hx-vals` running a command line.
fn line(command: &str) -> String {
    serde_json::json!({ "line": command }).to_string()
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    t: Tr,
    enabled: bool,
    failed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LoginForm {
    token: String,
}

pub(crate) async fn login_form(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    render(&LoginPage {
        t: Tr::new(&state, &headers),
        enabled: state.admin_enabled(),
        failed: false,
    })
}

pub(crate) async fn login(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    if !state.check_token(form.token.trim()) {
        // Slow down brute force a little.
        tokio::time::sleep(Duration::from_millis(500)).await;
        return render(&LoginPage {
            t: Tr::new(&state, &headers),
            enabled: state.admin_enabled(),
            failed: true,
        });
    }
    let Some(session) = state.new_session() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no randomness available").into_response();
    };
    (
        [(
            header::SET_COOKIE,
            format!("{SESSION_COOKIE}={session}; Path=/; HttpOnly; SameSite=Strict; Max-Age=43200"),
        )],
        Redirect::to("/admin"),
    )
        .into_response()
}

pub(crate) async fn logout(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Some(session) = cookie(&headers, SESSION_COOKIE) {
        state.drop_session(&session);
    }
    (
        [(
            header::SET_COOKIE,
            format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"),
        )],
        Redirect::to("/"),
    )
        .into_response()
}

/// Redirects to the login page (HTML) or answers 401 (htmx fragments, API).
fn unauthorized(headers: &HeaderMap) -> Response {
    if headers.contains_key("hx-request") {
        return (
            StatusCode::UNAUTHORIZED,
            [("HX-Redirect", "/admin/login")],
            "",
        )
            .into_response();
    }
    Redirect::to("/admin/login").into_response()
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminPage {
    t: Tr,
    cups: Vec<(String, String)>,
    cup_actions: Vec<(String, String)>,
    formats: Vec<(String, String)>,
    game_actions: Vec<(String, String)>,
}

pub(crate) async fn dashboard(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !state.is_admin(&headers) {
        return unauthorized(&headers);
    }
    let t = Tr::new(&state, &headers);
    let cups = state
        .0
        .cups
        .definitions()
        .into_iter()
        .map(|(id, name, phases)| {
            let shown = name.get(t.code());
            (
                id.clone(),
                format!("{} ({phases})", if shown.is_empty() { &id } else { shown }),
            )
        })
        .collect();
    let cup_actions = [
        "open",
        "start",
        "go",
        "pause",
        "resume",
        "advance",
        "retry",
        "status full",
        "abort",
        "unload",
    ]
    .iter()
    .map(|a| ((*a).to_owned(), line(&format!("cup {a}"))))
    .collect();
    let formats = state
        .0
        .formats
        .read()
        .map(|f| {
            f.offered()
                .map(|d| {
                    (
                        d.display_name(t.code()).to_owned(),
                        line(&format!("mode {}", d.id)),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let game_actions = [
        "restart",
        "next",
        "endround",
        "pause",
        "resume",
        "endwarmup",
        "map list",
    ]
    .iter()
    .map(|a| ((*a).to_owned(), line(a)))
    .collect();
    render(&AdminPage {
        t,
        cups,
        cup_actions,
        formats,
        game_actions,
    })
}

struct AdminPlayer {
    name: String,
    login: String,
    role: String,
    lang: String,
    /// (label, hx-vals, dangerous)
    actions: Vec<(String, String, bool)>,
}

#[derive(Template)]
#[template(path = "admin_players.html")]
struct PlayersFragment {
    t: Tr,
    players: Vec<AdminPlayer>,
}

pub(crate) async fn players(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !state.is_admin(&headers) {
        return unauthorized(&headers);
    }
    let players = state
        .handles()
        .state
        .snapshot()
        .players
        .into_values()
        .map(|p| {
            let actions = [
                ("spec", false),
                ("play", false),
                ("free", false),
                ("mute", false),
                ("unmute", false),
                ("kick", true),
                ("ban", true),
            ]
            .iter()
            .map(|(a, danger)| ((*a).to_owned(), line(&format!("{a} {}", p.login)), *danger))
            .collect();
            AdminPlayer {
                name: tm_html(&p.nickname),
                role: state.handles().roles.get(&p.login).as_str().to_owned(),
                lang: p.lang.code().to_owned(),
                login: p.login,
                actions,
            }
        })
        .collect();
    render(&PlayersFragment {
        t: Tr::new(&state, &headers),
        players,
    })
}

struct AuditRow {
    when: String,
    actor: String,
    action: String,
    details: String,
    success: bool,
}

#[derive(Template)]
#[template(path = "admin_audit.html")]
struct AuditFragment {
    t: Tr,
    rows: Vec<AuditRow>,
}

pub(crate) async fn audit(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !state.is_admin(&headers) {
        return unauthorized(&headers);
    }
    let rows = state
        .0
        .storage
        .audit_log(50)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|a| AuditRow {
            when: format_date(a.at),
            actor: a.actor,
            action: a.action,
            details: {
                let text = a.details.to_string();
                text.chars().take(120).collect()
            },
            success: a.success,
        })
        .collect();
    render(&AuditFragment {
        t: Tr::new(&state, &headers),
        rows,
    })
}

struct RegistrationRow {
    id: i64,
    participant_id: String,
    name: String,
    players: String,
    contact: String,
    status: String,
}

#[derive(Template)]
#[template(path = "admin_registrations.html")]
struct RegistrationsFragment {
    t: Tr,
    rows: Vec<RegistrationRow>,
}

pub(crate) async fn registrations(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !state.is_admin(&headers) {
        return unauthorized(&headers);
    }
    let rows = match state.0.cups.snapshot() {
        Some(cup) => state
            .0
            .storage
            .registrations(&cup.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| RegistrationRow {
                id: r.id,
                participant_id: r.participant_id,
                name: strip_formatting(&r.name),
                players: r.players.join(", "),
                contact: r.contact,
                status: r.status,
            })
            .collect(),
        None => Vec::new(),
    };
    render(&RegistrationsFragment {
        t: Tr::new(&state, &headers),
        rows,
    })
}

pub(crate) async fn decide_registration(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path((id, decision)): Path<(i64, String)>,
) -> Response {
    if !state.is_admin(&headers) {
        return unauthorized(&headers);
    }
    let Ok(Some(registration)) = state.0.storage.registration(id).await else {
        return Html("unknown registration".to_owned()).into_response();
    };
    let (input, status) = match decision.as_str() {
        "accept" => (
            CupInput::Approve {
                id: registration.participant_id.clone(),
            },
            "accepted",
        ),
        "refuse" => (
            CupInput::Unregister {
                id: registration.participant_id.clone(),
            },
            "refused",
        ),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let result = state.0.cups.input(input, web_actor()).await;
    if result.is_ok()
        && let Err(err) = state.0.storage.set_registration_status(id, status).await
    {
        tracing::warn!(%err, "cannot update the registration");
    }
    Html(escape(&match result {
        Ok(()) => format!("{} → {status}", registration.name),
        Err(err) => err,
    }))
    .into_response()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommandForm {
    line: String,
}

/// Runs a command line as the web admin and returns the answers (plain text).
pub(crate) async fn command(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Form(form): Form<CommandForm>,
) -> Response {
    if !state.is_admin(&headers) {
        return unauthorized(&headers);
    }
    let lang = Tr::new(&state, &headers).code();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    state.handles().invoke(
        web_actor(),
        Role::SuperAdmin,
        lang,
        &form.line,
        ReplyTo::Channel(tx),
    );
    // Collect answers until the plugin is quiet for a moment (some commands download maps).
    let mut lines = vec![format!("> {}", form.line.trim())];
    let mut wait = Duration::from_secs(4);
    while let Ok(Some(line)) = tokio::time::timeout(wait, rx.recv()).await {
        lines.push(strip_formatting(&line));
        wait = Duration::from_millis(400);
    }
    Html(escape(&lines.join("\n"))).into_response()
}
