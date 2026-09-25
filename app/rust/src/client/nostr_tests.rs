//! Nostr fallback end to end: our relay unreachable, a minimal Nostr relay in-process, real
//! clients. A call signal must get through, once, and a newer manifest must be picked up.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use proto::cert::{Cert, Device};
use proto::device::DeviceSecret;
use proto::manifest::{Manifest, ServerEntry};
use proto::{Id, SigningKey};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use super::contacts::Contacts;
use super::store::Store;
use super::{Emit, Event, Shared, nostr};
use crate::config;

/// NIP-01 subset: REQ with `kinds` and `#t`/`#d`, EVENT fan-out to matching subscriptions.
async fn mock_relay() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (bus, _) = broadcast::channel::<Value>(256);
    let stored = Arc::new(Mutex::new(Vec::<Value>::new())); // replaceable events are kept
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let bus = bus.clone();
            let stored = stored.clone();
            tokio::spawn(async move {
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let mut rx = bus.subscribe();
                let mut subs: Vec<(String, Value)> = Vec::new();
                let matches = |f: &Value, ev: &Value| {
                    let kind_ok = f["kinds"].as_array().is_none_or(|k| k.contains(&ev["kind"]));
                    let tag_ok = ["t", "d"].iter().all(|n| {
                        f.get(format!("#{n}")).and_then(Value::as_array).is_none_or(|want| {
                            ev["tags"].as_array().unwrap().iter().any(|t| t[0] == *n && want.contains(&t[1]))
                        })
                    });
                    kind_ok && tag_ok
                };
                loop {
                    tokio::select! {
                        m = ws.next() => {
                            let Some(Ok(Message::Text(t))) = m else { return };
                            let v: Value = serde_json::from_str(t.as_str()).unwrap();
                            match v[0].as_str().unwrap() {
                                "REQ" => {
                                    let id = v[1].as_str().unwrap().to_string();
                                    subs.retain(|(s, _)| *s != id);
                                    let old: Vec<Value> = stored.lock().unwrap().iter().filter(|e| matches(&v[2], e)).cloned().collect();
                                    for e in old {
                                        ws.send(Message::Text(json!(["EVENT", id, e]).to_string().into())).await.unwrap();
                                    }
                                    ws.send(Message::Text(json!(["EOSE", id]).to_string().into())).await.unwrap();
                                    subs.push((id, v[2].clone()));
                                }
                                "EVENT" => {
                                    let ev = v[1].clone();
                                    proto::nostr::Event::from_json(&ev).unwrap().verify().unwrap();
                                    if ev["kind"].as_u64().unwrap() >= 30000 {
                                        stored.lock().unwrap().push(ev.clone());
                                    }
                                    ws.send(Message::Text(json!(["OK", ev["id"], true, ""]).to_string().into())).await.unwrap();
                                    let _ = bus.send(ev);
                                }
                                _ => {}
                            }
                        }
                        Ok(ev) = rx.recv() => {
                            for (id, f) in &subs {
                                if matches(f, &ev) {
                                    let _ = ws.send(Message::Text(json!(["EVENT", id, ev]).to_string().into())).await;
                                }
                            }
                        }
                    }
                }
            });
        }
    });
    format!("ws://{addr}")
}

struct Node {
    shared: Arc<Shared>,
    events: mpsc::UnboundedReceiver<Event>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

fn node(root: &SigningKey, manifest: &Manifest, me: DeviceSecret, user: Id, certs: &[Vec<u8>], dir: &std::path::Path) -> Node {
    let now = config::now();
    let mut contacts = Contacts::load(dir.join("contacts.json"), root.verifying_key().to_bytes(), now);
    contacts.merge(certs, now, manifest);
    let (link, nrx) = nostr::Link::new();
    let shared = Arc::new(Shared {
        me,
        cert: certs[0].clone(),
        contacts: Mutex::new(contacts),
        manifest: std::sync::RwLock::new(Arc::new(manifest.clone())),
        manifest_path: dir.join("manifest.bin"),
        reconnect: tokio::sync::Notify::new(),
        store: Mutex::new(Store::open(&dir.join("botato.db"), [9; 32]).unwrap()),
        my_user: user,
        push_token: Mutex::new(None),
        nostr: link,
    });
    let (etx, events) = mpsc::unbounded_channel();
    let emit: Emit = Arc::new(move |e| {
        let _ = etx.send(e);
    });
    let (_cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let tasks = vec![
        tokio::spawn(super::run(shared.clone(), cmd_rx, emit.clone())),
        tokio::spawn(nostr::run_with_root(shared.clone(), nrx, emit, root.verifying_key().to_bytes())),
    ];
    shared.nostr.send(nostr::Cmd::Active(true));
    // The command channel is dropped: `run` keeps dialing the dead relay, which is the point.
    std::mem::forget(_cmd_tx);
    Node { shared, events, _tasks: tasks }
}

fn cert(root: &SigningKey, d: &DeviceSecret, user: Id, name: &str) -> Vec<u8> {
    Cert {
        user_id: user,
        name: name.into(),
        issued_at: config::now() - 10,
        expires_at: config::now() + 86400,
        devices: vec![Device { device_id: d.device_id, sign_pub: d.sign_pub(), enc_pub: d.enc_pub() }],
    }
    .sign(root)
    .unwrap()
}

async fn wait<T>(n: &mut Node, secs: u64, mut pick: impl FnMut(&Event) -> Option<T>) -> Option<T> {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            let e = n.events.recv().await?;
            if let Some(t) = pick(&e) {
                return Some(t);
            }
        }
    })
    .await
    .ok()
    .flatten()
}

#[tokio::test]
async fn signal_over_nostr_when_the_relay_is_gone() {
    let nostr_url = mock_relay().await;
    let root = proto::generate_signing_key();
    let manifest = Manifest {
        version: 1,
        servers: vec![ServerEntry { url: "ws://127.0.0.1:9/ws".into(), ip_fallback: String::new(), server_pub: [7; 32] }],
        nostr_relays: vec![nostr_url],
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let (da, db) = (DeviceSecret::generate(), DeviceSecret::generate());
    let (ua, ub) = (Id::random(), Id::random());
    let (ca, cb) = (cert(&root, &da, ua, "a"), cert(&root, &db, ub, "b"));
    let b_dev = db.device_id;
    for d in ["a", "b"] {
        std::fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    let a = node(&root, &manifest, da, ua, &[ca.clone(), cb.clone()], &dir.path().join("a"));
    let mut b = node(&root, &manifest, db, ub, &[cb, ca], &dir.path().join("b"));
    tokio::time::sleep(Duration::from_millis(500)).await; // B subscribed

    assert!(!a.shared.nostr.online());
    let blob = super::seal_signal(&a.shared, &b_dev, br#"{"type":"offer"}"#).unwrap();
    nostr::publish_signal(&a.shared, b_dev, blob);

    let got = wait(&mut b, 5, |e| match e {
        Event::Signal { from_user, payload, .. } => Some((*from_user, payload.clone())),
        _ => None,
    })
    .await
    .expect("signal over Nostr");
    assert_eq!(got, (ua, r#"{"type":"offer"}"#.to_string()));

    // Published again at +3 s and +8 s: B must not see it twice.
    let again = wait(&mut b, 9, |e| matches!(e, Event::Signal { .. }).then_some(())).await;
    assert!(again.is_none(), "duplicate signal delivered");
}

#[tokio::test]
async fn newer_manifest_from_nostr() {
    let nostr_url = mock_relay().await;
    let root = proto::generate_signing_key();
    let root_pub = root.verifying_key().to_bytes();
    let dead = ServerEntry { url: "ws://127.0.0.1:9/ws".into(), ip_fallback: String::new(), server_pub: [7; 32] };
    let manifest = Manifest { version: 1, servers: vec![dead.clone()], nostr_relays: vec![nostr_url.clone()], ..Default::default() };
    let dir = tempfile::tempdir().unwrap();
    let d = DeviceSecret::generate();
    let u = Id::random();
    let c = cert(&root, &d, u, "a");
    let mut n = node(&root, &manifest, d, u, &[c], dir.path());

    // The admin publishes version 2 (moved server), and a forgery with version 9.
    let newer = Manifest { version: 2, servers: vec![ServerEntry { url: "ws://127.0.0.1:10/ws".into(), ..dead.clone() }], ..manifest.clone() };
    let forged = Manifest { version: 9, ..manifest.clone() }.sign(&proto::generate_signing_key()).unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(&nostr_url).await.unwrap();
    for raw in [forged, newer.sign(&root).unwrap()] {
        let ev = proto::nostr::manifest_event(&proto::nostr::random_secp_key(), &raw, &root_pub, config::now()).unwrap();
        ws.send(Message::Text(json!(["EVENT", ev.to_json()]).to_string().into())).await.unwrap();
    }

    // Offline past MANIFEST_AFTER (1 s in tests), the client queries Nostr: the forgery fails
    // the root signature, version 2 is taken.
    wait(&mut n, 10, |e| matches!(e, Event::ContactsChanged).then_some(())).await.expect("manifest from Nostr");
    assert_eq!(n.shared.manifest().version, 2);
    assert_eq!(n.shared.manifest().servers[0].url, "ws://127.0.0.1:10/ws");
}

/// A reaches our relay, B can't (blocked on its network): the relay answers "B offline", A
/// sends the signal over Nostr too, B gets it there.
#[tokio::test]
async fn relay_says_offline_so_nostr_carries_it() {
    let nostr_url = mock_relay().await;
    let root = proto::generate_signing_key();
    let server_key = proto::generate_signing_key();
    let relay = relay::Relay::new(relay::Config::new(root.verifying_key().to_bytes(), server_key.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, relay::router(relay)).await.unwrap() });
    let live = ServerEntry { url: format!("ws://{addr}/ws"), ip_fallback: String::new(), server_pub: server_key.verifying_key().to_bytes() };
    let dead = ServerEntry { url: "ws://127.0.0.1:9/ws".into(), ip_fallback: String::new(), server_pub: server_key.verifying_key().to_bytes() };
    let for_a = Manifest { version: 1, servers: vec![live], nostr_relays: vec![nostr_url.clone()], ..Default::default() };
    let for_b = Manifest { servers: vec![dead], ..for_a.clone() };

    let dir = tempfile::tempdir().unwrap();
    let (da, db) = (DeviceSecret::generate(), DeviceSecret::generate());
    let (ua, ub) = (Id::random(), Id::random());
    let (ca, cb) = (cert(&root, &da, ua, "a"), cert(&root, &db, ub, "b"));
    let b_dev = db.device_id;
    for d in ["a", "b"] {
        std::fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    // A needs a command channel to send through the relay session.
    let now = config::now();
    let mut contacts = Contacts::load(dir.path().join("a/contacts.json"), root.verifying_key().to_bytes(), now);
    contacts.merge(&[ca.clone(), cb.clone()], now, &for_a);
    let (link, nrx) = nostr::Link::new();
    let a = Arc::new(Shared {
        me: da,
        cert: ca.clone(),
        contacts: Mutex::new(contacts),
        manifest: std::sync::RwLock::new(Arc::new(for_a)),
        manifest_path: dir.path().join("a/manifest.bin"),
        reconnect: tokio::sync::Notify::new(),
        store: Mutex::new(Store::open(&dir.path().join("a/botato.db"), [9; 32]).unwrap()),
        my_user: ua,
        push_token: Mutex::new(None),
        nostr: link,
    });
    let (atx, mut aev) = mpsc::unbounded_channel();
    let emit: Emit = Arc::new(move |e| {
        let _ = atx.send(e);
    });
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    tokio::spawn(super::run(a.clone(), cmd_rx, emit.clone()));
    tokio::spawn(nostr::run_with_root(a.clone(), nrx, emit, root.verifying_key().to_bytes()));
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = aev.recv().await {
            if matches!(e, Event::Online { .. }) {
                break;
            }
        }
    })
    .await
    .unwrap();

    let mut b = node(&root, &for_b, db, ub, &[cb, ca], &dir.path().join("b"));
    tokio::time::sleep(Duration::from_millis(500)).await;

    cmd_tx.send(super::Cmd::Signal { to: b_dev, payload: br#"{"type":"offer"}"#.to_vec() }).unwrap();
    let got = wait(&mut b, 8, |e| match e {
        Event::Signal { from_user, .. } => Some(*from_user),
        _ => None,
    })
    .await
    .expect("signal copied to Nostr after 'offline'");
    assert_eq!(got, ua);
}
