//! End-to-end tests over a real WebSocket.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use proto::auth::{self, Nonce};
use proto::cert::{Cert, Device};
use proto::frames::{ClientFrame, ErrorCode, Platform, ServerFrame, close};
use proto::manifest::{Manifest, ServerEntry};
use proto::{Id, SigningKey};
use relay::push::{PushKind, PushToken, Pusher};
use relay::turn::TurnConfig;
use relay::{Config, Relay};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const T0: u64 = 1_800_000_000;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Default)]
struct RecPusher(Mutex<Vec<(String, PushKind)>>);

impl Pusher for RecPusher {
    fn push(&self, t: &PushToken, k: PushKind) {
        self.0.lock().unwrap().push((t.token.clone(), k));
    }
}

struct Env {
    root: SigningKey,
    relay: Arc<Relay>,
    addr: String,
    clock: Arc<AtomicU64>,
    pushes: Arc<RecPusher>,
}

struct Dev {
    user: Id,
    device: Id,
    key: SigningKey,
    cert: Vec<u8>,
}

impl Env {
    async fn new() -> Env {
        Self::with(|_| {}).await
    }

    async fn with(tweak: impl FnOnce(&mut Config)) -> Env {
        let root = proto::generate_signing_key();
        let clock = Arc::new(AtomicU64::new(T0));
        let pushes = Arc::new(RecPusher::default());
        let mut cfg = Config::new(
            root.verifying_key().to_bytes(),
            proto::generate_signing_key(),
        );
        let c = clock.clone();
        cfg.clock = Arc::new(move || c.load(Ordering::SeqCst));
        cfg.pusher = pushes.clone();
        cfg.auth_timeout = Duration::from_millis(500);
        cfg.turn = Some(TurnConfig {
            secret: b"s3cret".to_vec(),
            urls: vec!["turn:t.example:3478".into()],
            ttl_secs: 7200,
        });
        tweak(&mut cfg);
        let relay = Relay::new(cfg);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let app = relay::router(relay.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Env {
            root,
            relay,
            addr,
            clock,
            pushes,
        }
    }

    /// A user with `n` devices, all in one cert.
    fn user(&self, name: &str, n: usize, issued_at: u64) -> Vec<Dev> {
        let user = Id::random();
        let keys: Vec<(Id, SigningKey)> = (0..n)
            .map(|_| (Id::random(), proto::generate_signing_key()))
            .collect();
        self.user_with(user, name, &keys, issued_at)
    }

    fn user_with(
        &self,
        user: Id,
        name: &str,
        keys: &[(Id, SigningKey)],
        issued_at: u64,
    ) -> Vec<Dev> {
        let cert = Cert {
            user_id: user,
            name: name.into(),
            issued_at,
            expires_at: T0 + 365 * 86400,
            devices: keys
                .iter()
                .map(|(d, k)| Device {
                    device_id: *d,
                    sign_pub: k.verifying_key().to_bytes(),
                    enc_pub: [7; 32],
                })
                .collect(),
        }
        .sign(&self.root)
        .unwrap();
        keys.iter()
            .map(|(d, k)| Dev {
                user,
                device: *d,
                key: k.clone(),
                cert: cert.clone(),
            })
            .collect()
    }

    async fn raw(&self) -> Ws {
        tokio_tungstenite::connect_async(format!("ws://{}/ws", self.addr))
            .await
            .unwrap()
            .0
    }

    /// Full handshake, including verifying the server's proof.
    async fn connect(&self, d: &Dev) -> Ws {
        let mut ws = self.raw().await;
        let cn: Nonce = proto::random_bytes();
        send(
            &mut ws,
            &ClientFrame::Hello {
                v: 1,
                nonce: cn.to_vec(),
            },
        )
        .await;
        let ServerFrame::Challenge {
            nonce,
            server_pub,
            sig,
        } = recv(&mut ws).await
        else {
            panic!("no challenge")
        };
        assert_eq!(server_pub, self.relay.server_pub());
        let sn: Nonce = nonce.try_into().unwrap();
        auth::verify_server(&server_pub, &cn, &sn, &sig).expect("server proof");
        let csig = auth::sign_client(&d.key, &sn, &self.relay.server_pub(), &cn);
        send(
            &mut ws,
            &ClientFrame::Auth {
                cert: d.cert.clone(),
                device_id: d.device,
                sig: csig.to_vec(),
            },
        )
        .await;
        assert_eq!(
            recv(&mut ws).await,
            ServerFrame::Ready {
                user_id: d.user,
                device_id: d.device
            }
        );
        ws
    }

    fn manifest(&self, version: u64, revoked: Vec<Id>) -> Vec<u8> {
        Manifest {
            version,
            issued_at: T0,
            servers: vec![ServerEntry {
                url: "wss://x/ws".into(),
                ip_fallback: "".into(),
                server_pub: self.relay.server_pub(),
            }],
            revoked,
            ..Default::default()
        }
        .sign(&self.root)
        .unwrap()
    }
}

async fn send(ws: &mut Ws, f: &ClientFrame) {
    ws.send(Message::Text(serde_json::to_string(f).unwrap().into()))
        .await
        .unwrap();
}

async fn recv(ws: &mut Ws) -> ServerFrame {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
            .expect("eof")
            .expect("ws error");
        match m {
            Message::Text(t) => return serde_json::from_str(t.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

async fn expect_close(ws: &mut Ws, code: u16) {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
        {
            Some(Ok(Message::Close(Some(f)))) => {
                assert_eq!(f.code, CloseCode::from(code));
                return;
            }
            Some(Ok(Message::Text(t))) => panic!("expected close {code}, got {t}"),
            Some(Ok(_)) => continue,
            other => panic!("expected close {code}, got {other:?}"),
        }
    }
}

async fn silent(ws: &mut Ws) {
    if let Ok(Some(Ok(Message::Text(t)))) =
        tokio::time::timeout(Duration::from_millis(200), ws.next()).await
    {
        panic!("unexpected frame {t}");
    }
}

#[tokio::test]
async fn send_deliver_ack_online() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    let mut wb = env.connect(&b).await;

    let id = Id::random();
    send(
        &mut wa,
        &ClientFrame::Send {
            to: b.device,
            id,
            blob: b"hi".to_vec(),
        },
    )
    .await;
    assert_eq!(recv(&mut wa).await, ServerFrame::Stored { id });
    assert_eq!(
        recv(&mut wb).await,
        ServerFrame::Deliver {
            id,
            from: a.device,
            blob: b"hi".to_vec()
        }
    );
    send(&mut wb, &ClientFrame::Ack { id }).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(env.relay.mailbox().len(), 0);
}

#[tokio::test]
async fn offline_delivery_on_reconnect_and_redelivery_until_ack() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    let ids: Vec<Id> = (0..40).map(|_| Id::random()).collect(); // more than one pump batch
    for id in &ids {
        send(
            &mut wa,
            &ClientFrame::Send {
                to: b.device,
                id: *id,
                blob: vec![1],
            },
        )
        .await;
        assert_eq!(recv(&mut wa).await, ServerFrame::Stored { id: *id });
    }
    // Resend is idempotent.
    send(
        &mut wa,
        &ClientFrame::Send {
            to: b.device,
            id: ids[0],
            blob: vec![1],
        },
    )
    .await;
    assert_eq!(recv(&mut wa).await, ServerFrame::Stored { id: ids[0] });

    let mut wb = env.connect(&b).await;
    for id in &ids {
        let ServerFrame::Deliver { id: got, .. } = recv(&mut wb).await else {
            panic!()
        };
        assert_eq!(got, *id);
    }
    send(&mut wb, &ClientFrame::Ack { id: ids[0] }).await;
    drop(wb);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Unacked messages come again on the next connection.
    let mut wb = env.connect(&b).await;
    let ServerFrame::Deliver { id: got, .. } = recv(&mut wb).await else {
        panic!()
    };
    assert_eq!(got, ids[1]);
}

#[tokio::test]
async fn messages_expire_after_ttl() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    let id = Id::random();
    send(
        &mut wa,
        &ClientFrame::Send {
            to: b.device,
            id,
            blob: vec![1],
        },
    )
    .await;
    recv(&mut wa).await;
    env.clock.fetch_add(24 * 3600, Ordering::SeqCst);
    env.relay.sweep();
    assert_eq!(env.relay.mailbox().len(), 0);
    let mut wb = env.connect(&b).await;
    silent(&mut wb).await;
}

#[tokio::test]
async fn auth_rejects_forgeries() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);

    // Cert signed by another root.
    let other = Env::new().await;
    let foreign = other.user("x", 1, T0).remove(0);
    let mut ws = env.raw().await;
    handshake_expect(&env, &mut ws, &foreign, &foreign.key, close::AUTH_FAILED).await;

    // Valid cert, wrong device key.
    let mut ws = env.raw().await;
    handshake_expect(
        &env,
        &mut ws,
        &a,
        &proto::generate_signing_key(),
        close::AUTH_FAILED,
    )
    .await;

    // Expired cert.
    env.clock.store(T0 + 366 * 86400, Ordering::SeqCst);
    let mut ws = env.raw().await;
    handshake_expect(&env, &mut ws, &a, &a.key, close::AUTH_FAILED).await;
    env.clock.store(T0, Ordering::SeqCst);

    // Garbage instead of hello.
    let mut ws = env.raw().await;
    ws.send(Message::Text("{}".into())).await.unwrap();
    expect_close(&mut ws, close::PROTOCOL).await;

    // Silence.
    let mut ws = env.raw().await;
    expect_close(&mut ws, close::AUTH_TIMEOUT).await;
}

async fn handshake_expect(env: &Env, ws: &mut Ws, d: &Dev, key: &SigningKey, code: u16) {
    let cn: Nonce = proto::random_bytes();
    send(
        ws,
        &ClientFrame::Hello {
            v: 1,
            nonce: cn.to_vec(),
        },
    )
    .await;
    let ServerFrame::Challenge { nonce, .. } = recv(ws).await else {
        panic!()
    };
    let sn: Nonce = nonce.try_into().unwrap();
    let sig = auth::sign_client(key, &sn, &env.relay.server_pub(), &cn);
    send(
        ws,
        &ClientFrame::Auth {
            cert: d.cert.clone(),
            device_id: d.device,
            sig: sig.to_vec(),
        },
    )
    .await;
    expect_close(ws, code).await;
}

#[tokio::test]
async fn same_device_is_replaced_other_devices_coexist() {
    let env = Env::new().await;
    let devs = env.user("a", 2, T0);
    let mut first = env.connect(&devs[0]).await;
    let mut other = env.connect(&devs[1]).await;
    let _second = env.connect(&devs[0]).await;
    expect_close(&mut first, close::REPLACED).await;
    send(&mut other, &ClientFrame::Directory {}).await;
    assert!(matches!(
        recv(&mut other).await,
        ServerFrame::Directory { .. }
    ));
}

#[tokio::test]
async fn signals_live_and_held() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;

    // b offline, with a push token registered earlier.
    {
        let mut wb = env.connect(&b).await;
        send(
            &mut wb,
            &ClientFrame::PushToken {
                platform: Platform::Fcm,
                token: "tok-b".into(),
            },
        )
        .await;
        assert_eq!(
            recv(&mut wb).await,
            ServerFrame::Ok {
                r#ref: "push_token".into()
            }
        );
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    send(
        &mut wa,
        &ClientFrame::Signal {
            to: b.device,
            blob: b"offer".to_vec(),
        },
    )
    .await;
    assert_eq!(
        recv(&mut wa).await,
        ServerFrame::Signaled {
            to: b.device,
            online: false
        }
    );
    send(
        &mut wa,
        &ClientFrame::Signal {
            to: b.device,
            blob: b"ice".to_vec(),
        },
    )
    .await;
    recv(&mut wa).await;
    // Call pushes are debounced.
    assert_eq!(
        *env.pushes.0.lock().unwrap(),
        vec![("tok-b".to_string(), PushKind::Call)]
    );

    let mut wb = env.connect(&b).await;
    assert_eq!(
        recv(&mut wb).await,
        ServerFrame::Signal {
            from: a.device,
            blob: b"offer".to_vec()
        }
    );
    assert_eq!(
        recv(&mut wb).await,
        ServerFrame::Signal {
            from: a.device,
            blob: b"ice".to_vec()
        }
    );

    send(
        &mut wb,
        &ClientFrame::Signal {
            to: a.device,
            blob: b"answer".to_vec(),
        },
    )
    .await;
    assert_eq!(
        recv(&mut wb).await,
        ServerFrame::Signaled {
            to: a.device,
            online: true
        }
    );
    assert_eq!(
        recv(&mut wa).await,
        ServerFrame::Signal {
            from: b.device,
            blob: b"answer".to_vec()
        }
    );
}

#[tokio::test]
async fn held_signals_expire() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    send(
        &mut wa,
        &ClientFrame::Signal {
            to: b.device,
            blob: b"offer".to_vec(),
        },
    )
    .await;
    recv(&mut wa).await;
    env.clock.fetch_add(61, Ordering::SeqCst);
    let mut wb = env.connect(&b).await;
    silent(&mut wb).await;
}

#[tokio::test]
async fn message_push_for_offline_device() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    {
        let mut wb = env.connect(&b).await;
        send(
            &mut wb,
            &ClientFrame::PushToken {
                platform: Platform::ApnsVoip,
                token: "tok".into(),
            },
        )
        .await;
        recv(&mut wb).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut wa = env.connect(&a).await;
    for _ in 0..3 {
        send(
            &mut wa,
            &ClientFrame::Send {
                to: b.device,
                id: Id::random(),
                blob: vec![1],
            },
        )
        .await;
        recv(&mut wa).await;
    }
    assert_eq!(
        *env.pushes.0.lock().unwrap(),
        vec![("tok".to_string(), PushKind::Message)]
    );
}

#[tokio::test]
async fn revocation_via_manifest() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 2, T0);
    let mut wa = env.connect(&a).await;
    let mut wb0 = env.connect(&b[0]).await;
    let mut wb1 = env.connect(&b[1]).await;

    send(
        &mut wa,
        &ClientFrame::Send {
            to: b[1].device,
            id: Id::random(),
            blob: vec![1],
        },
    )
    .await;
    recv(&mut wa).await;
    recv(&mut wb1).await; // delivered, not acked

    // Revoke one device of b.
    env.relay
        .set_manifest(&env.manifest(1, vec![b[1].device]))
        .unwrap();
    expect_close(&mut wb1, close::REVOKED).await;
    assert_eq!(env.relay.mailbox().len(), 0);
    send(
        &mut wa,
        &ClientFrame::Send {
            to: b[1].device,
            id: Id::random(),
            blob: vec![1],
        },
    )
    .await;
    assert!(matches!(
        recv(&mut wa).await,
        ServerFrame::Error {
            code: ErrorCode::Revoked,
            ..
        }
    ));
    let mut ws = env.raw().await;
    handshake_expect(&env, &mut ws, &b[1], &b[1].key, close::REVOKED).await;

    // Stale manifest is rejected; revoking the whole user kicks the rest.
    assert!(env.relay.set_manifest(&env.manifest(1, vec![])).is_err());
    env.relay
        .set_manifest(&env.manifest(2, vec![b[1].device, b[0].user]))
        .unwrap();
    expect_close(&mut wb0, close::REVOKED).await;
    send(
        &mut wa,
        &ClientFrame::Send {
            to: b[0].device,
            id: Id::random(),
            blob: vec![1],
        },
    )
    .await;
    assert!(matches!(
        recv(&mut wa).await,
        ServerFrame::Error {
            code: ErrorCode::Revoked,
            ..
        }
    ));

    let body = reqwest_like_get(&env.addr, "/manifest").await;
    assert_eq!(body, env.manifest(2, vec![b[1].device, b[0].user]));
}

#[tokio::test]
async fn newer_cert_supersedes_removed_device() {
    let env = Env::new().await;
    let user = Id::random();
    let k0 = (Id::random(), proto::generate_signing_key());
    let k1 = (Id::random(), proto::generate_signing_key());
    let old = env.user_with(user, "u", &[k0.clone(), k1.clone()], T0);
    let new = env.user_with(user, "u", std::slice::from_ref(&k0), T0 + 10);

    let mut w = env.connect(&new[0]).await;
    send(&mut w, &ClientFrame::Directory {}).await;
    assert_eq!(
        recv(&mut w).await,
        ServerFrame::Directory {
            certs: vec![new[0].cert.clone()]
        }
    );

    // The removed device still holds the old (unexpired) cert, but the relay knows better.
    let mut ws = env.raw().await;
    handshake_expect(&env, &mut ws, &old[1], &old[1].key, close::AUTH_FAILED).await;
    // The kept device can still use the old cert (replacing its own session); the directory keeps the newer one.
    let mut w0 = env.connect(&old[0]).await;
    expect_close(&mut w, close::REPLACED).await;
    send(&mut w0, &ClientFrame::Directory {}).await;
    assert_eq!(
        recv(&mut w0).await,
        ServerFrame::Directory {
            certs: vec![new[0].cert.clone()]
        }
    );
}

#[tokio::test]
async fn limits_and_bad_frames() {
    let env = Env::with(|c| {
        c.rate_burst = 5.0;
        c.rate_per_sec = 0.001;
    })
    .await;
    let a = env.user("a", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;

    ws_text(&mut wa, "not json").await;
    assert!(matches!(
        recv(&mut wa).await,
        ServerFrame::Error {
            code: ErrorCode::BadFrame,
            ..
        }
    ));

    let id = Id::random();
    send(
        &mut wa,
        &ClientFrame::Send {
            to: a.device,
            id,
            blob: vec![0; proto::frames::MAX_BLOB + 1],
        },
    )
    .await;
    assert_eq!(
        recv(&mut wa).await,
        ServerFrame::Error {
            r#ref: "send".into(),
            id: Some(id),
            code: ErrorCode::TooLarge
        }
    );

    send(&mut wa, &ClientFrame::Turn {}).await;
    let ServerFrame::Turn {
        username,
        ttl,
        urls,
        ..
    } = recv(&mut wa).await
    else {
        panic!()
    };
    assert_eq!(username, (T0 + 7200).to_string());
    assert_eq!((ttl, urls.len()), (7200, 1));

    for _ in 0..3 {
        send(&mut wa, &ClientFrame::Directory {}).await;
        recv(&mut wa).await;
    }
    send(&mut wa, &ClientFrame::Directory {}).await;
    assert_eq!(
        recv(&mut wa).await,
        ServerFrame::Error {
            r#ref: "directory".into(),
            id: None,
            code: ErrorCode::RateLimited
        }
    );
}

#[tokio::test]
async fn sender_quota() {
    let env = Env::with(|c| c.limits.per_sender_count = 2).await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    for i in 0..3 {
        let id = Id::random();
        send(
            &mut wa,
            &ClientFrame::Send {
                to: b.device,
                id,
                blob: vec![1],
            },
        )
        .await;
        let want = if i < 2 {
            ServerFrame::Stored { id }
        } else {
            ServerFrame::Error {
                r#ref: "send".into(),
                id: Some(id),
                code: ErrorCode::Quota,
            }
        };
        assert_eq!(recv(&mut wa).await, want);
    }
}

#[tokio::test]
async fn shutdown_closes_sessions() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    env.relay.shutdown();
    expect_close(&mut wa, 1001).await;
}

#[tokio::test]
async fn http_endpoints() {
    let env = Env::new().await;
    assert!(
        reqwest_like_status(&env.addr, "/manifest")
            .await
            .starts_with("HTTP/1.1 404")
    );
    assert!(
        reqwest_like_status(&env.addr, "/healthz")
            .await
            .starts_with("HTTP/1.1 204")
    );
}

async fn ws_text(ws: &mut Ws, s: &str) {
    ws.send(Message::Text(s.into())).await.unwrap();
}

async fn http_get(addr: &str, path: &str) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = vec![];
    s.read_to_end(&mut buf).await.unwrap();
    buf
}

async fn reqwest_like_status(addr: &str, path: &str) -> String {
    String::from_utf8_lossy(&http_get(addr, path).await)
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

async fn reqwest_like_get(addr: &str, path: &str) -> Vec<u8> {
    let raw = http_get(addr, path).await;
    let pos = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    raw[pos + 4..].to_vec()
}

#[tokio::test]
async fn attachments_upload_download_expire() {
    let dir = tempfile::tempdir().unwrap();
    let files = Arc::new(relay::files::Files::open(dir.path(), Default::default()).unwrap());
    let env = Env::with(|c| c.files = Some(files)).await;
    let a = env.user("a", 1, T0).remove(0);
    let b = env.user("b", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    let mut wb = env.connect(&b).await;

    // Bigger than a message blob: goes through the raised read limit.
    let data = vec![0x5a; 300 * 1024];
    let id = Id::random();
    send(&mut wa, &ClientFrame::Upload { id, data: data.clone() }).await;
    assert_eq!(recv(&mut wa).await, ServerFrame::Uploaded { id });
    send(&mut wb, &ClientFrame::Download { id }).await;
    assert_eq!(recv(&mut wb).await, ServerFrame::File { id, data });

    let missing = Id::random();
    send(&mut wb, &ClientFrame::Download { id: missing }).await;
    assert_eq!(
        recv(&mut wb).await,
        ServerFrame::Error { r#ref: "download".into(), id: Some(missing), code: ErrorCode::NotFound }
    );
    let too_big = vec![0; proto::file::MAX_FILE + 1];
    send(&mut wa, &ClientFrame::Upload { id: Id::random(), data: too_big }).await;
    assert!(matches!(recv(&mut wa).await, ServerFrame::Error { code: ErrorCode::TooLarge, .. }));

    env.clock.fetch_add(24 * 3600, Ordering::SeqCst);
    env.relay.sweep();
    send(&mut wb, &ClientFrame::Download { id }).await;
    assert!(matches!(recv(&mut wb).await, ServerFrame::Error { code: ErrorCode::NotFound, .. }));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn attachments_unavailable_without_store() {
    let env = Env::new().await;
    let a = env.user("a", 1, T0).remove(0);
    let mut wa = env.connect(&a).await;
    send(&mut wa, &ClientFrame::Download { id: Id::random() }).await;
    assert!(matches!(recv(&mut wa).await, ServerFrame::Error { code: ErrorCode::Unavailable, .. }));
}
