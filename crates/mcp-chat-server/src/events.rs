//! Server-sent events.
//!
//! The desktop shell forwards `CoreEvent` to the webview over three Tauri
//! channels. Here everything goes down one SSE stream in broadcast order, which
//! is strictly easier to reason about: the frontend already dispatches on
//! `event.type`.
//!
//! On reconnect we emit `Resync`, because events that arrived while the socket
//! was down are gone. The frontend answers that by reloading the open
//! conversation, which is the same path it uses when the desktop app lags.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast;

use mcp_chat_core::events::CoreEvent;

use crate::state::AppState;

pub async fn events(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let reconnected = headers.contains_key("last-event-id");
    let receiver = state.engine.subscribe();
    let first = reconnected.then(|| CoreEvent::Resync {
        reason: "Reconnected; reloading the active conversation.".into(),
    });

    let stream = futures::stream::unfold((receiver, first), |(mut receiver, first)| async move {
        if let Some(event) = first {
            return Some((Ok::<_, Infallible>(frame(event)), (receiver, None)));
        }
        let event = match receiver.recv().await {
            Ok(event) => event,
            // Losing events silently would leave the transcript wrong, so ask
            // the client to reload, exactly like the desktop does.
            Err(broadcast::error::RecvError::Lagged(count)) => CoreEvent::Resync {
                reason: format!(
                    "The UI could not keep up with {count} events; reloading the active \
                     conversation."
                ),
            },
            Err(broadcast::error::RecvError::Closed) => return None,
        };
        Some((Ok(frame(event)), (receiver, None)))
    });

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    // A reverse proxy buffers by default, which turns a token stream into one
    // delivery at the end. `no-cache` alone is not enough: nginx (and NPM,
    // which runs nginx) only honours `X-Accel-Buffering` for this.
    //
    // The proxy still has to have buffering off for the connection -- this
    // header is the app's half of the contract, not a substitute for it.
    response.headers_mut().insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    response
}

fn frame(event: CoreEvent) -> Event {
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
    Event::default()
        .id(mcp_chat_core::now_ms().to_string())
        .data(data)
}
