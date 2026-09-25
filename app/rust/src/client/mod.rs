//! Relay client: one background task that keeps an authenticated WebSocket to a relay
//! from the manifest, and turns relay frames into verified events for the UI.

pub mod contacts;
pub mod messaging;
pub mod nostr;
pub mod photo;
pub mod store;
#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod nostr_tests;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use proto::auth::{self, Nonce};
use proto::cert::Cert;
use proto::device::DeviceSecret;
use proto::envelope::{self, KIND_SIGNAL};
use proto::frames::{ClientFrame, ErrorCode, PROTOCOL_VERSION, ServerFrame};
use proto::manifest::{Manifest, ServerEntry};
use proto::{Id, enroll};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::config;
use contacts::Contacts;
use crate::lang::tr;

/// Signals older (or further in the future) than this are dropped.
const SIGNAL_MAX_AGE_SECS: u64 = 120;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(10);
/// No frame (incl. pong) for this long = the socket died silently (typical after a network change).
const LIVENESS: Duration = Duration::from_secs(25);
/// New users appear only in the directory; poll it now and then.
const DIRECTORY_EVERY: Duration = Duration::from_secs(300);
/// Outbox expiry check.
const EXPIRE_EVERY: Duration = Duration::from_secs(600);
/// How long a sent signal is kept for a possible Nostr copy (`signaled online:false` comes back fast).
const RECENT_SIGNALS: Duration = Duration::from_secs(10);

pub enum Event {
    Connecting { server: String },
    Online { server: String },
    Offline { reason: String },
    ContactsChanged,
    Signal { from_user: Id, from_device: Id, payload: String },
    Signaled { to: Id, online: bool },
    Turn { urls: Vec<String>, username: String, credential: String },
    Error { message: String },
    /// History of this conversation changed (new message, status update).
    MessagesChanged { peer: Id },
    /// A new incoming message (for a notification).
    NewMessage { peer: Id, text: String },
    /// Answer to `Cmd::Presence`: the queried devices that are connected now.
    Presence { online: Vec<Id> },
}

pub enum Cmd {
    Signal { to: Id, payload: Vec<u8> },
    Directory,
    Turn,
    /// Send outbox entries not yet sent in this session.
    Flush,
    /// (Re)register the push token with the relay.
    PushToken,
    /// Ask which of these devices are connected.
    Presence(Vec<Id>),
}

enum Exit {
    Stopped,
    Reconnect,
}

pub struct Shared {
    pub me: DeviceSecret,
    pub cert: Vec<u8>,
    pub contacts: Mutex<Contacts>,
    /// The newest signed manifest known: built in, saved, or fetched from a relay.
    pub manifest: std::sync::RwLock<Arc<Manifest>>,
    /// Where a newer manifest is saved (survives restarts; the built-in one only changes with a release).
    pub manifest_path: std::path::PathBuf,
    /// Drop the current connection and dial again at once (network changed).
    pub reconnect: tokio::sync::Notify,
    pub store: Mutex<store::Store>,
    /// Our own user id (from our certificate).
    pub my_user: Id,
    /// FCM token; sent to the relay on every connection (it keeps tokens in RAM only).
    pub push_token: Mutex<Option<String>>,
    /// The Nostr fallback (call signals and the manifest when our relay can't help).
    pub nostr: nostr::Link,
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// Picks the built-in manifest (the only channel so far).
pub fn load_manifest() -> anyhow::Result<Manifest> {
    if config::BUILTIN_MANIFEST.is_empty() {
        anyhow::bail!(tr("в сборке нет манифеста", "the build has no manifest"));
    }
    Manifest::verify(&config::root_pub(), config::BUILTIN_MANIFEST).map_err(|e| anyhow::anyhow!("{}: {e}", tr("манифест", "manifest")))
}

/// The built-in manifest, or the saved one at `path` if it is valid and newer.
pub fn load_manifest_from(path: &std::path::Path) -> anyhow::Result<Manifest> {
    let builtin = load_manifest();
    let saved = std::fs::read(path).ok().and_then(|raw| Manifest::verify(&config::root_pub(), &raw).ok());
    match (builtin, saved) {
        (Ok(b), Some(s)) if s.version > b.version => Ok(s),
        (Ok(b), _) => Ok(b),
        (Err(_), Some(s)) => Ok(s),
        (Err(e), None) => Err(e),
    }
}

impl Shared {
    pub fn manifest(&self) -> Arc<Manifest> {
        self.manifest.read().unwrap().clone()
    }

    /// Takes a signed manifest from any channel: accepted only with the admin signature and a
    /// higher version than the current one (a relay can withhold a manifest, not roll it back).
    /// Returns true if it replaced the current one.
    pub fn offer_manifest(&self, raw: &[u8]) -> anyhow::Result<bool> {
        self.offer_manifest_signed_by(raw, &config::root_pub())
    }

    pub(crate) fn offer_manifest_signed_by(&self, raw: &[u8], root_pub: &[u8; 32]) -> anyhow::Result<bool> {
        let m = Manifest::verify(root_pub, raw).map_err(|e| anyhow::anyhow!("{}: {e}", tr("манифест", "manifest")))?;
        let mut cur = self.manifest.write().unwrap();
        if m.version <= cur.version {
            return Ok(false);
        }
        let tmp = self.manifest_path.with_extension("tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, &self.manifest_path)?;
        *cur = Arc::new(m);
        Ok(true)
    }
}

pub fn parse_cert(text: &str) -> anyhow::Result<Vec<u8>> {
    enroll::from_text(enroll::CERT_PREFIX, text).map_err(|_| anyhow::anyhow!("bad cert"))
}

/// Runs forever: connect, serve, back off, reconnect.
pub async fn run(shared: Arc<Shared>, mut cmds: mpsc::UnboundedReceiver<Cmd>, emit: Emit) {
    let mut backoff = 1u64;
    loop {
        for server in shared.manifest().servers.clone() {
            emit(Event::Connecting { server: server.url.clone() });
            let result = session(&shared, &server, &mut cmds, &emit).await;
            shared.nostr.set_online(false);
            match result {
                Ok(Exit::Stopped) => return, // command channel closed: the app stopped the client
                Ok(Exit::Reconnect) => {
                    backoff = 0;
                    continue;
                }
                Err(e) => emit(Event::Offline { reason: e.to_string() }),
            }
        }
        // Queued commands stay queued; a reconnect request skips the wait.
        let wait = tokio::time::sleep(Duration::from_secs(backoff));
        tokio::pin!(wait);
        tokio::select! {
            _ = &mut wait => {}
            _ = shared.reconnect.notified() => backoff = 0,
        }
        backoff = (backoff * 2).clamp(1, 30);
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, f: &ClientFrame) -> anyhow::Result<()> {
    ws.send(Message::Text(serde_json::to_string(f)?.into())).await?;
    Ok(())
}

/// Next JSON frame; every received message (pongs included) refreshes `last_rx`.
async fn recv(ws: &mut Ws, last_rx: &mut tokio::time::Instant) -> anyhow::Result<ServerFrame> {
    loop {
        let msg = ws.next().await;
        *last_rx = tokio::time::Instant::now();
        match msg {
            Some(Ok(Message::Text(t))) => return Ok(serde_json::from_str(t.as_str())?),
            Some(Ok(Message::Close(f))) => anyhow::bail!("{}: {}", tr("закрыто сервером", "closed by the server"), f.map(|f| f.code.to_string()).unwrap_or_default()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => anyhow::bail!(tr("соединение закрыто", "connection closed")),
        }
    }
}

/// Dials the relay by name; if that fails (DNS broken under some VPNs, poisoned
/// records) and the manifest has an IP, dials the IP with TLS still bound to the name.
/// The server key check in the handshake makes the IP path as safe as the name path.
async fn dial(server: &ServerEntry) -> anyhow::Result<Ws> {
    let direct = tokio_tungstenite::connect_async(&server.url).await;
    let err = match direct {
        Ok((ws, _)) => return Ok(ws),
        Err(e) => e,
    };
    if server.ip_fallback.is_empty() {
        return Err(err.into());
    }
    let uri: tokio_tungstenite::tungstenite::http::Uri = server.url.parse()?;
    let port = uri.port_u16().unwrap_or(443);
    let tcp = tokio::net::TcpStream::connect((server.ip_fallback.as_str(), port))
        .await
        .map_err(|e2| anyhow::anyhow!("{err}; {}: {e2}", tr("по IP тоже не вышло", "by IP failed too")))?;
    let (ws, _) = tokio_tungstenite::client_async_tls(server.url.as_str(), tcp).await?;
    Ok(ws)
}

async fn handshake(shared: &Shared, server: &ServerEntry) -> anyhow::Result<Ws> {
    let mut ws = dial(server).await?;
    let mut last = tokio::time::Instant::now();
    let cn: Nonce = proto::random_bytes();
    send(&mut ws, &ClientFrame::Hello { v: PROTOCOL_VERSION, nonce: cn.to_vec() }).await?;
    let ServerFrame::Challenge { nonce, server_pub, sig } = recv(&mut ws, &mut last).await? else {
        anyhow::bail!(tr("сервер нарушил протокол", "the server broke the protocol"));
    };
    // Verify the server before revealing who we are.
    if server_pub != server.server_pub {
        anyhow::bail!(tr("ключ сервера не совпадает с манифестом", "the server key doesn't match the manifest"));
    }
    let sn: Nonce = nonce.try_into().map_err(|_| anyhow::anyhow!("bad nonce"))?;
    auth::verify_server(&server_pub, &cn, &sn, &sig).map_err(|_| anyhow::anyhow!(tr("сервер не подтвердил ключ", "the server didn't prove its key")))?;
    let csig = auth::sign_client(shared.me.signing_key(), &sn, &server.server_pub, &cn);
    send(&mut ws, &ClientFrame::Auth { cert: shared.cert.clone(), device_id: shared.me.device_id, sig: csig.to_vec() }).await?;
    match recv(&mut ws, &mut last).await? {
        ServerFrame::Ready { .. } => Ok(ws),
        other => anyhow::bail!("{}: {other:?}", tr("авторизация не прошла", "authorization failed")),
    }
}

async fn session(shared: &Shared, server: &ServerEntry, cmds: &mut mpsc::UnboundedReceiver<Cmd>, emit: &Emit) -> anyhow::Result<Exit> {
    let mut ws = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(shared, server))
        .await
        .map_err(|_| anyhow::anyhow!(tr("таймаут подключения", "connection timeout")))??;
    emit(Event::Online { server: server.url.clone() });
    shared.nostr.set_online(true);
    send(&mut ws, &ClientFrame::Directory {}).await?;

    send_push_token(shared, &mut ws).await?;
    let mut s = SessionState::default();
    expire(shared, emit);
    // Everything in the outbox goes out on every connect: the relay may have been restarted
    // or replaced, and it deduplicates copies it already has.
    flush(shared, &mut ws, &mut s).await?;
    request_downloads(shared, &mut ws, &mut s).await?;

    let mut last_rx = tokio::time::Instant::now();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.reset();
    let mut directory = tokio::time::interval(DIRECTORY_EVERY);
    directory.reset();
    let mut housekeeping = tokio::time::interval(EXPIRE_EVERY);
    housekeeping.reset();
    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return Ok(Exit::Stopped) };
                let frame = match cmd {
                    Cmd::Directory => ClientFrame::Directory {},
                    Cmd::Turn => ClientFrame::Turn {},
                    Cmd::Flush => { flush(shared, &mut ws, &mut s).await?; continue; }
                    Cmd::PushToken => { send_push_token(shared, &mut ws).await?; continue; }
                    Cmd::Presence(mut devices) => {
                        devices.truncate(proto::frames::MAX_PRESENCE);
                        ClientFrame::Presence { devices }
                    }
                    Cmd::Signal { to, payload } => match seal_signal(shared, &to, &payload) {
                        Ok(blob) => {
                            s.recent_signals.retain(|(_, _, t)| t.elapsed() < RECENT_SIGNALS);
                            s.recent_signals.push((to, blob.clone(), std::time::Instant::now()));
                            ClientFrame::Signal { to, blob }
                        }
                        Err(e) => { emit(Event::Error { message: e.to_string() }); continue; }
                    },
                };
                send(&mut ws, &frame).await?;
            }
            frame = recv(&mut ws, &mut last_rx) => handle(shared, &mut ws, &mut s, frame?, emit).await?,
            _ = ping.tick() => {
                if last_rx.elapsed() > LIVENESS {
                    anyhow::bail!(tr("сервер не отвечает", "the server is not responding"));
                }
                ws.send(Message::Ping(Default::default())).await?;
            }
            _ = shared.reconnect.notified() => return Ok(Exit::Reconnect),
            _ = directory.tick() => send(&mut ws, &ClientFrame::Directory {}).await?,
            _ = housekeeping.tick() => expire(shared, emit),
        }
    }
}

#[derive(Default)]
struct SessionState {
    /// Outbox entries already sent on this connection.
    sent: std::collections::HashSet<Id>,
    /// Deliveries from senders we don't know yet (retried after a directory update).
    pending: Vec<(Id, Vec<u8>)>,
    /// Attachments uploaded / requested on this connection.
    uploaded: std::collections::HashSet<Id>,
    downloading: std::collections::HashSet<Id>,
    /// Signals sent lately, for a Nostr copy if the relay says the peer is offline.
    recent_signals: Vec<(Id, Vec<u8>, std::time::Instant)>,
}

async fn send_push_token(shared: &Shared, ws: &mut Ws) -> anyhow::Result<()> {
    let token = shared.push_token.lock().unwrap().clone();
    if let Some(token) = token {
        send(ws, &ClientFrame::PushToken { platform: proto::frames::Platform::Fcm, token }).await?;
    }
    Ok(())
}

/// Photos go up before the message copies that point at them: the relay handles one
/// connection's frames in order, so a receiver can't ask for a file that isn't there yet.
async fn flush(shared: &Shared, ws: &mut Ws, s: &mut SessionState) -> anyhow::Result<()> {
    let uploads = shared.store.lock().unwrap().uploads_due()?;
    for u in uploads {
        if s.uploaded.insert(u.file_id) {
            let data = proto::file::seal(&u.key, &u.file_id, &u.data).map_err(|e| anyhow::anyhow!("{e}"))?;
            send(ws, &ClientFrame::Upload { id: u.file_id, data }).await?;
        }
    }
    let rows = shared.store.lock().unwrap().outbox()?;
    for row in rows {
        if s.sent.insert(row.id) {
            send(ws, &ClientFrame::Send { to: row.to_device, id: row.id, blob: row.blob }).await?;
        }
    }
    Ok(())
}

async fn request_downloads(shared: &Shared, ws: &mut Ws, s: &mut SessionState) -> anyhow::Result<()> {
    let pending = shared.store.lock().unwrap().pending_downloads()?;
    for id in pending {
        if s.downloading.insert(id) {
            send(ws, &ClientFrame::Download { id }).await?;
        }
    }
    Ok(())
}

fn expire(shared: &Shared, emit: &Emit) {
    let peers = shared.store.lock().unwrap().expire(config::now() as i64).unwrap_or_default();
    for peer in peers {
        emit(Event::MessagesChanged { peer });
    }
}

/// Processes a mailbox delivery; acks it unless the sender is still unknown.
async fn deliver(shared: &Shared, ws: &mut Ws, s: &mut SessionState, id: Id, blob: Vec<u8>, emit: &Emit) -> anyhow::Result<()> {
    match messaging::on_deliver(shared, &blob) {
        Ok(messaging::Delivered::Done { changed, new_text }) => {
            send(ws, &ClientFrame::Ack { id }).await?;
            if let Some(peer) = changed {
                emit(Event::MessagesChanged { peer });
            }
            if let Some((peer, text)) = new_text {
                emit(Event::NewMessage { peer, text });
            }
            flush(shared, ws, s).await?; // the receipt we owe
            request_downloads(shared, ws, s).await?;
        }
        Ok(messaging::Delivered::UnknownSender) => {
            s.pending.push((id, blob));
            send(ws, &ClientFrame::Directory {}).await?;
        }
        // Local storage error: leave it on the relay, it comes again next connection.
        Err(e) => emit(Event::Error { message: format!("{}: {e}", tr("сообщение", "message")) }),
    }
    Ok(())
}

pub(crate) fn seal_signal(shared: &Shared, to: &Id, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
    let contacts = shared.contacts.lock().unwrap();
    let (_, dev) = contacts.device(to, &shared.manifest()).ok_or_else(|| anyhow::anyhow!(tr("неизвестное устройство", "unknown device")))?;
    envelope::seal(&shared.me, to, &dev.enc_pub, KIND_SIGNAL, config::now(), payload).map_err(|e| anyhow::anyhow!("seal: {e}"))
}

async fn handle(shared: &Shared, ws: &mut Ws, s: &mut SessionState, frame: ServerFrame, emit: &Emit) -> anyhow::Result<()> {
    match frame {
        ServerFrame::Directory { certs } => {
            let changed = shared.contacts.lock().unwrap().merge(&certs, config::now(), &shared.manifest());
            if changed {
                emit(Event::ContactsChanged);
            }
            for (id, blob) in std::mem::take(&mut s.pending) {
                deliver(shared, ws, s, id, blob, emit).await?;
            }
        }
        ServerFrame::Deliver { id, from: _, blob } => deliver(shared, ws, s, id, blob, emit).await?,
        ServerFrame::Stored { id } => {
            if let Ok(Some(peer)) = shared.store.lock().unwrap().on_stored(&id) {
                emit(Event::MessagesChanged { peer });
            }
        }
        // The same envelope may also come over Nostr: take it once.
        ServerFrame::Signal { from: _, blob } if !shared.nostr.first_seen(&blob) => {}
        ServerFrame::Signal { from: _, blob } => match open_signal(shared, &blob) {
            Ok(Some(ev)) => emit(ev),
            Ok(None) => {
                // Unknown sender: maybe a cert we haven't fetched yet.
                send(ws, &ClientFrame::Directory {}).await?;
            }
            Err(_) => {} // forged, stale or garbled: drop silently
        },
        ServerFrame::Signaled { to, online } => {
            // Offline for our relay (it holds the signal and pushes): send it over Nostr too,
            // the peer may be reachable there.
            if !online {
                for (_, blob, _) in s.recent_signals.iter().filter(|(t, _, _)| *t == to) {
                    nostr::publish_signal(shared, to, blob.clone());
                }
                s.recent_signals.retain(|(t, _, _)| *t != to);
            }
            emit(Event::Signaled { to, online })
        }
        ServerFrame::Turn { urls, username, credential, .. } => emit(Event::Turn { urls, username, credential }),
        ServerFrame::Uploaded { .. } => {}
        ServerFrame::Presence { online } => emit(Event::Presence { online }),
        ServerFrame::File { id, data } => {
            s.downloading.remove(&id);
            match messaging::on_file(shared, &id, &data) {
                Ok(Some(peer)) => emit(Event::MessagesChanged { peer }),
                Ok(None) => {}
                Err(e) => emit(Event::Error { message: format!("{}: {e}", tr("фото", "photo")) }),
            }
        }
        // Not there (yet): asked again on the next connection, given up after 24 h.
        ServerFrame::Error { r#ref, id: Some(id), code: ErrorCode::NotFound } if r#ref == "download" => {
            if let Ok(Some(peer)) = messaging::on_file_missing(shared, &id) {
                emit(Event::MessagesChanged { peer });
            }
        }
        // A copy for a revoked device can never be delivered: drop it instead of re-sending it
        // on every connection. (A lying relay gains nothing: it can lose messages anyway.)
        ServerFrame::Error { r#ref, id: Some(id), code: ErrorCode::Revoked } if r#ref == "send" => {
            shared.store.lock().unwrap().drop_copy(&id)?;
        }
        // Calling a revoked device: nothing to report.
        ServerFrame::Error { code: ErrorCode::Revoked, .. } => {}
        // A copy the relay refused otherwise (quota) stays in the outbox: it is retried on the
        // next connection and dropped at expiry. Same for a refused upload.
        ServerFrame::Error { r#ref, code, .. } => emit(Event::Error { message: format!("{ref}: {code:?}") }),
        ServerFrame::Ok { .. } => {}
        ServerFrame::Challenge { .. } | ServerFrame::Ready { .. } => anyhow::bail!(tr("неожиданный фрейм", "unexpected frame")),
    }
    Ok(())
}

/// `Ok(None)` = sender unknown (fetch the directory and let the peer retry).
fn open_signal(shared: &Shared, blob: &[u8]) -> anyhow::Result<Option<Event>> {
    let unverified = envelope::open(&shared.me, blob).map_err(|e| anyhow::anyhow!("{e}"))?;
    let sender = unverified.sender();
    let contacts = shared.contacts.lock().unwrap();
    let Some((cert, dev)) = contacts.device(&sender, &shared.manifest()) else { return Ok(None) };
    let from_user = cert.user_id;
    let opened = unverified.verify(&dev.sign_pub).map_err(|e| anyhow::anyhow!("{e}"))?;
    drop(contacts);
    if opened.kind != KIND_SIGNAL || config::now().abs_diff(opened.sent_at) > SIGNAL_MAX_AGE_SECS {
        anyhow::bail!("stale or wrong kind");
    }
    Ok(Some(Event::Signal {
        from_user,
        from_device: sender,
        payload: String::from_utf8(opened.payload)?,
    }))
}

/// Own cert, verified, for display.
pub fn my_cert(cert: &[u8]) -> anyhow::Result<Cert> {
    Cert::verify(&config::root_pub(), cert, config::now()).map_err(|e| anyhow::anyhow!("{e}"))
}
