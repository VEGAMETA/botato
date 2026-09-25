//! Nostr fallback (protocol/README.md §7): call signals and outbox copies (messages, receipts,
//! updates) over public Nostr relays when our relay can't carry them, and the signed manifest
//! from there when our server is gone.
//!
//! One task keeps a WebSocket per relay of the manifest's list while the app is active,
//! subscribed to the tags our peers would address us with. Everything that arrives is an
//! ordinary envelope, opened and verified exactly like one from our relay.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use proto::Id;
use proto::nostr::{self as pn, Event as NEvent};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use super::{Emit, Event, Shared};
use crate::config;

/// Signals seen (by envelope hash) are remembered this long: the same envelope may come over
/// several relays and over our relay too.
const SEEN_FOR: Duration = Duration::from_secs(600);
/// Our relay unreachable this long → look for a newer manifest on Nostr, then every few minutes.
const MANIFEST_AFTER: Duration = if cfg!(test) { Duration::from_secs(1) } else { Duration::from_secs(60) };
const MANIFEST_EVERY: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(300) };
/// A publish while inactive keeps the relays connected this long (the answer may come back).
const PUBLISH_KEEPS: Duration = Duration::from_secs(120);
/// Our relay unreachable this long → the outbox also goes out over Nostr.
const OUTBOX_AFTER: Duration = if cfg!(test) { Duration::from_secs(1) } else { Duration::from_secs(15) };
/// Message events published only until then are looked for this far back (their lifetime).
const MESSAGES_SINCE: u64 = pn::MESSAGE_TTL;
/// A device heard from over Nostr this recently is written to over Nostr too.
const NOSTR_PEER_FOR: Duration = Duration::from_secs(600);
/// A device our relay has reported offline this long is written to over Nostr too.
const PEER_OFFLINE_AFTER: Duration = if cfg!(test) { Duration::from_secs(1) } else { Duration::from_secs(30) };

pub enum Cmd {
    /// The app is active (desktop, phone in front, or a call): stay subscribed.
    Active(bool),
    /// An envelope to publish as a signal event (already sealed for `to`).
    Signal { to: Id, blob: Vec<u8> },
}

/// The part of the client state the Nostr task shares with the relay session.
pub struct Link {
    tx: mpsc::UnboundedSender<Cmd>,
    /// Our relay is connected (authenticated) right now.
    online: AtomicBool,
    /// Since when our relay has been unreachable.
    offline_since: Mutex<Option<Instant>>,
    seen: Mutex<HashMap<[u8; 32], Instant>>,
    /// Asks the relay session to send the outbox now (a receipt owed for a Nostr delivery).
    pub flush: tokio::sync::Notify,
    /// Devices we lately got something from over Nostr.
    heard_on_nostr: Mutex<HashMap<Id, Instant>>,
    /// Devices our relay reports offline, and since when (from presence answers).
    offline_peers: Mutex<HashMap<Id, Instant>>,
}

impl Link {
    pub fn new() -> (Link, mpsc::UnboundedReceiver<Cmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let link = Link {
            tx,
            online: AtomicBool::new(false),
            offline_since: Mutex::new(Some(Instant::now())),
            seen: Mutex::default(),
            flush: tokio::sync::Notify::new(),
            heard_on_nostr: Mutex::default(),
            offline_peers: Mutex::default(),
        };
        (link, rx)
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn set_online(&self, on: bool) {
        self.online.store(on, Ordering::Relaxed);
        let mut since = self.offline_since.lock().unwrap();
        if on {
            *since = None;
        } else if since.is_none() {
            *since = Some(Instant::now());
        }
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// A presence answer: of `asked`, only `online` are connected to our relay.
    pub fn presence(&self, asked: &[Id], online: &[Id]) {
        let mut off = self.offline_peers.lock().unwrap();
        for d in asked {
            if online.contains(d) {
                off.remove(d);
            } else {
                off.entry(*d).or_insert_with(Instant::now);
            }
        }
    }

    fn heard(&self, device: Id) {
        self.heard_on_nostr.lock().unwrap().insert(device, Instant::now());
    }

    /// Our relay can't reach `device` (it lives on Nostr lately, or has been offline for a
    /// while): copies to it go over Nostr too, even while we are connected.
    fn peer_needs_nostr(&self, device: &Id) -> bool {
        self.heard_on_nostr.lock().unwrap().get(device).is_some_and(|t| t.elapsed() < NOSTR_PEER_FOR)
            || self.offline_peers.lock().unwrap().get(device).is_some_and(|t| t.elapsed() >= PEER_OFFLINE_AFTER)
    }

    fn offline_for(&self) -> Option<Duration> {
        self.offline_since.lock().unwrap().map(|t| t.elapsed())
    }

    /// True the first time this envelope is seen (within [`SEEN_FOR`]).
    pub fn first_seen(&self, blob: &[u8]) -> bool {
        let h: [u8; 32] = Sha256::digest(blob).into();
        let mut seen = self.seen.lock().unwrap();
        seen.retain(|_, t| t.elapsed() < SEEN_FOR);
        seen.insert(h, Instant::now()).is_none()
    }
}

/// Publishes an envelope for `to` over Nostr (no-op without relays in the manifest).
pub fn publish_signal(shared: &Shared, to: Id, blob: Vec<u8>) {
    shared.nostr.send(Cmd::Signal { to, blob });
}

/// The tag a signal to `to` carries today; `None` if `to` is unknown.
fn tag_for(shared: &Shared, to: &Id, now: u64) -> Option<String> {
    let contacts = shared.contacts.lock().unwrap();
    let (_, dev) = contacts.device(to, &shared.manifest())?;
    Some(shared.me.nostr_pair(&dev.enc_pub)?.tag(to, pn::day(now)))
}

/// Tags our known peer devices (own other devices too) address us with: yesterday, today, tomorrow.
fn own_tags(shared: &Shared, now: u64) -> Vec<String> {
    let me = shared.me.device_id;
    let manifest = shared.manifest();
    let contacts = shared.contacts.lock().unwrap();
    let today = pn::day(now);
    let mut tags = Vec::new();
    for cert in contacts.all() {
        for dev in &cert.devices {
            if dev.device_id == me || manifest.is_revoked(&dev.device_id) {
                continue;
            }
            if let Some(k) = shared.me.nostr_pair(&dev.enc_pub) {
                for d in [today - 1, today, today + 1] {
                    tags.push(k.tag(&me, d));
                }
            }
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

fn signal_filter(tags: &[String], now: u64) -> Value {
    json!([
        "REQ",
        "s",
        {"kinds": [pn::KIND_SIGNAL], "#t": tags, "since": now.saturating_sub(60)},
        {"kinds": [pn::KIND_MESSAGE], "#t": tags, "since": now.saturating_sub(MESSAGES_SINCE)}
    ])
}

/// Runs for the client's lifetime.
pub async fn run(shared: std::sync::Arc<Shared>, cmds: mpsc::UnboundedReceiver<Cmd>, emit: Emit) {
    run_with_root(shared, cmds, emit, config::root_pub()).await
}

/// `root_pub`: the admin key manifests from Nostr must be signed with.
pub async fn run_with_root(shared: std::sync::Arc<Shared>, mut cmds: mpsc::UnboundedReceiver<Cmd>, emit: Emit, root_pub: [u8; 32]) {
    let (out_tx, _) = broadcast::channel::<String>(64);
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<String>();
    let (tags_tx, tags_rx) = watch::channel(own_tags(&shared, config::now()));
    let mut active = cfg!(not(target_os = "android")); // desktop: always; Android: Dart says
    let mut keep_until: Option<Instant> = None;
    let mut relays: Vec<String> = Vec::new();
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut last_manifest_query: Option<Instant> = None;
    let d_tag = pn::manifest_d_tag(&root_pub);
    let mut tick = tokio::time::interval(if cfg!(test) { Duration::from_millis(500) } else { Duration::from_secs(5) });
    // Outbox copies already published during this outage.
    let mut published: std::collections::HashSet<Id> = std::collections::HashSet::new();

    loop {
        // Connections follow the wanted state: active (or a recent publish) and a relay list.
        let want = active || keep_until.is_some_and(|t| Instant::now() < t) || shared.nostr.offline_for().is_some_and(|d| d >= MANIFEST_AFTER);
        let list = shared.manifest().nostr_relays.clone();
        if !want || list != relays {
            for t in tasks.drain(..) {
                t.abort();
            }
            relays.clear();
        }
        if want && tasks.is_empty() && !list.is_empty() {
            relays = list;
            for url in &relays {
                tasks.push(tokio::spawn(relay_task(url.clone(), out_tx.subscribe(), in_tx.clone(), tags_rx.clone())));
            }
        }

        tokio::select! {
            cmd = cmds.recv() => match cmd {
                None => break,
                Some(Cmd::Active(a)) => active = a,
                Some(Cmd::Signal { to, blob }) => {
                    let now = config::now();
                    let Some(tag) = tag_for(&shared, &to, now) else { continue };
                    let Ok(ev) = pn::signal_event(&blob, &tag, now) else { continue };
                    keep_until = Some(Instant::now() + PUBLISH_KEEPS);
                    // Now, and again for relays that are still connecting (or were not
                    // connected at all); receivers drop repeats by the envelope hash.
                    let text = json!(["EVENT", ev.to_json()]).to_string();
                    let _ = out_tx.send(text.clone());
                    let out = out_tx.clone();
                    tokio::spawn(async move {
                        for wait in [3, 5] {
                            tokio::time::sleep(Duration::from_secs(wait)).await;
                            let _ = out.send(text.clone());
                        }
                    });
                }
            },
            Some(text) = in_rx.recv() => handle(&shared, &text, &d_tag, &root_pub, &emit),
            _ = tick.tick() => {
                // New contacts, a new day or a new manifest: resubscribe if the tags changed.
                let tags = own_tags(&shared, config::now());
                if *tags_tx.borrow() != tags {
                    let _ = tags_tx.send(tags);
                }
                // Our server gone for a while: ask Nostr for a newer manifest.
                if shared.nostr.offline_for().is_some_and(|d| d >= MANIFEST_AFTER)
                    && last_manifest_query.is_none_or(|t| t.elapsed() >= MANIFEST_EVERY)
                {
                    last_manifest_query = Some(Instant::now());
                    let _ = out_tx.send(json!(["REQ", "m", {"kinds": [pn::KIND_MANIFEST], "#d": [d_tag], "limit": 20}]).to_string());
                }
                if shared.nostr.online() {
                    last_manifest_query = None;
                }
                // What waits in the outbox goes over Nostr when we have no server, or for peers
                // our server can't reach; the receipt that comes back clears it, as over our relay.
                let self_off = shared.nostr.offline_for().is_some_and(|d| d >= OUTBOX_AFTER);
                if !shared.manifest().nostr_relays.is_empty() {
                    let rows = shared.store.lock().unwrap().outbox().unwrap_or_default();
                    published.retain(|id| rows.iter().any(|r| r.id == *id));
                    let now = config::now();
                    for row in rows {
                        if published.contains(&row.id) || !(self_off || shared.nostr.peer_needs_nostr(&row.to_device)) {
                            continue;
                        }
                        let Some(tag) = tag_for(&shared, &row.to_device, now) else { continue };
                        let Ok(ev) = pn::message_event(&row.blob, &tag, now) else { continue };
                        published.insert(row.id);
                        keep_until = Some(Instant::now() + PUBLISH_KEEPS);
                        let _ = out_tx.send(json!(["EVENT", ev.to_json()]).to_string());
                    }
                }
            }
        }
    }
    for t in tasks {
        t.abort();
    }
}

fn handle(shared: &Shared, text: &str, d_tag: &str, root_pub: &[u8; 32], emit: &Emit) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    if v[0] != "EVENT" {
        return;
    }
    let Ok(ev) = NEvent::from_json(&v[2]) else { return };
    let Ok(payload) = ev.payload() else { return };
    match ev.kind {
        pn::KIND_SIGNAL => {
            if !shared.nostr.first_seen(&payload) {
                return;
            }
            if let Ok(Some(e)) = super::open_signal(shared, &payload) {
                if let Event::Signal { from_device, .. } = &e {
                    shared.nostr.heard(*from_device);
                }
                emit(e);
            }
        }
        pn::KIND_MESSAGE => {
            if !shared.nostr.first_seen(&payload) {
                return;
            }
            // Exactly as a mailbox delivery; an unknown sender waits (no directory offline).
            if let Ok(super::messaging::Delivered::Done { changed, new_text }) = super::messaging::on_deliver(shared, &payload) {
                // Verified inside on_deliver when it changed something: the sender lives on
                // Nostr now, so what we owe it (the receipt first) goes there too.
                if changed.is_some() {
                    if let Ok(u) = proto::envelope::open(&shared.me, &payload) {
                        shared.nostr.heard(u.sender());
                    }
                }
                if let Some(peer) = changed {
                    emit(Event::MessagesChanged { peer });
                }
                if let Some((peer, text)) = new_text {
                    emit(Event::NewMessage { peer, text });
                }
                // The receipt we owe is in the outbox: our relay sends it now if connected,
                // otherwise the next tick publishes it here.
                shared.nostr.flush.notify_one();
            }
        }
        pn::KIND_MANIFEST if ev.tag("d") == Some(d_tag) => {
            // Accepted only with the admin's signature and a higher version.
            if let Ok(true) = shared.offer_manifest_signed_by(&payload, root_pub) {
                emit(Event::ContactsChanged);
                shared.reconnect.notify_one();
            }
        }
        _ => {}
    }
}

/// One relay: connect, subscribe to our tags, publish what comes on `out`, forward what
/// arrives to `inbox`; reconnect with backoff.
async fn relay_task(url: String, mut out: broadcast::Receiver<String>, inbox: mpsc::UnboundedSender<String>, mut tags: watch::Receiver<Vec<String>>) {
    let mut backoff = 5u64;
    loop {
        if let Ok(Ok((mut ws, _))) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(&url)).await {
            backoff = 5;
            let _ = serve(&mut ws, &mut out, &inbox, &mut tags).await;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(120);
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(
    ws: &mut Ws,
    out: &mut broadcast::Receiver<String>,
    inbox: &mpsc::UnboundedSender<String>,
    tags: &mut watch::Receiver<Vec<String>>,
) -> anyhow::Result<()> {
    let subscribe = |t: &[String]| signal_filter(t, config::now()).to_string();
    let current = tags.borrow_and_update().clone();
    if !current.is_empty() {
        ws.send(Message::Text(subscribe(&current).into())).await?;
    }
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    ping.reset();
    loop {
        tokio::select! {
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(t))) => { let _ = inbox.send(t.to_string()); }
                Some(Ok(Message::Close(_))) | None => anyhow::bail!("closed"),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
            },
            o = out.recv() => match o {
                Ok(text) => ws.send(Message::Text(text.into())).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => anyhow::bail!("stopped"),
            },
            changed = tags.changed() => {
                if changed.is_err() { anyhow::bail!("stopped"); }
                let t = tags.borrow_and_update().clone();
                // Same subscription id: the relay replaces the old filter.
                if !t.is_empty() { ws.send(Message::Text(subscribe(&t).into())).await?; }
            }
            _ = ping.tick() => ws.send(Message::Ping(Default::default())).await?,
        }
    }
}
