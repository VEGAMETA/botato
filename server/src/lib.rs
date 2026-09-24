//! botato relay: authenticates devices, stores encrypted blobs for ≤24 h in RAM,
//! forwards call signaling, issues TURN credentials. Never sees plaintext.

pub mod fcm;
pub mod mailbox;
pub mod push;
pub mod relay;
mod session;
pub mod turn;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

pub use relay::{Config, Relay};

pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/ws", get(ws))
        .route("/manifest", get(manifest))
        .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .with_state(relay)
}

async fn ws(State(relay): State<Arc<Relay>>, up: WebSocketUpgrade) -> Response {
    let Ok(permit) = relay.handshakes.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    up.max_message_size(session::READ_LIMIT)
        .max_frame_size(session::READ_LIMIT)
        .on_upgrade(move |sock| session::run(relay, sock, permit))
}

async fn manifest(State(relay): State<Arc<Relay>>) -> Response {
    match relay.manifest_raw() {
        Some(raw) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            raw.to_vec(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Re-reads the manifest file whenever its contents change.
pub async fn watch_manifest(relay: Arc<Relay>, path: PathBuf, every: Duration) {
    let mut last: Option<Vec<u8>> = None;
    let mut tick = tokio::time::interval(every);
    loop {
        tick.tick().await;
        let raw = match tokio::fs::read(&path).await {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                eprintln!("manifest: read: {e}");
                continue;
            }
        };
        if last.as_deref() == Some(&raw[..]) {
            continue;
        }
        if let Err(e) = relay.set_manifest(&raw) {
            eprintln!("manifest: rejected: {e:?}");
        }
        last = Some(raw);
    }
}

/// Expires mailbox entries and held signals once a minute.
pub async fn janitor(relay: Arc<Relay>, every: Duration) {
    let mut tick = tokio::time::interval(every);
    loop {
        tick.tick().await;
        relay.sweep();
    }
}
