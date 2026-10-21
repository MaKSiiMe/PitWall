//! Router tests (no game server needed).

use std::sync::{Arc, RwLock};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use pitwall_core::{Controller, ControllerOptions, RoleBook, RolesConfig, Translator};
use pitwall_formats::FormatRegistry;
use pitwall_formats::cup::CupLibrary;
use pitwall_plugin_cup::CupPlugin;
use pitwall_storage::Storage;
use tower::ServiceExt;

use crate::{ApiConfig, ApiState, router};

const TOKEN: &str = "s3cret-token";

async fn state() -> ApiState {
    let controller = Controller::new(
        ControllerOptions::default(),
        RoleBook::from_config(&RolesConfig::default()),
        Translator::with_core_catalogs().unwrap(),
    );
    let formats = Arc::new(RwLock::new(FormatRegistry::builtin().unwrap()));
    let (_plugin, cups) = CupPlugin::new(
        Arc::clone(&formats),
        Arc::new(RwLock::new(CupLibrary::default())),
        std::env::temp_dir(),
    );
    ApiState::new(
        controller.handles(),
        cups,
        formats,
        Storage::in_memory().await.unwrap(),
        ApiConfig {
            bind: "127.0.0.1:0".into(),
            admin_token: TOKEN.into(),
            public_url: String::new(),
            cups: true,
        },
    )
}

async fn get(
    state: &ApiState,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut request = Request::builder().uri(uri);
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

async fn post_form(
    state: &ApiState,
    uri: &str,
    form: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = router(state.clone())
        .oneshot(request.body(Body::from(form.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn public_pages_render() {
    let state = state().await;
    let (status, _, body) = get(&state, "/", &[("accept-language", "fr-FR,fr;q=0.9")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Hors ligne"), "French by Accept-Language");
    let (_, _, body) = get(&state, "/", &[("cookie", "pw_lang=en")]).await;
    assert!(body.contains("Offline"));
    let (status, _, body) = get(&state, "/cup/live", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No cup"));
    let (_, _, body) = get(&state, "/register", &[]).await;
    assert!(body.contains("closed"));
    let (status, headers, _) = get(&state, "/static/htmx.min.js", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/javascript")
    );
    let (status, _, body) = get(&state, "/api/state", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"connected\":false"));
    let (status, _, _) = get(&state, "/api/history/42/export.csv", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, body) = get(&state, "/history", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Past cups") || body.contains("Cups passées"));
}

#[tokio::test]
async fn admin_requires_authentication() {
    let state = state().await;
    let (status, headers, _) = get(&state, "/admin", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/admin/login");
    let (status, headers, _) = get(&state, "/admin/players", &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["hx-redirect"], "/admin/login");

    let (status, _, _) = get(
        &state,
        "/admin/players",
        &[("authorization", "Bearer wrong")],
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    let bearer = format!("Bearer {TOKEN}");
    let (status, _, _) = get(&state, "/admin/players", &[("authorization", &bearer)]).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn login_creates_a_session() {
    let state = state().await;
    let (status, _, body) = post_form(&state, "/admin/login", "token=nope", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Wrong token") || body.contains("Token incorrect"));

    let (status, headers, _) =
        post_form(&state, "/admin/login", &format!("token={TOKEN}"), &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let cookie = headers[header::SET_COOKIE].to_str().unwrap().to_owned();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let session = cookie.split(';').next().unwrap().to_owned();

    let (status, _, body) = get(&state, "/admin", &[("cookie", &session)]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("hx-post=\"/admin/command\""));

    let (status, _, _) = post_form(&state, "/admin/logout", "", &[("cookie", &session)]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _, _) = get(&state, "/admin", &[("cookie", &session)]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "session revoked");
}

#[tokio::test]
async fn console_runs_commands() {
    let state = state().await;
    let bearer = format!("Bearer {TOKEN}");
    let (status, _, body) = post_form(
        &state,
        "/admin/command",
        "line=nope",
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("&gt; nope"), "{body}");
    assert!(
        body.contains("Unknown command") || body.contains("Commande inconnue"),
        "{body}"
    );
}
