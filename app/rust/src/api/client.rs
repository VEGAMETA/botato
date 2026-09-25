//! Relay client API for Dart: start/stop, contacts, call signaling, TURN.

use std::sync::{Arc, LazyLock, Mutex};

use proto::Id;
use proto::device::DeviceSecret;
use tokio::sync::mpsc;

use crate::client::contacts::Contacts;
use crate::client::{self, Cmd, Event, Shared};
use crate::config;
use crate::frb_generated::StreamSink;

pub enum ClientEvent {
    Connecting { server: String },
    Online { server: String },
    Offline { reason: String },
    ContactsChanged,
    /// Authenticated call signal from a device in a valid certificate.
    Signal { from_user: String, from_device: String, payload: String },
    Signaled { to_device: String, online: bool },
    Turn { urls: Vec<String>, username: String, credential: String },
    Error { message: String },
    MessagesChanged { peer_user: String },
    NewMessage { peer_user: String, text: String },
    /// Devices (hex ids) connected to the relay right now, answering `client_query_presence`.
    Presence { online: Vec<String> },
}

pub struct ContactInfo {
    pub user_id: String,
    pub name: String,
    pub devices: Vec<String>,
    pub is_me: bool,
}

pub struct IceConfig {
    pub stun: Vec<String>,
}

struct Running {
    shared: Arc<Shared>,
    tx: mpsc::UnboundedSender<Cmd>,
    task: tokio::task::JoinHandle<()>,
}

static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime")
});

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Rust panics go to `<data_dir>/errors.log` (the app's error journal, shared with Dart and
/// Kotlin), then to the default hook. Location and message only: never message contents.
fn log_panics(data_dir: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let path = std::path::Path::new(data_dir).join("errors.log");
    ONCE.call_once(move || {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            use std::io::Write;
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_default();
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{} [rust] panic at {at}: {msg}", config::now());
            }
            default(info);
        }));
    });
}

fn convert(e: Event) -> ClientEvent {
    match e {
        Event::Connecting { server } => ClientEvent::Connecting { server },
        Event::Online { server } => ClientEvent::Online { server },
        Event::Offline { reason } => ClientEvent::Offline { reason },
        Event::ContactsChanged => ClientEvent::ContactsChanged,
        Event::Signal { from_user, from_device, payload } => ClientEvent::Signal {
            from_user: from_user.to_hex(),
            from_device: from_device.to_hex(),
            payload,
        },
        Event::Signaled { to, online } => ClientEvent::Signaled { to_device: to.to_hex(), online },
        Event::Presence { online } => ClientEvent::Presence { online: online.iter().map(Id::to_hex).collect() },
        Event::Turn { urls, username, credential } => ClientEvent::Turn { urls, username, credential },
        Event::Error { message } => ClientEvent::Error { message },
        Event::MessagesChanged { peer } => ClientEvent::MessagesChanged { peer_user: peer.to_hex() },
        Event::NewMessage { peer, text } => ClientEvent::NewMessage { peer_user: peer.to_hex(), text },
    }
}

/// Starts (or restarts) the relay client. Events arrive on `sink` until `client_stop`.
/// `db_key`: 32 random bytes from platform secure storage (encrypts message bodies at rest).
pub fn client_start(
    data_dir: String,
    secret: Vec<u8>,
    cert: String,
    db_key: Vec<u8>,
    sink: StreamSink<ClientEvent>,
) -> anyhow::Result<()> {
    client_stop();
    log_panics(&data_dir);
    let me = DeviceSecret::from_bytes(&secret).map_err(|e| anyhow::anyhow!("device secret: {e}"))?;
    let cert = client::parse_cert(&cert)?;
    let manifest_path = std::path::Path::new(&data_dir).join("manifest.bin");
    let manifest = client::load_manifest_from(&manifest_path)?;
    let now = config::now();
    let mut contacts = Contacts::load(std::path::Path::new(&data_dir).join("contacts.json"), config::root_pub(), now);
    contacts.merge(std::slice::from_ref(&cert), now, &manifest);
    let my_user = client::my_cert(&cert)?.user_id;
    let db_key: [u8; 32] = db_key.try_into().map_err(|_| anyhow::anyhow!("db key must be 32 bytes"))?;
    let store = client::store::Store::open(&std::path::Path::new(&data_dir).join("botato.db"), db_key)?;
    let shared = Arc::new(Shared {
        me,
        cert,
        contacts: Mutex::new(contacts),
        manifest: std::sync::RwLock::new(Arc::new(manifest)),
        manifest_path,
        reconnect: tokio::sync::Notify::new(),
        store: Mutex::new(store),
        my_user,
        push_token: Mutex::new(None),
    });

    let (tx, rx) = mpsc::unbounded_channel();
    let emit: client::Emit = Arc::new(move |e| {
        let _ = sink.add(convert(e));
    });
    let task = RT.spawn(client::run(shared.clone(), rx, emit.clone()));
    *RUNNING.lock().unwrap() = Some(Running { shared, tx, task });
    // Dart can read contacts only now that the client is registered.
    emit(Event::ContactsChanged);
    Ok(())
}

pub fn client_stop() {
    if let Some(r) = RUNNING.lock().unwrap().take() {
        r.task.abort();
    }
}

fn with<T>(f: impl FnOnce(&Running) -> T) -> anyhow::Result<T> {
    let guard = RUNNING.lock().unwrap();
    let r = guard.as_ref().ok_or_else(|| anyhow::anyhow!("клиент не запущен"))?;
    Ok(f(r))
}

fn parse_id(s: &str) -> anyhow::Result<Id> {
    Id::from_hex(s).ok_or_else(|| anyhow::anyhow!("bad id"))
}

#[flutter_rust_bridge::frb(sync)]
pub fn client_contacts() -> Vec<ContactInfo> {
    with(|r| {
        let me = r.shared.me.device_id;
        let contacts = r.shared.contacts.lock().unwrap();
        let mut list: Vec<ContactInfo> = contacts
            .all()
            .map(|c| ContactInfo {
                user_id: c.user_id.to_hex(),
                name: c.name.clone(),
                devices: c.devices.iter().map(|d| d.device_id.to_hex()).collect(),
                is_me: c.device(&me).is_some(),
            })
            .collect();
        list.sort_by(|a, b| (b.is_me, &a.name).cmp(&(a.is_me, &b.name)));
        list
    })
    .unwrap_or_default()
}

/// Seals `payload` (call-signal JSON) to one device and sends it via the relay.
#[flutter_rust_bridge::frb(sync)]
pub fn client_signal(to_device: String, payload: String) -> anyhow::Result<()> {
    let to = parse_id(&to_device)?;
    with(|r| r.tx.send(Cmd::Signal { to, payload: payload.into_bytes() }))?.map_err(|_| anyhow::anyhow!("клиент остановлен"))
}

/// Network changed: drop the (possibly silently dead) relay connection and dial again now.
#[flutter_rust_bridge::frb(sync)]
pub fn client_reconnect() {
    let _ = with(|r| r.shared.reconnect.notify_one());
}

#[flutter_rust_bridge::frb(sync)]
pub fn client_request_turn() -> anyhow::Result<()> {
    with(|r| r.tx.send(Cmd::Turn))?.map_err(|_| anyhow::anyhow!("клиент остановлен"))
}

#[flutter_rust_bridge::frb(sync)]
pub fn client_refresh_directory() -> anyhow::Result<()> {
    with(|r| r.tx.send(Cmd::Directory))?.map_err(|_| anyhow::anyhow!("клиент остановлен"))
}

/// Asks the relay which of these devices are connected; answered by `ClientEvent::Presence`.
#[flutter_rust_bridge::frb(sync)]
pub fn client_query_presence(devices: Vec<String>) -> anyhow::Result<()> {
    let ids = devices.iter().map(|d| parse_id(d)).collect::<anyhow::Result<Vec<_>>>()?;
    with(|r| r.tx.send(Cmd::Presence(ids)))?.map_err(|_| anyhow::anyhow!("клиент остановлен"))
}

/// This device's id (hex).
#[flutter_rust_bridge::frb(sync)]
pub fn client_my_device() -> Option<String> {
    with(|r| r.shared.me.device_id.to_hex()).ok()
}

/// `https://<relay host>/manifest` for every server in the current manifest.
#[flutter_rust_bridge::frb(sync)]
pub fn client_manifest_urls() -> Vec<String> {
    with(|r| {
        r.shared
            .manifest()
            .servers
            .iter()
            .filter_map(|s| {
                let host = s.url.strip_prefix("wss://")?.split('/').next()?;
                Some(format!("https://{host}/manifest"))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// A manifest fetched from a relay (or any channel): applied only if signed by the admin and
/// newer. Returns true if it replaced the current one. Revocations apply at once; new servers
/// from the next connection.
#[flutter_rust_bridge::frb(sync)]
pub fn client_offer_manifest(raw: Vec<u8>) -> anyhow::Result<bool> {
    let changed = with(|r| r.shared.offer_manifest(&raw))??;
    if changed {
        let _ = with(|r| r.tx.send(Cmd::Directory));
    }
    Ok(changed)
}

/// STUN servers from the manifest; TURN comes via `client_request_turn`.
#[flutter_rust_bridge::frb(sync)]
pub fn client_ice_config() -> IceConfig {
    IceConfig { stun: with(|r| r.shared.manifest().stun.clone()).unwrap_or_default() }
}

// ---- messages

pub struct MessageInfo {
    pub msg_id: String,
    pub from_me: bool,
    pub text: String,
    /// Sender's clock, unix seconds.
    pub sent_at: i64,
    /// 0 sending, 1 sent, 2 delivered, 3 failed, 10 received.
    pub status: i64,
    pub photo: Option<PhotoInfo>,
    pub voice: Option<VoiceInfo>,
    pub edited: bool,
    /// Deleted for everyone: show a placeholder.
    pub deleted: bool,
    pub reply: Option<ReplyInfo>,
    /// Display name of the original author, if forwarded.
    pub forwarded_from: Option<String>,
    pub reactions: Vec<ReactionInfo>,
}

pub struct ReplyInfo {
    pub msg_id: String,
    pub from_user: String,
    /// Snapshot of the quoted text (≤ 200 chars).
    pub text: String,
}

pub struct ReactionInfo {
    pub user_id: String,
    pub emoji: String,
}

pub struct PhotoInfo {
    pub width: u32,
    pub height: u32,
    /// Small JPEG preview, shown until the photo is downloaded.
    pub thumb: Vec<u8>,
    /// The full photo is on this device (`client_photo`).
    pub ready: bool,
    /// Expired on the relay before we got it.
    pub gone: bool,
}

pub struct VoiceInfo {
    pub duration_ms: u32,
    /// Loudness bars, 0..=255.
    pub waveform: Vec<u8>,
    /// The audio is on this device (`client_photo` returns it).
    pub ready: bool,
    pub gone: bool,
}

pub struct ConversationInfo {
    pub peer_user: String,
    pub last_text: String,
    pub last_at: i64,
    pub last_from_me: bool,
    pub unread: i64,
}

/// Queues a text message to every device of `peer_user` (and our other devices);
/// `reply_to` quotes one of the conversation's messages.
#[flutter_rust_bridge::frb(sync)]
pub fn client_send_text(peer_user: String, text: String, reply_to: Option<String>) -> anyhow::Result<String> {
    let peer = parse_id(&peer_user)?;
    let reply = reply_to.as_deref().map(parse_id).transpose()?;
    let guard = RUNNING.lock().unwrap();
    let r = guard.as_ref().ok_or_else(|| anyhow::anyhow!("клиент не запущен"))?;
    let msg_id = client::messaging::compose(&r.shared, &peer, &text, reply.as_ref())?;
    let _ = r.tx.send(Cmd::Flush);
    Ok(msg_id.to_hex())
}

/// Sends a copy of a stored message (text and/or photo) to `to_peer`, marked "forwarded from
/// `from_name`". Photos are re-uploaded: call it off the UI isolate.
pub fn client_forward(msg_id: String, to_peer: String, from_name: String) -> anyhow::Result<String> {
    let (msg, to) = (parse_id(&msg_id)?, parse_id(&to_peer)?);
    let (shared, tx) = running()?;
    let id = client::messaging::forward(&shared, &msg, &to, &from_name)?;
    let _ = tx.send(Cmd::Flush);
    Ok(id.to_hex())
}

/// Replaces the text of one of our messages, here and on every device of the conversation.
#[flutter_rust_bridge::frb(sync)]
pub fn client_edit(peer_user: String, msg_id: String, text: String) -> anyhow::Result<()> {
    send_update(&peer_user, &msg_id, client::messaging::Update::Edit(text))
}

/// Deletes a message: `for_all` (only our own) everywhere, otherwise on this device only.
#[flutter_rust_bridge::frb(sync)]
pub fn client_delete(peer_user: String, msg_id: String, for_all: bool) -> anyhow::Result<()> {
    if for_all {
        return send_update(&peer_user, &msg_id, client::messaging::Update::Delete);
    }
    let id = parse_id(&msg_id)?;
    with(|r| r.shared.store.lock().unwrap().delete_local(&id).map(|_| ()))?
}

/// Sets our reaction on a message; an empty `emoji` removes it.
#[flutter_rust_bridge::frb(sync)]
pub fn client_react(peer_user: String, msg_id: String, emoji: String) -> anyhow::Result<()> {
    send_update(&peer_user, &msg_id, client::messaging::Update::React(emoji))
}

fn send_update(peer_user: &str, msg_id: &str, op: client::messaging::Update) -> anyhow::Result<()> {
    let (peer, msg) = (parse_id(peer_user)?, parse_id(msg_id)?);
    let (shared, tx) = running()?;
    client::messaging::update(&shared, &peer, &msg, op)?;
    let _ = tx.send(Cmd::Flush);
    Ok(())
}

fn running() -> anyhow::Result<(Arc<Shared>, mpsc::UnboundedSender<Cmd>)> {
    let guard = RUNNING.lock().unwrap();
    let r = guard.as_ref().ok_or_else(|| anyhow::anyhow!("клиент не запущен"))?;
    Ok((r.shared.clone(), r.tx.clone()))
}

/// Queues a photo (any common image format; re-encoded, metadata dropped) with an optional caption.
/// `keep_png`: send a PNG as PNG (false when the PNG is just an intermediate of a photo).
/// Runs the image work on the caller's thread: call it off the UI isolate.
pub fn client_send_photo(
    peer_user: String,
    image: Vec<u8>,
    caption: String,
    reply_to: Option<String>,
    keep_png: bool,
) -> anyhow::Result<String> {
    let peer = parse_id(&peer_user)?;
    let reply = reply_to.as_deref().map(parse_id).transpose()?;
    let (shared, tx) = running()?;
    let msg_id = client::messaging::compose_photo(&shared, &peer, &image, &caption, reply.as_ref(), keep_png)?;
    let _ = tx.send(Cmd::Flush);
    Ok(msg_id.to_hex())
}

pub struct SearchHit {
    pub peer_user: String,
    pub msg_id: String,
    pub text: String,
    pub sent_at: i64,
    pub from_me: bool,
}

/// Messages containing `query` (case-insensitive), newest first: in one conversation, or in
/// all when `peer_user` is None. At most `limit` hits.
pub fn client_search(peer_user: Option<String>, query: String, limit: u32) -> anyhow::Result<Vec<SearchHit>> {
    let peer = peer_user.as_deref().map(parse_id).transpose()?;
    let hits = with(|r| r.shared.store.lock().unwrap().search(peer.as_ref(), &query, limit as usize))??;
    Ok(hits
        .into_iter()
        .map(|(p, m)| SearchHit { peer_user: p.to_hex(), msg_id: m.msg_id.to_hex(), text: m.text, sent_at: m.sent_at, from_me: m.from_me })
        .collect())
}

/// Queues a voice message: `audio` as the recorder wrote it (AAC in M4A), its duration and a
/// waveform for the bubble. Not re-encoded.
pub fn client_send_voice(
    peer_user: String,
    audio: Vec<u8>,
    duration_ms: u32,
    waveform: Vec<u8>,
    reply_to: Option<String>,
) -> anyhow::Result<String> {
    let peer = parse_id(&peer_user)?;
    let reply = reply_to.as_deref().map(parse_id).transpose()?;
    let (shared, tx) = running()?;
    let msg_id = client::messaging::compose_voice(&shared, &peer, audio, duration_ms, waveform, reply.as_ref())?;
    let _ = tx.send(Cmd::Flush);
    Ok(msg_id.to_hex())
}

/// The full JPEG of a photo message, if downloaded.
pub fn client_photo(msg_id: String) -> anyhow::Result<Option<Vec<u8>>> {
    let id = parse_id(&msg_id)?;
    with(|r| r.shared.store.lock().unwrap().photo_data(&id))?
}

/// Oldest first; `before` (sent_at) pages backwards.
#[flutter_rust_bridge::frb(sync)]
pub fn client_messages(peer_user: String, before: Option<i64>, limit: i64) -> anyhow::Result<Vec<MessageInfo>> {
    let peer = parse_id(&peer_user)?;
    with(|r| r.shared.store.lock().unwrap().messages(&peer, before, limit))??
        .into_iter()
        .map(|m| {
            Ok(MessageInfo {
                msg_id: m.msg_id.to_hex(),
                from_me: m.from_me,
                text: m.text,
                sent_at: m.sent_at,
                status: m.status,
                voice: m.photo.as_ref().filter(|p| p.kind == client::store::KIND_VOICE).map(|p| VoiceInfo {
                    duration_ms: p.w,
                    waveform: p.thumb.clone(),
                    ready: p.ready,
                    gone: p.gone,
                }),
                photo: m
                    .photo
                    .filter(|p| p.kind == client::store::KIND_PHOTO)
                    .map(|p| PhotoInfo { width: p.w, height: p.h, thumb: p.thumb, ready: p.ready, gone: p.gone }),
                edited: m.edited,
                deleted: m.deleted,
                reply: m.extra.reply.map(|r| ReplyInfo { msg_id: r.msg.to_hex(), from_user: r.from.to_hex(), text: r.text }),
                forwarded_from: m.extra.fwd,
                reactions: m
                    .reactions
                    .into_iter()
                    .map(|(u, e)| ReactionInfo { user_id: u.to_hex(), emoji: e })
                    .collect(),
            })
        })
        .collect()
}

#[flutter_rust_bridge::frb(sync)]
pub fn client_conversations() -> Vec<ConversationInfo> {
    with(|r| r.shared.store.lock().unwrap().conversations().unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .map(|c| ConversationInfo {
            peer_user: c.peer_user.to_hex(),
            last_text: if c.last_deleted {
                "сообщение удалено".into()
            } else {
                client::messaging::preview(&c.last_text, c.last_attach)
            },
            last_at: c.last_at,
            last_from_me: c.last_from_me,
            unread: c.unread,
        })
        .collect()
}

#[flutter_rust_bridge::frb(sync)]
pub fn client_mark_read(peer_user: String) -> anyhow::Result<()> {
    let peer = parse_id(&peer_user)?;
    with(|r| r.shared.store.lock().unwrap().mark_read(&peer))?
}

/// Forgets the local history with `peer_user` (both directions, and pending copies).
#[flutter_rust_bridge::frb(sync)]
pub fn client_delete_history(peer_user: String) -> anyhow::Result<()> {
    let peer = parse_id(&peer_user)?;
    with(|r| r.shared.store.lock().unwrap().delete_conversation(&peer))?
}

/// FCM registration token (Android). Kept in memory and sent to the relay on every connect.
#[flutter_rust_bridge::frb(sync)]
pub fn client_set_push_token(token: String) -> anyhow::Result<()> {
    with(|r| {
        *r.shared.push_token.lock().unwrap() = Some(token);
        let _ = r.tx.send(Cmd::PushToken);
    })
}
