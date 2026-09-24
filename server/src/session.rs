//! One WebSocket connection: handshake, then reader / writer / mailbox pump.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use proto::auth::{self, Nonce};
use proto::cert::Cert;
use proto::frames::{
    ClientFrame, ErrorCode, MAX_BLOB, MAX_PUSH_TOKEN, PROTOCOL_VERSION, ServerFrame, close,
};
use proto::file::MAX_FILE;
use proto::{Id, random_bytes};
use tokio::sync::{Notify, OwnedSemaphorePermit, mpsc};
use tokio::time::{interval, timeout};
use tokio_util::sync::CancellationToken;

use crate::files::FileError;
use crate::push::PushToken;
use crate::relay::{AuthCheck, Out, Relay, SessionHandle};

/// Base64 inflates blobs by 4/3; leave room for the JSON envelope. Attachments are the largest.
pub const READ_LIMIT: usize = if MAX_FILE > MAX_BLOB { MAX_FILE } else { MAX_BLOB } * 4 / 3 + 8 * 1024;
const PUMP_BATCH: usize = 32;
const QUEUE: usize = 64;

struct Identity {
    user: Id,
    device: Id,
}

pub async fn run(relay: Arc<Relay>, mut ws: WebSocket, permit: OwnedSemaphorePermit) {
    let who = match timeout(relay.cfg.auth_timeout, handshake(&relay, &mut ws)).await {
        Ok(Ok(who)) => who,
        Ok(Err(code)) => return close_now(ws, code).await,
        Err(_) => return close_now(ws, close::AUTH_TIMEOUT).await,
    };
    drop(permit);

    let (tx, rx) = mpsc::channel(QUEUE);
    let (sig_tx, sig_rx) = mpsc::channel(QUEUE);
    let handle = SessionHandle {
        conn: 0,
        user: who.user,
        tx,
        sig_tx,
        wake: Arc::new(Notify::new()),
        cancel: CancellationToken::new(),
    };
    let Some((handle, old, held)) = relay.register(who.device, handle) else {
        return close_now(ws, 1001).await;
    };
    if let Some(old) = old {
        old.kick(close::REPLACED, "replaced");
    }
    // `ready` must precede everything else; the writer (which prioritizes signals) is not running yet.
    let ready = ServerFrame::Ready {
        user_id: who.user,
        device_id: who.device,
    };
    if !matches!(
        timeout(relay.cfg.write_timeout, send_frame(&mut ws, &ready)).await,
        Ok(Ok(()))
    ) {
        relay.unregister(&who.device, handle.conn);
        return;
    }
    for h in held {
        let _ = handle.sig_tx.try_send(Out::Frame(ServerFrame::Signal {
            from: h.from,
            blob: h.blob,
        }));
    }

    let (sink, stream) = ws.split();
    let writer = tokio::spawn(write_loop(
        relay.clone(),
        sink,
        rx,
        sig_rx,
        handle.cancel.clone(),
    ));
    let pump = tokio::spawn(pump(relay.clone(), who.device, handle.clone()));
    let mut reader = Reader {
        relay: relay.clone(),
        device: who.device,
        handle: handle.clone(),
        tokens: relay.cfg.rate_burst,
        last: Instant::now(),
    };
    reader.run(stream).await;

    handle.cancel.cancel();
    relay.unregister(&who.device, handle.conn);
    let _ = pump.await;
    let _ = writer.await;
}

async fn close_now(mut ws: WebSocket, code: u16) {
    let _ = ws
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: Utf8Bytes::from_static(""),
        })))
        .await;
}

async fn recv_frame(ws: &mut WebSocket) -> Result<ClientFrame, u16> {
    loop {
        match ws.recv().await {
            Some(Ok(Message::Text(t))) => {
                return serde_json::from_str(t.as_str()).map_err(|_| close::PROTOCOL);
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return Err(close::PROTOCOL),
        }
    }
}

async fn send_frame(ws: &mut WebSocket, f: &ServerFrame) -> Result<(), u16> {
    let s = serde_json::to_string(f).expect("frames serialize");
    ws.send(Message::Text(s.into()))
        .await
        .map_err(|_| close::PROTOCOL)
}

async fn handshake(relay: &Relay, ws: &mut WebSocket) -> Result<Identity, u16> {
    let ClientFrame::Hello { v, nonce } = recv_frame(ws).await? else {
        return Err(close::PROTOCOL);
    };
    let cn: Nonce = nonce.as_slice().try_into().map_err(|_| close::PROTOCOL)?;
    if v != PROTOCOL_VERSION {
        return Err(close::PROTOCOL);
    }
    let sn: Nonce = random_bytes();
    let sig = auth::sign_server(&relay.cfg.server_key, &cn, &sn);
    send_frame(
        ws,
        &ServerFrame::Challenge {
            nonce: sn.to_vec(),
            server_pub: relay.server_pub.to_vec(),
            sig: sig.to_vec(),
        },
    )
    .await?;

    let ClientFrame::Auth {
        cert,
        device_id,
        sig,
    } = recv_frame(ws).await?
    else {
        return Err(close::PROTOCOL);
    };
    let c =
        Cert::verify(&relay.cfg.root_pub, &cert, relay.now()).map_err(|_| close::AUTH_FAILED)?;
    let dev = c.device(&device_id).ok_or(close::AUTH_FAILED)?;
    auth::verify_client(&dev.sign_pub, &sn, &relay.server_pub, &cn, &sig)
        .map_err(|_| close::AUTH_FAILED)?;
    match relay.admit(&c, &cert, &device_id) {
        AuthCheck::Ok => {}
        AuthCheck::Revoked => return Err(close::REVOKED),
        AuthCheck::Superseded => return Err(close::AUTH_FAILED),
    }
    Ok(Identity {
        user: c.user_id,
        device: device_id,
    })
}

async fn write_loop(
    relay: Arc<Relay>,
    mut sink: SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<Out>,
    mut sig_rx: mpsc::Receiver<Out>,
    cancel: CancellationToken,
) {
    let mut ping = interval(relay.cfg.ping_interval);
    ping.reset();
    loop {
        let msg = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            Some(o) = sig_rx.recv() => o,
            Some(o) = rx.recv() => o,
            _ = ping.tick() => {
                if timeout(relay.cfg.write_timeout, sink.send(Message::Ping(Default::default()))).await.map_or(true, |r| r.is_err()) {
                    break;
                }
                continue;
            }
        };
        let (m, last) = match msg {
            Out::Frame(f) => (
                Message::Text(serde_json::to_string(&f).expect("frames serialize").into()),
                false,
            ),
            Out::Close(code, reason) => (
                Message::Close(Some(CloseFrame {
                    code,
                    reason: Utf8Bytes::from_static(reason),
                })),
                true,
            ),
        };
        if timeout(relay.cfg.write_timeout, sink.send(m))
            .await
            .map_or(true, |r| r.is_err())
            || last
        {
            break;
        }
    }
    cancel.cancel();
}

/// Streams mailbox entries in order; wakes on new mail.
async fn pump(relay: Arc<Relay>, device: Id, h: SessionHandle) {
    let mut last = 0u64;
    loop {
        let batch = relay
            .mailbox()
            .after(&device, last, PUMP_BATCH, relay.now());
        let full = batch.len() == PUMP_BATCH;
        for m in batch {
            let f = ServerFrame::Deliver {
                id: m.id,
                from: m.from,
                blob: m.blob.to_vec(),
            };
            tokio::select! {
                _ = h.cancel.cancelled() => return,
                r = h.tx.send(Out::Frame(f)) => if r.is_err() { return },
            }
            last = m.seq;
        }
        if full {
            continue;
        }
        tokio::select! {
            _ = h.cancel.cancelled() => return,
            _ = h.wake.notified() => {}
        }
    }
}

struct Reader {
    relay: Arc<Relay>,
    device: Id,
    handle: SessionHandle,
    tokens: f64,
    last: Instant,
}

impl Reader {
    async fn run(&mut self, mut stream: SplitStream<WebSocket>) {
        let idle = self.relay.cfg.idle_timeout;
        loop {
            let next = tokio::select! {
                _ = self.handle.cancel.cancelled() => return,
                n = timeout(idle, stream.next()) => n,
            };
            let msg = match next {
                Ok(Some(Ok(m))) => m,
                _ => return, // idle timeout, error or EOF
            };
            match msg {
                Message::Text(t) => match serde_json::from_str::<ClientFrame>(t.as_str()) {
                    Ok(f) => {
                        if !self.allow() {
                            self.error(frame_ref(&f), None, ErrorCode::RateLimited)
                                .await;
                        } else {
                            self.dispatch(f).await;
                        }
                    }
                    Err(_) => self.error("", None, ErrorCode::BadFrame).await,
                },
                Message::Binary(_) => self.error("", None, ErrorCode::BadFrame).await,
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Close(_) => return,
            }
        }
    }

    fn allow(&mut self) -> bool {
        let cfg = &self.relay.cfg;
        let now = Instant::now();
        self.tokens = (self.tokens
            + now.duration_since(self.last).as_secs_f64() * cfg.rate_per_sec)
            .min(cfg.rate_burst);
        self.last = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }

    async fn reply(&self, f: ServerFrame) {
        let _ = self.handle.tx.send(Out::Frame(f)).await;
    }

    async fn error(&self, r: &str, id: Option<Id>, code: ErrorCode) {
        self.reply(ServerFrame::Error {
            r#ref: r.into(),
            id,
            code,
        })
        .await;
    }

    async fn dispatch(&mut self, f: ClientFrame) {
        let relay = self.relay.clone();
        match f {
            ClientFrame::Send { to, id, blob } => {
                if blob.is_empty() {
                    return self.error("send", Some(id), ErrorCode::BadFrame).await;
                }
                if blob.len() > MAX_BLOB {
                    return self.error("send", Some(id), ErrorCode::TooLarge).await;
                }
                if relay.is_device_revoked(&to) {
                    return self.error("send", Some(id), ErrorCode::Revoked).await;
                }
                let stored = relay.mailbox().put(to, self.device, id, &blob, relay.now());
                if stored.is_err() {
                    return self.error("send", Some(id), ErrorCode::Quota).await;
                }
                self.reply(ServerFrame::Stored { id }).await;
                relay.notify_message(&to);
            }
            ClientFrame::Ack { id } => relay.mailbox().ack(&self.device, &id),
            ClientFrame::Signal { to, blob } => {
                if blob.is_empty() {
                    return self.error("signal", None, ErrorCode::BadFrame).await;
                }
                if blob.len() > MAX_BLOB {
                    return self.error("signal", None, ErrorCode::TooLarge).await;
                }
                if relay.is_device_revoked(&to) {
                    return self.error("signal", None, ErrorCode::Revoked).await;
                }
                let online = relay.route_signal(self.device, to, blob);
                self.reply(ServerFrame::Signaled { to, online }).await;
            }
            ClientFrame::Directory {} => {
                self.reply(ServerFrame::Directory {
                    certs: relay.directory(),
                })
                .await
            }
            ClientFrame::Turn {} => match &relay.cfg.turn {
                Some(t) => {
                    let c = t.issue(relay.now());
                    self.reply(ServerFrame::Turn {
                        urls: t.urls.clone(),
                        username: c.username,
                        credential: c.credential,
                        ttl: t.ttl_secs,
                    })
                    .await;
                }
                None => self.error("turn", None, ErrorCode::Unavailable).await,
            },
            ClientFrame::PushToken { platform, token } => {
                if token.is_empty() || token.len() > MAX_PUSH_TOKEN {
                    return self.error("push_token", None, ErrorCode::BadFrame).await;
                }
                relay.set_push_token(self.device, PushToken { platform, token });
                self.reply(ServerFrame::Ok {
                    r#ref: "push_token".into(),
                })
                .await;
            }
            ClientFrame::Upload { id, data } => {
                let Some(files) = &relay.cfg.files else {
                    return self.error("upload", Some(id), ErrorCode::Unavailable).await;
                };
                if data.is_empty() {
                    return self.error("upload", Some(id), ErrorCode::BadFrame).await;
                }
                if data.len() > MAX_FILE {
                    return self.error("upload", Some(id), ErrorCode::TooLarge).await;
                }
                match files.put(self.device, id, &data, relay.now()).await {
                    Ok(()) => self.reply(ServerFrame::Uploaded { id }).await,
                    Err(FileError::Quota) => self.error("upload", Some(id), ErrorCode::Quota).await,
                    Err(FileError::Io) => self.error("upload", Some(id), ErrorCode::Internal).await,
                }
            }
            ClientFrame::Download { id } => {
                let Some(files) = &relay.cfg.files else {
                    return self.error("download", Some(id), ErrorCode::Unavailable).await;
                };
                match files.get(&id, relay.now()).await {
                    Some(data) => self.reply(ServerFrame::File { id, data }).await,
                    None => self.error("download", Some(id), ErrorCode::NotFound).await,
                }
            }
            ClientFrame::Hello { .. } | ClientFrame::Auth { .. } => {
                self.error("auth", None, ErrorCode::BadFrame).await
            }
        }
    }
}

fn frame_ref(f: &ClientFrame) -> &'static str {
    match f {
        ClientFrame::Hello { .. } => "hello",
        ClientFrame::Auth { .. } => "auth",
        ClientFrame::Send { .. } => "send",
        ClientFrame::Ack { .. } => "ack",
        ClientFrame::Signal { .. } => "signal",
        ClientFrame::Directory {} => "directory",
        ClientFrame::Turn {} => "turn",
        ClientFrame::PushToken { .. } => "push_token",
        ClientFrame::Upload { .. } => "upload",
        ClientFrame::Download { .. } => "download",
    }
}
