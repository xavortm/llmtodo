//! The live WebSocket stream.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use llmtodo_api_types::*;

use crate::state::AppState;
use crate::store;

pub async fn ws(State(st): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws_loop(st, socket))
}

async fn ws_loop(st: AppState, mut socket: WebSocket) {
    let mut rx = st.events.subscribe();
    // An upgraded socket outlives the HTTP server's graceful shutdown, so
    // it ends itself when the server stops. Clients then reconnect.
    let shutdown = st.shutdown.notified();
    tokio::pin!(shutdown);
    shutdown.as_mut().enable();
    let last_seq = match st.pool().acquire().await {
        Ok(mut c) => store::activity::last_seq(&mut c).await.unwrap_or(0),
        Err(_) => 0,
    };
    let hello = WsMessage::Hello {
        last_seq,
        version: VERSION.to_string(),
    };
    if socket
        .send(Message::Text(
            serde_json::to_string(&hello).unwrap_or_default().into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
            msg = rx.recv() => {
                let msg = match msg {
                    Ok(m) => m,
                    // The client fell behind; tell it to catch up from its last seq.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => WsMessage::Reload,
                    Err(_) => return,
                };
                let text = serde_json::to_string(&msg).unwrap_or_default();
                if socket.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    _ => {}
                }
            }
        }
    }
}
