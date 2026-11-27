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
        loop {
            match receiver.recv().await {
                Ok(event) => return Some((Ok(frame(event)), (receiver, None))),
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    let event = CoreEvent::Resync {
                        reason: format!(
                            "The UI could not keep up with {count} events; reloading the \
                                 active conversation."
                        ),
                    };
                    return Some((Ok(frame(event)), (receiver, None)));
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn frame(event: CoreEvent) -> Event {
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
    Event::default()
        .id(mcp_chat_core::now_ms().to_string())
        .data(data)
}
