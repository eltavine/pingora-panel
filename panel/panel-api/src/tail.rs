//! What the API follows over a WebSocket (RFC 6455): each message is a JSON
//! text message, and one that says why the tail ended is the last.

use axum::{
    body::Bytes,
    extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket},
};
use futures_util::{Stream, StreamExt};
use panel_errors::{ErrorCode, PanelError};
use serde::Serialize;
use std::time::Duration;
use utoipa::ToSchema;

/// How often an idle tail is pinged.
const PING: Duration = Duration::from_secs(30);

/// Why a tail ended.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LogTailError {
    pub code: String,
    pub message: String,
}

impl From<&PanelError> for LogTailError {
    fn from(error: &PanelError) -> Self {
        Self {
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
        }
    }
}

/// What a tail sends next.
pub(crate) enum Relayed<M> {
    Sent(M),
    /// The last message, which says why the tail ended.
    Failed(M, PanelError),
}

/// The close code for a tail that failed with `error`.
fn close_code(error: &PanelError) -> u16 {
    match error.code.as_str() {
        ErrorCode::INVALID_ARGUMENT => 1008,
        ErrorCode::UNAVAILABLE | ErrorCode::RESOURCE_EXHAUSTED => 1013,
        _ => 1011,
    }
}

async fn send(socket: &mut WebSocket, message: &impl Serialize) -> bool {
    let text = serde_json::to_string(message).expect("tail messages serialize");
    socket
        .send(Message::Text(Utf8Bytes::from(text)))
        .await
        .is_ok()
}

/// Relays `messages` to `socket` until either side ends it, pinging it
/// while idle. A tail that runs out closes the socket with `end`.
pub(crate) async fn relay<M: Serialize>(
    mut socket: WebSocket,
    messages: impl Stream<Item = Relayed<M>>,
    end: Option<CloseFrame>,
) {
    let mut messages = std::pin::pin!(messages);
    let mut ping = tokio::time::interval(PING);
    ping.tick().await;
    loop {
        tokio::select! {
            next = messages.next() => {
                match next {
                    Some(Relayed::Sent(message)) => {
                        if !send(&mut socket, &message).await {
                            return;
                        }
                    }
                    Some(Relayed::Failed(message, error)) => {
                        let _ = send(&mut socket, &message).await;
                        let _ = socket
                            .send(Message::Close(Some(CloseFrame {
                                code: close_code(&error),
                                reason: Utf8Bytes::from_static("the tail ended"),
                            })))
                            .await;
                        return;
                    }
                    None => {
                        let _ = socket.send(Message::Close(end)).await;
                        return;
                    }
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => {}
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
        }
    }
}
