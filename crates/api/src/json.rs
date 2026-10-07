//! Public JSON API and exports.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use pitwall_formats::cup::Standing;
use pitwall_integrations::export::{ExportRow, to_csv};
use serde::Serialize;

use crate::ApiState;

#[derive(Serialize)]
struct PublicPlayer {
    login: String,
    nickname: String,
    zone: String,
    team: i32,
    spectator: bool,
}

#[derive(Serialize)]
struct PublicState {
    connected: bool,
    map: Option<pitwall_core::gbx::types::MapInfo>,
    script: String,
    players: Vec<PublicPlayer>,
    cup: Option<pitwall_formats::cup::CupSnapshot>,
}

pub(crate) async fn state(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let snapshot = state.handles().state.snapshot();
    let public = PublicState {
        connected: snapshot.connected,
        map: snapshot.map,
        script: snapshot.script,
        players: snapshot
            .players
            .into_values()
            .map(|p| PublicPlayer {
                login: p.login,
                nickname: p.nickname,
                zone: p.zone,
                team: p.team,
                spectator: p.spectator,
            })
            .collect(),
        cup: state.0.cups.snapshot(),
    };
    Json(serde_json::to_value(public).unwrap_or_default())
}

pub(crate) async fn cup(
    State(state): State<ApiState>,
) -> Json<Option<pitwall_formats::cup::CupSnapshot>> {
    Json(state.0.cups.snapshot())
}

fn internal(err: impl std::fmt::Display) -> Response {
    tracing::error!(%err, "API error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

pub(crate) async fn history(State(state): State<ApiState>) -> Response {
    match state.0.storage.cup_history(100).await {
        Ok(runs) => Json(runs).into_response(),
        Err(err) => internal(err),
    }
}

pub(crate) async fn history_detail(State(state): State<ApiState>, Path(id): Path<i64>) -> Response {
    let run = match state.0.storage.cup_run(id).await {
        Ok(Some(run)) => run,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => return internal(err),
    };
    match state.0.storage.phase_results(id).await {
        Ok(phases) => Json(serde_json::json!({ "run": run, "phases": phases })).into_response(),
        Err(err) => internal(err),
    }
}

#[allow(clippy::result_large_err)] // the error is the ready-made HTTP response
async fn export_rows(
    state: &ApiState,
    id: i64,
) -> Result<Option<(String, Vec<ExportRow>)>, Response> {
    let run = match state.0.storage.cup_run(id).await {
        Ok(Some(run)) => run,
        Ok(None) => return Ok(None),
        Err(err) => return Err(internal(err)),
    };
    let phases = state.0.storage.phase_results(id).await.map_err(internal)?;
    let mut rows = Vec::new();
    for phase in phases {
        let standings: Vec<Standing> = serde_json::from_value(phase.standings).unwrap_or_default();
        for s in standings {
            rows.push(ExportRow {
                phase: phase.phase_id.clone(),
                rank: s.rank,
                qualified: phase.qualified.contains(&s.id),
                id: s.id,
                name: pitwall_core::tmtext::strip_formatting(&s.name),
                points: s.points,
                best_time: s.best_time,
            });
        }
    }
    Ok(Some((run.cup_id, rows)))
}

fn attachment(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("attachment; filename=\"{safe}\"")
}

pub(crate) async fn export_csv(State(state): State<ApiState>, Path(id): Path<i64>) -> Response {
    match export_rows(&state, id).await {
        Ok(Some((cup, rows))) => (
            [
                (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    attachment(&format!("{cup}-{id}.csv")),
                ),
            ],
            to_csv(&rows),
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(response) => response,
    }
}

pub(crate) async fn export_json(State(state): State<ApiState>, Path(id): Path<i64>) -> Response {
    match export_rows(&state, id).await {
        Ok(Some((cup, rows))) => (
            [(
                header::CONTENT_DISPOSITION,
                attachment(&format!("{cup}-{id}.json")),
            )],
            Json(rows),
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(response) => response,
    }
}

pub(crate) async fn leaderboard(State(state): State<ApiState>) -> Response {
    match state.0.storage.leaderboard(100).await {
        Ok(rows) => Json(rows).into_response(),
        Err(err) => internal(err),
    }
}

pub(crate) async fn records(State(state): State<ApiState>, Path(uid): Path<String>) -> Response {
    match state.0.storage.best_times(&uid, 100).await {
        Ok(rows) => Json(rows).into_response(),
        Err(err) => internal(err),
    }
}

/// Vendored htmx (works without Internet).
pub(crate) async fn htmx() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("../static/htmx.min.js"),
    )
        .into_response()
}
