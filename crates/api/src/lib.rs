//! Web dashboard, registration page, JSON API and WebSocket events (axum + askama + htmx).
//!
//! - Public: `/` (server status), `/cup` (live cup, refreshed with htmx), `/register`,
//!   `/api/*` (JSON), `/api/history/{id}/export.csv|json`, `/ws` (public event stream).
//! - Admin (token): `/admin` (players, formats, cup controls, registrations, audit log, console).
//!
//! Admin authentication: `Authorization: Bearer <token>` or a session cookie obtained on
//! `/admin/login`. Sessions are random, HttpOnly, `SameSite=Strict` and expire after 12 hours.
//! Serve it behind a reverse proxy with TLS if it is exposed outside the machine.

mod admin;
mod json;
mod pages;
mod ws;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::{HeaderMap, header};
use axum::routing::{get, post};
use pitwall_core::{Handles, Lang};
use pitwall_formats::FormatRegistry;
use pitwall_plugin_cup::CupHandle;
use pitwall_storage::Storage;
use tokio_util::sync::CancellationToken;

/// API settings.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub bind: String,
    /// Empty disables the admin pages.
    pub admin_token: String,
    pub public_url: String,
    /// Cups are enabled: cup, registration and history pages are linked.
    pub cups: bool,
}

/// Errors starting the server.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("cannot listen on {addr}: {source}")]
    Bind {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("web server error: {0}")]
    Serve(#[source] std::io::Error),
}

const SESSION_COOKIE: &str = "pw_session";
const LANG_COOKIE: &str = "pw_lang";
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);

struct Inner {
    handles: Handles,
    cups: CupHandle,
    formats: Arc<RwLock<FormatRegistry>>,
    storage: Storage,
    config: ApiConfig,
    sessions: Mutex<HashMap<String, Instant>>,
}

/// Shared state of the handlers.
#[derive(Clone)]
pub struct ApiState(Arc<Inner>);

impl ApiState {
    pub fn new(
        handles: Handles,
        cups: CupHandle,
        formats: Arc<RwLock<FormatRegistry>>,
        storage: Storage,
        config: ApiConfig,
    ) -> ApiState {
        let i18n = &handles.i18n;
        if let Err(err) = i18n
            .add_catalog(Lang::En, include_str!("../locales/en.toml"))
            .and_then(|()| i18n.add_catalog(Lang::Fr, include_str!("../locales/fr.toml")))
        {
            tracing::error!(%err, "invalid web translations");
        }
        ApiState(Arc::new(Inner {
            handles,
            cups,
            formats,
            storage,
            config,
            sessions: Mutex::new(HashMap::new()),
        }))
    }

    fn handles(&self) -> &Handles {
        &self.0.handles
    }

    fn admin_enabled(&self) -> bool {
        !self.0.config.admin_token.is_empty()
    }

    /// Creates a session and returns its id.
    fn new_session(&self) -> Option<String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).ok()?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut sessions = self
            .0
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        sessions.retain(|_, created| created.elapsed() < SESSION_TTL);
        sessions.insert(id.clone(), Instant::now());
        Some(id)
    }

    fn drop_session(&self, id: &str) {
        self.0
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }

    /// `true` if the request carries the admin token or a valid session.
    fn is_admin(&self, headers: &HeaderMap) -> bool {
        if !self.admin_enabled() {
            return false;
        }
        if let Some(token) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            return constant_time_eq(token.as_bytes(), self.0.config.admin_token.as_bytes());
        }
        match cookie(headers, SESSION_COOKIE) {
            Some(id) => self
                .0
                .sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&id)
                .is_some_and(|created| created.elapsed() < SESSION_TTL),
            None => false,
        }
    }

    fn check_token(&self, token: &str) -> bool {
        self.admin_enabled()
            && constant_time_eq(token.as_bytes(), self.0.config.admin_token.as_bytes())
    }
}

/// Compares secrets without leaking their content through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reads a cookie.
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

/// Language of a request: `pw_lang` cookie, then `Accept-Language`, then the default.
fn request_lang(state: &ApiState, headers: &HeaderMap) -> Lang {
    if let Some(lang) = cookie(headers, LANG_COOKIE).and_then(|l| Lang::from_code(&l)) {
        return lang;
    }
    headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(',')
                .find_map(|part| Lang::from_code(part.split(';').next()?.trim()))
        })
        .unwrap_or(state.handles().default_lang)
}

/// All routes.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/", get(pages::index))
        .route("/lang/{lang}", get(pages::set_lang))
        .route("/cup", get(pages::cup))
        .route("/cup/live", get(pages::cup_live))
        .route("/register", get(pages::register_form).post(pages::register))
        .route("/history", get(pages::history))
        .route("/history/{id}", get(pages::history_detail))
        .route("/api/state", get(json::state))
        .route("/api/cup", get(json::cup))
        .route("/api/history", get(json::history))
        .route("/api/history/{id}", get(json::history_detail))
        .route("/api/history/{id}/export.csv", get(json::export_csv))
        .route("/api/history/{id}/export.json", get(json::export_json))
        .route("/api/leaderboard", get(json::leaderboard))
        .route("/api/records/{uid}", get(json::records))
        .route("/ws", get(ws::public))
        .route("/static/htmx.min.js", get(json::htmx))
        .route("/admin", get(admin::dashboard))
        .route("/admin/login", get(admin::login_form).post(admin::login))
        .route("/admin/logout", post(admin::logout))
        .route("/admin/players", get(admin::players))
        .route("/admin/audit", get(admin::audit))
        .route("/admin/registrations", get(admin::registrations))
        .route(
            "/admin/registrations/{id}/{decision}",
            post(admin::decide_registration),
        )
        .route("/admin/command", post(admin::command))
        .route("/admin/ws", get(ws::admin))
        .with_state(state)
}

/// Serves until `shutdown` is cancelled.
pub async fn serve(state: ApiState, shutdown: CancellationToken) -> Result<(), ApiError> {
    let addr = state.0.config.bind.clone();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|source| ApiError::Bind {
            addr: addr.clone(),
            source,
        })?;
    if state.admin_enabled() {
        tracing::info!(%addr, "web dashboard listening (admin at /admin)");
    } else {
        tracing::info!(%addr, "web dashboard listening (admin disabled: set PITWALL_API__ADMIN_TOKEN)");
    }
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .map_err(ApiError::Serve)
}

#[cfg(test)]
mod tests;
