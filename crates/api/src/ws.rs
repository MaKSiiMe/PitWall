//! WebSocket event streams (overlays, external tools).
//!
//! `/ws` is public and only carries race and cup events (no chat, no admin actions);
//! `/admin/ws` requires the admin token and carries everything but raw callbacks.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use pitwall_core::Event;
use tokio::sync::broadcast::error::RecvError;

use crate::ApiState;

fn public_event(event: &Event) -> bool {
    match event {
        Event::Connected { .. }
        | Event::Disconnected
        | Event::PlayerJoined { .. }
        | Event::PlayerLeft { .. }
        | Event::Checkpoint { .. }
        | Event::Finish { .. }
        | Event::GiveUp { .. }
        | Event::MatchStart
        | Event::MatchEnd
        | Event::MapStart { .. }
        | Event::MapEnd { .. }
        | Event::RoundStart { .. }
        | Event::RoundEnd { .. }
        | Event::Scores { .. }
        | Event::WarmUp { .. }
        | Event::Pause { .. }
        | Event::ModeChanged { .. } => true,
        Event::Custom { source, .. } => source == "cup",
        _ => false,
    }
}

fn admin_event(event: &Event) -> bool {
    !matches!(event, Event::Callback { .. })
}

pub(crate) async fn public(State(state): State<ApiState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| stream(state, socket, public_event))
}

pub(crate) async fn admin(
    State(state): State<ApiState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !state.is_admin(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    upgrade.on_upgrade(move |socket| stream(state, socket, admin_event))
}

async fn stream(state: ApiState, socket: WebSocket, filter: fn(&Event) -> bool) {
    let mut events = state.handles().bus.raw_subscribe();
    let (mut sink, mut incoming) = socket.split();
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) if filter(&event) => {
                    let Ok(json) = serde_json::to_string(&event) else { continue };
                    if sink.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(skipped)) => {
                    tracing::debug!(skipped, "WebSocket client too slow");
                }
                Err(RecvError::Closed) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
