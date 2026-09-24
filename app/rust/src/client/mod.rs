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
const PING_EVERY: Duration = Duration::from_secs(25);
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

pub struct Shared {
    pub me: DeviceSecret,
    pub cert: Vec<u8>,
    pub contacts: Mutex<Contacts>,
    pub manifest: Manifest,
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
                Ok(()) => return, // command channel closed: the app stopped the client
                Err(e) => emit(Event::Offline { reason: e.to_string() }),
            }
            backoff = 1; // a server that got as far as failing is tried again quickly next round
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(ws: &mut Ws, f: &ClientFrame) -> anyhow::Result<()> {
    ws.send(Message::Text(serde_json::to_string(f)?.into())).await?;
    Ok(())
}

async fn recv(ws: &mut Ws) -> anyhow::Result<ServerFrame> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return Ok(serde_json::from_str(t.as_str())?),
            Some(Ok(Message::Close(f))) => anyhow::bail!("закрыто сервером: {}", f.map(|f| f.code.to_string()).unwrap_or_default()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => anyhow::bail!("соединение закрыто"),
        }
    }
}

async fn handshake(shared: &Shared, server: &ServerEntry) -> anyhow::Result<Ws> {
    let (mut ws, _) = tokio_tungstenite::connect_async(&server.url).await?;
    let cn: Nonce = proto::random_bytes();
    send(&mut ws, &ClientFrame::Hello { v: PROTOCOL_VERSION, nonce: cn.to_vec() }).await?;
    let ServerFrame::Challenge { nonce, server_pub, sig } = recv(&mut ws).await? else {
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
    match recv(&mut ws).await? {
        ServerFrame::Ready { .. } => Ok(ws),
        other => anyhow::bail!("авторизация не прошла: {other:?}"),
    }
}

async fn session(shared: &Shared, server: &ServerEntry, cmds: &mut mpsc::UnboundedReceiver<Cmd>, emit: &Emit) -> anyhow::Result<()> {
    let mut ws = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(shared, server))
        .await
        .map_err(|_| anyhow::anyhow!("таймаут подключения"))??;
    emit(Event::Online { server: server.url.clone() });
    send(&mut ws, &ClientFrame::Directory {}).await?;

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.reset();
    let mut directory = tokio::time::interval(DIRECTORY_EVERY);
    directory.reset();
    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
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
            frame = recv(&mut ws) => handle(shared, &mut ws, frame?, emit).await?,
            _ = ping.tick() => ws.send(Message::Ping(Default::default())).await?,
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
