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
        Event::Turn { urls, username, credential } => ClientEvent::Turn { urls, username, credential },
        Event::Error { message } => ClientEvent::Error { message },
    }
}

/// Starts (or restarts) the relay client. Events arrive on `sink` until `client_stop`.
pub fn client_start(data_dir: String, secret: Vec<u8>, cert: String, sink: StreamSink<ClientEvent>) -> anyhow::Result<()> {
    client_stop();
    let me = DeviceSecret::from_bytes(&secret).map_err(|e| anyhow::anyhow!("device secret: {e}"))?;
    let cert = client::parse_cert(&cert)?;
    let manifest = client::load_manifest()?;
    let now = config::now();
    let mut contacts = Contacts::load(std::path::Path::new(&data_dir).join("contacts.json"), config::root_pub(), now);
    contacts.merge(std::slice::from_ref(&cert), now, &manifest);
    let shared = Arc::new(Shared { me, cert, contacts: Mutex::new(contacts), manifest });

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

#[flutter_rust_bridge::frb(sync)]
pub fn client_request_turn() -> anyhow::Result<()> {
    with(|r| r.tx.send(Cmd::Turn))?.map_err(|_| anyhow::anyhow!("клиент остановлен"))
}

#[flutter_rust_bridge::frb(sync)]
pub fn client_refresh_directory() -> anyhow::Result<()> {
    with(|r| r.tx.send(Cmd::Directory))?.map_err(|_| anyhow::anyhow!("клиент остановлен"))
}

/// This device's id (hex).
#[flutter_rust_bridge::frb(sync)]
pub fn client_my_device() -> Option<String> {
    with(|r| r.shared.me.device_id.to_hex()).ok()
}

/// STUN servers from the manifest; TURN comes via `client_request_turn`.
#[flutter_rust_bridge::frb(sync)]
pub fn client_ice_config() -> IceConfig {
    IceConfig { stun: with(|r| r.shared.manifest.stun.clone()).unwrap_or_default() }
}
