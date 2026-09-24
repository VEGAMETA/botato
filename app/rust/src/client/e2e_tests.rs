//! End to end: real relay in-process, real clients (`run`), real store.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use proto::cert::{Cert, Device};
use proto::device::DeviceSecret;
use proto::manifest::{Manifest, ServerEntry};
use proto::{Id, SigningKey};
use tokio::sync::mpsc;

use super::contacts::Contacts;
use super::store::{self, Store};
use super::{Cmd, Emit, Event, Shared, messaging, run};
use crate::config;

struct Net {
    root: SigningKey,
    manifest: Manifest,
    dir: tempfile::TempDir,
}

struct Client {
    shared: Arc<Shared>,
    tx: mpsc::UnboundedSender<Cmd>,
    events: mpsc::UnboundedReceiver<Event>,
    task: tokio::task::JoinHandle<()>,
}

async fn net() -> Net {
    let root = proto::generate_signing_key();
    let server_key = proto::generate_signing_key();
    let relay = relay::Relay::new(relay::Config::new(root.verifying_key().to_bytes(), server_key.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, relay::router(relay)).await.unwrap() });
    let manifest = Manifest {
        version: 1,
        issued_at: 0,
        servers: vec![ServerEntry {
            url: format!("ws://{addr}/ws"),
            ip_fallback: String::new(),
            server_pub: server_key.verifying_key().to_bytes(),
        }],
        ..Default::default()
    };
    Net { root, manifest, dir: tempfile::tempdir().unwrap() }
}

/// A user with `n` devices; returns their secrets and the signed cert.
fn user(net: &Net, name: &str, n: usize) -> (Id, Vec<DeviceSecret>, Vec<u8>) {
    let devices: Vec<DeviceSecret> = (0..n).map(|_| DeviceSecret::generate()).collect();
    let user_id = Id::random();
    let cert = Cert {
        user_id,
        name: name.into(),
        issued_at: config::now() - 10,
        expires_at: config::now() + 86400,
        devices: devices
            .iter()
            .map(|d| Device { device_id: d.device_id, sign_pub: d.sign_pub(), enc_pub: d.enc_pub() })
            .collect(),
    }
    .sign(&net.root)
    .unwrap();
    (user_id, devices, cert)
}

fn start(net: &Net, me: DeviceSecret, user: Id, cert: &[u8]) -> Client {
    let dir = net.dir.path().join(me.device_id.to_hex());
    std::fs::create_dir_all(&dir).unwrap();
    start_in(net, me, user, cert, &dir)
}

fn start_in(net: &Net, me: DeviceSecret, user: Id, cert: &[u8], dir: &std::path::Path) -> Client {
    let now = config::now();
    let mut contacts = Contacts::load(dir.join("contacts.json"), net.root.verifying_key().to_bytes(), now);
    contacts.merge(&[cert.to_vec()], now, &net.manifest);
    let shared = Arc::new(Shared {
        me,
        cert: cert.to_vec(),
        contacts: Mutex::new(contacts),
        manifest: net.manifest.clone(),
        reconnect: tokio::sync::Notify::new(),
        store: Mutex::new(Store::open(&dir.join("botato.db"), [9; 32]).unwrap()),
        my_user: user,
        push_token: Mutex::new(None),
    });
    let (tx, rx) = mpsc::unbounded_channel();
    let (etx, events) = mpsc::unbounded_channel();
    let emit: Emit = Arc::new(move |e| {
        let _ = etx.send(e);
    });
    let task = tokio::spawn(run(shared.clone(), rx, emit));
    Client { shared, tx, events, task }
}

impl Client {
    async fn wait(&mut self, mut pred: impl FnMut(&Event) -> bool) -> Event {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let e = self.events.recv().await.expect("client stopped");
                if pred(&e) {
                    return e;
                }
            }
        })
        .await
        .expect("event did not arrive")
    }

    async fn online(&mut self) {
        self.wait(|e| matches!(e, Event::Online { .. })).await;
    }

    fn send(&self, peer: &Id, text: &str) -> Id {
        let id = messaging::compose(&self.shared, peer, text).unwrap();
        self.tx.send(Cmd::Flush).unwrap();
        id
    }

    fn history(&self, peer: &Id) -> Vec<(bool, String, i64)> {
        let s = self.shared.store.lock().unwrap();
        s.messages(peer, None, 100).unwrap().into_iter().map(|m| (m.from_me, m.text, m.status)).collect()
    }

    async fn wait_status(&mut self, peer: &Id, status: i64) {
        for _ in 0..100 {
            if self.history(peer).last().is_some_and(|m| m.2 == status) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("status {status} not reached: {:?}", self.history(peer));
    }
}

#[tokio::test]
async fn messages_receipts_multidevice_and_offline() {
    let net = net().await;
    let (alice, mut a_devs, a_cert) = user(&net, "alice", 1);
    let (bob, mut b_devs, b_cert) = user(&net, "bob", 2);

    // Bob's devices connect first, so they don't know Alice yet (exercises the
    // unknown-sender path: delivery held, directory fetched, then processed).
    let mut bob1 = start(&net, b_devs.remove(0), bob, &b_cert);
    let mut bob2 = start(&net, b_devs.remove(0), bob, &b_cert);
    bob1.online().await;
    bob2.online().await;
    let mut a = start(&net, a_devs.remove(0), alice, &a_cert);
    a.online().await;
    a.wait(|e| matches!(e, Event::ContactsChanged)).await;

    // Alice -> Bob: both of Bob's devices get it, Alice sees "delivered".
    a.send(&bob, "привет, боб");
    for b in [&mut bob1, &mut bob2] {
        let e = b.wait(|e| matches!(e, Event::NewMessage { .. })).await;
        let Event::NewMessage { peer, text } = e else { unreachable!() };
        assert_eq!((peer, text.as_str()), (alice, "привет, боб"));
    }
    a.wait_status(&bob, store::DELIVERED).await;
    assert!(a.shared.store.lock().unwrap().outbox().unwrap().is_empty(), "all copies acknowledged");

    // Bob (device 1) -> Alice: Bob's device 2 files it as his own outgoing message.
    b_reply(&bob1, &alice).await;
    a.wait(|e| matches!(e, Event::NewMessage { text, .. } if text == "и тебе")).await;
    bob2.wait(|e| matches!(e, Event::MessagesChanged { .. })).await;
    assert_eq!(bob2.history(&alice).last().map(|m| (m.0, m.1.clone())), Some((true, "и тебе".to_string())));

    // Alice goes offline; Bob writes; Alice comes back (same storage) and gets it.
    let a_dir = net.dir.path().join(a.shared.me.device_id.to_hex());
    let a_secret = DeviceSecret::from_bytes(&a.shared.me.to_bytes()).unwrap();
    a.task.abort();
    drop(a);
    tokio::time::sleep(Duration::from_millis(200)).await;
    bob1.send(&alice, "ты тут?");
    bob1.wait_status(&alice, store::SENT).await;
    let mut a = start_in(&net, a_secret, alice, &a_cert, &a_dir);
    a.wait(|e| matches!(e, Event::NewMessage { text, .. } if text == "ты тут?")).await;
    bob1.wait_status(&alice, store::DELIVERED).await;
    // Earlier history survived the restart and there are no duplicates.
    let h = a.history(&bob);
    assert_eq!(h.iter().map(|m| m.1.as_str()).collect::<Vec<_>>(), ["привет, боб", "и тебе", "ты тут?"]);
}

async fn b_reply(bob1: &Client, alice: &Id) {
    bob1.send(alice, "и тебе");
}
