//! Relay client: one background task that keeps an authenticated WebSocket to a relay
//! from the manifest, and turns relay frames into verified events for the UI.

pub mod contacts;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use proto::auth::{self, Nonce};
use proto::cert::Cert;
use proto::device::DeviceSecret;
use proto::envelope::{self, KIND_SIGNAL};
use proto::frames::{ClientFrame, PROTOCOL_VERSION, ServerFrame};
use proto::manifest::{Manifest, ServerEntry};
use proto::{Id, enroll};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::config;
use contacts::Contacts;

/// Signals older (or further in the future) than this are dropped.
const SIGNAL_MAX_AGE_SECS: u64 = 120;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(10);
/// No frame (incl. pong) for this long = the socket died silently (typical after a network change).
const LIVENESS: Duration = Duration::from_secs(25);
/// New users appear only in the directory; poll it now and then.
const DIRECTORY_EVERY: Duration = Duration::from_secs(300);

pub enum Event {
    Connecting { server: String },
    Online { server: String },
    Offline { reason: String },
    ContactsChanged,
    Signal { from_user: Id, from_device: Id, payload: String },
    Signaled { to: Id, online: bool },
    Turn { urls: Vec<String>, username: String, credential: String },
    Error { message: String },
}

pub enum Cmd {
    Signal { to: Id, payload: Vec<u8> },
    Directory,
    Turn,
}

enum Exit {
    Stopped,
    Reconnect,
}

pub struct Shared {
    pub me: DeviceSecret,
    pub cert: Vec<u8>,
    pub contacts: Mutex<Contacts>,
    pub manifest: Manifest,
    /// Drop the current connection and dial again at once (network changed).
    pub reconnect: tokio::sync::Notify,
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

/// Picks the built-in manifest (the only channel so far).
pub fn load_manifest() -> anyhow::Result<Manifest> {
    if config::BUILTIN_MANIFEST.is_empty() {
        anyhow::bail!("в сборке нет манифеста");
    }
    Manifest::verify(&config::root_pub(), config::BUILTIN_MANIFEST).map_err(|e| anyhow::anyhow!("манифест: {e}"))
}

pub fn parse_cert(text: &str) -> anyhow::Result<Vec<u8>> {
    enroll::from_text(enroll::CERT_PREFIX, text).map_err(|_| anyhow::anyhow!("bad cert"))
}

/// Runs forever: connect, serve, back off, reconnect.
pub async fn run(shared: Arc<Shared>, mut cmds: mpsc::UnboundedReceiver<Cmd>, emit: Emit) {
    let mut backoff = 1u64;
    loop {
        for server in shared.manifest.servers.clone() {
            emit(Event::Connecting { server: server.url.clone() });
            match session(&shared, &server, &mut cmds, &emit).await {
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
            Some(Ok(Message::Close(f))) => anyhow::bail!("закрыто сервером: {}", f.map(|f| f.code.to_string()).unwrap_or_default()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => anyhow::bail!("соединение закрыто"),
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
        .map_err(|e2| anyhow::anyhow!("{err}; по IP тоже не вышло: {e2}"))?;
    let (ws, _) = tokio_tungstenite::client_async_tls(server.url.as_str(), tcp).await?;
    Ok(ws)
}

async fn handshake(shared: &Shared, server: &ServerEntry) -> anyhow::Result<Ws> {
    let mut ws = dial(server).await?;
    let mut last = tokio::time::Instant::now();
    let cn: Nonce = proto::random_bytes();
    send(&mut ws, &ClientFrame::Hello { v: PROTOCOL_VERSION, nonce: cn.to_vec() }).await?;
    let ServerFrame::Challenge { nonce, server_pub, sig } = recv(&mut ws, &mut last).await? else {
        anyhow::bail!("сервер нарушил протокол");
    };
    // Verify the server before revealing who we are.
    if server_pub != server.server_pub {
        anyhow::bail!("ключ сервера не совпадает с манифестом");
    }
    let sn: Nonce = nonce.try_into().map_err(|_| anyhow::anyhow!("bad nonce"))?;
    auth::verify_server(&server_pub, &cn, &sn, &sig).map_err(|_| anyhow::anyhow!("сервер не подтвердил ключ"))?;
    let csig = auth::sign_client(shared.me.signing_key(), &sn, &server.server_pub, &cn);
    send(&mut ws, &ClientFrame::Auth { cert: shared.cert.clone(), device_id: shared.me.device_id, sig: csig.to_vec() }).await?;
    match recv(&mut ws, &mut last).await? {
        ServerFrame::Ready { .. } => Ok(ws),
        other => anyhow::bail!("авторизация не прошла: {other:?}"),
    }
}

async fn session(shared: &Shared, server: &ServerEntry, cmds: &mut mpsc::UnboundedReceiver<Cmd>, emit: &Emit) -> anyhow::Result<Exit> {
    let mut ws = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(shared, server))
        .await
        .map_err(|_| anyhow::anyhow!("таймаут подключения"))??;
    emit(Event::Online { server: server.url.clone() });
    send(&mut ws, &ClientFrame::Directory {}).await?;

    let mut last_rx = tokio::time::Instant::now();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.reset();
    let mut directory = tokio::time::interval(DIRECTORY_EVERY);
    directory.reset();
    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return Ok(Exit::Stopped) };
                let frame = match cmd {
                    Cmd::Directory => ClientFrame::Directory {},
                    Cmd::Turn => ClientFrame::Turn {},
                    Cmd::Signal { to, payload } => match seal_signal(shared, &to, &payload) {
                        Ok(blob) => ClientFrame::Signal { to, blob },
                        Err(e) => { emit(Event::Error { message: e.to_string() }); continue; }
                    },
                };
                send(&mut ws, &frame).await?;
            }
            frame = recv(&mut ws, &mut last_rx) => handle(shared, &mut ws, frame?, emit).await?,
            _ = ping.tick() => {
                if last_rx.elapsed() > LIVENESS {
                    anyhow::bail!("сервер не отвечает");
                }
                ws.send(Message::Ping(Default::default())).await?;
            }
            _ = shared.reconnect.notified() => return Ok(Exit::Reconnect),
            _ = directory.tick() => send(&mut ws, &ClientFrame::Directory {}).await?,
        }
    }
}

fn seal_signal(shared: &Shared, to: &Id, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
    let contacts = shared.contacts.lock().unwrap();
    let (_, dev) = contacts.device(to, &shared.manifest).ok_or_else(|| anyhow::anyhow!("неизвестное устройство"))?;
    envelope::seal(&shared.me, to, &dev.enc_pub, KIND_SIGNAL, config::now(), payload).map_err(|e| anyhow::anyhow!("seal: {e}"))
}

async fn handle(shared: &Shared, ws: &mut Ws, frame: ServerFrame, emit: &Emit) -> anyhow::Result<()> {
    match frame {
        ServerFrame::Directory { certs } => {
            let changed = shared.contacts.lock().unwrap().merge(&certs, config::now(), &shared.manifest);
            if changed {
                emit(Event::ContactsChanged);
            }
        }
        ServerFrame::Signal { from: _, blob } => match open_signal(shared, &blob) {
            Ok(Some(ev)) => emit(ev),
            Ok(None) => {
                // Unknown sender: maybe a cert we haven't fetched yet.
                send(ws, &ClientFrame::Directory {}).await?;
            }
            Err(_) => {} // forged, stale or garbled: drop silently
        },
        ServerFrame::Signaled { to, online } => emit(Event::Signaled { to, online }),
        ServerFrame::Turn { urls, username, credential, .. } => emit(Event::Turn { urls, username, credential }),
        ServerFrame::Error { r#ref, code, .. } => emit(Event::Error { message: format!("{ref}: {code:?}") }),
        // Messages are not implemented yet; leave them on the server (unacked).
        ServerFrame::Deliver { .. } | ServerFrame::Stored { .. } | ServerFrame::Ok { .. } => {}
        ServerFrame::Challenge { .. } | ServerFrame::Ready { .. } => anyhow::bail!("неожиданный фрейм"),
    }
    Ok(())
}

/// `Ok(None)` = sender unknown (fetch the directory and let the peer retry).
fn open_signal(shared: &Shared, blob: &[u8]) -> anyhow::Result<Option<Event>> {
    let unverified = envelope::open(&shared.me, blob).map_err(|e| anyhow::anyhow!("{e}"))?;
    let sender = unverified.sender();
    let contacts = shared.contacts.lock().unwrap();
    let Some((cert, dev)) = contacts.device(&sender, &shared.manifest) else { return Ok(None) };
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
#[allow(dead_code)]
pub fn my_cert(cert: &[u8]) -> anyhow::Result<Cert> {
    Cert::verify(&config::root_pub(), cert, config::now()).map_err(|e| anyhow::anyhow!("{e}"))
}
