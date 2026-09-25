//! Load generator: N synthetic devices with test certificates exchange messages, call
//! signals and attachments through a relay; prints latency percentiles and errors.
//!
//! Never point it at a production relay: its certificates are signed by a throwaway root,
//! a real relay rejects them (and must not learn fake users anyway).
//!
//!   # relay in this process (quick, but shares the CPU with the generator)
//!   cargo run --release -p relay --example load -- --devices 200 --secs 30
//!
//!   # separate relay: print a throwaway root, start the relay with its public half
//!   cargo run --release -p relay --example load -- --new-root
//!   cargo run --release -p relay -- serve --listen 127.0.0.1:8080 --key target/load.key \
//!       --root-pub <pub> --files-dir target/load-files
//!   cargo run --release -p relay --example load -- --url ws://127.0.0.1:8080/ws --root-seed <seed>

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use proto::auth::{self, Nonce};
use proto::cert::{Cert, Device};
use proto::frames::{ClientFrame, ServerFrame};
use proto::{Id, SigningKey};
use tokio::sync::{Semaphore, mpsc};
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser)]
struct Args {
    /// Relay URL; without it a relay runs in this process.
    #[arg(long)]
    url: Option<String>,
    /// Hex seed of the throwaway root that signs the test certs (with --url).
    #[arg(long)]
    root_seed: Option<String>,
    /// Print a new throwaway root (seed and public key) and exit.
    #[arg(long)]
    new_root: bool,
    #[arg(long, default_value_t = 100)]
    devices: usize,
    #[arg(long, default_value_t = 30)]
    secs: u64,
    /// Messages per second per device.
    #[arg(long, default_value_t = 0.5)]
    msg_rate: f64,
    /// Message blob size, bytes (≤ 65536).
    #[arg(long, default_value_t = 1500)]
    msg_size: usize,
    /// Call signals per second per device.
    #[arg(long, default_value_t = 1.0)]
    signal_rate: f64,
    /// Attachments per minute per device (0 = none). Each is uploaded, then downloaded.
    #[arg(long, default_value_t = 1.0)]
    photos_per_min: f64,
    #[arg(long, default_value_t = 400)]
    photo_kb: usize,
    /// Simultaneous handshakes (the relay allows 16 by default).
    #[arg(long, default_value_t = 12)]
    connect_parallel: usize,
}

#[derive(Default)]
struct Stats {
    lat: HashMap<&'static str, Vec<u64>>, // microseconds
    errors: HashMap<String, u64>,
}

impl Stats {
    fn lat(&mut self, kind: &'static str, us: u64) {
        self.lat.entry(kind).or_default().push(us);
    }
    fn err(&mut self, e: impl Into<String>) {
        *self.errors.entry(e.into()).or_default() += 1;
    }
}

struct Shared {
    start: Instant,
    stats: Mutex<Stats>,
    devices: Vec<Id>,
    stop: AtomicBool,
    sent: AtomicU64,
}

impl Shared {
    fn now_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

fn stamp(now_us: u64, size: usize) -> Vec<u8> {
    let mut b = vec![0x42u8; size.max(8)];
    b[..8].copy_from_slice(&now_us.to_be_bytes());
    b
}

fn stamped(b: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(..8)?.try_into().ok()?))
}

fn pick_other(devices: &[Id], me: &Id) -> Id {
    loop {
        let i = u64::from_be_bytes(proto::random_bytes()) as usize % devices.len();
        if devices[i] != *me {
            return devices[i];
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn recv(ws: &mut Ws) -> anyhow_like::Result<ServerFrame> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(t))) => return Ok(serde_json::from_str(t.as_str())?),
            Some(Ok(Message::Close(c))) => return Err(format!("closed {c:?}").into()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.to_string().into()),
            None => return Err("eof".into()),
        }
    }
}

async fn send(ws: &mut Ws, f: &ClientFrame) -> anyhow_like::Result<()> {
    ws.send(Message::Text(serde_json::to_string(f)?.into())).await.map_err(|e| e.to_string().into())
}

async fn connect(url: &str, device: Id, key: &SigningKey, cert: &[u8]) -> anyhow_like::Result<Ws> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.map_err(|e| e.to_string())?;
    let cn: Nonce = proto::random_bytes();
    send(&mut ws, &ClientFrame::Hello { v: 1, nonce: cn.to_vec() }).await?;
    let ServerFrame::Challenge { nonce, server_pub, .. } = recv(&mut ws).await? else {
        return Err("no challenge".into());
    };
    let sn: Nonce = nonce.try_into().map_err(|_| "nonce")?;
    let server_pub: [u8; 32] = server_pub.try_into().map_err(|_| "server_pub")?;
    let sig = auth::sign_client(key, &sn, &server_pub, &cn);
    send(&mut ws, &ClientFrame::Auth { cert: cert.to_vec(), device_id: device, sig: sig.to_vec() }).await?;
    match recv(&mut ws).await? {
        ServerFrame::Ready { .. } => Ok(ws),
        other => Err(format!("auth: {other:?}").into()),
    }
}

/// Runs one device until `stop`.
async fn device(sh: Arc<Shared>, args: Arc<Args>, url: String, me: Id, key: SigningKey, cert: Vec<u8>, gate: Arc<Semaphore>) {
    let t0 = Instant::now();
    let ws = {
        let _permit = gate.acquire().await.unwrap();
        let mut tries = 0;
        loop {
            match connect(&url, me, &key, &cert).await {
                Ok(ws) => break ws,
                Err(e) if tries < 20 => {
                    tries += 1;
                    sh.stats.lock().unwrap().err(format!("connect retry: {e}"));
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(e) => {
                    sh.stats.lock().unwrap().err(format!("connect failed: {e}"));
                    return;
                }
            }
        }
    };
    sh.stats.lock().unwrap().lat("handshake", t0.elapsed().as_micros() as u64);

    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::channel::<ClientFrame>(256);
    let writer = tokio::spawn(async move {
        while let Some(f) = rx.recv().await {
            if sink.send(Message::Text(serde_json::to_string(&f).unwrap().into())).await.is_err() {
                break;
            }
        }
    });

    // Attachments in flight: id -> (uploaded at, downloaded at).
    let pending: Arc<Mutex<HashMap<Id, u64>>> = Arc::default();
    let reader = {
        let (sh, tx, pending, photo_bytes) = (sh.clone(), tx.clone(), pending.clone(), args.photo_kb * 1024);
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Ok(Message::Text(t)) = msg else {
                    if msg.is_err() {
                        break;
                    }
                    continue;
                };
                let Ok(f) = serde_json::from_str::<ServerFrame>(t.as_str()) else { continue };
                let now = sh.now_us();
                match f {
                    ServerFrame::Deliver { id, blob, .. } => {
                        if let Some(at) = stamped(&blob) {
                            sh.stats.lock().unwrap().lat("message", now.saturating_sub(at));
                        }
                        let _ = tx.send(ClientFrame::Ack { id }).await;
                    }
                    ServerFrame::Signal { blob, .. } => {
                        if let Some(at) = stamped(&blob) {
                            sh.stats.lock().unwrap().lat("signal", now.saturating_sub(at));
                        }
                    }
                    ServerFrame::Uploaded { id } => {
                        let at = pending.lock().unwrap().insert(id, now);
                        if let Some(at) = at {
                            sh.stats.lock().unwrap().lat("upload", now.saturating_sub(at));
                        }
                        let _ = tx.send(ClientFrame::Download { id }).await;
                    }
                    ServerFrame::File { id, data } => {
                        let at = pending.lock().unwrap().remove(&id);
                        let mut st = sh.stats.lock().unwrap();
                        if let Some(at) = at {
                            st.lat("download", now.saturating_sub(at));
                        }
                        if data.len() != photo_bytes {
                            st.err("download: wrong size");
                        }
                    }
                    ServerFrame::Error { r#ref, code, .. } => {
                        sh.stats.lock().unwrap().err(format!("{ref}: {code:?}"));
                    }
                    _ => {}
                }
            }
        })
    };

    let period = |rate: f64| (rate > 0.0).then(|| Duration::from_secs_f64(1.0 / rate));
    let jitter = |p: Duration| p.mul_f64((u64::from_be_bytes(proto::random_bytes()) % 1000) as f64 / 1000.0);
    let mk = |p: Option<Duration>| {
        p.map(|p| {
            let mut i = tokio::time::interval_at(tokio::time::Instant::now() + jitter(p), p);
            i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            i
        })
    };
    let mut msg = mk(period(args.msg_rate));
    let mut sig = mk(period(args.signal_rate));
    let mut photo = mk(period(args.photos_per_min / 60.0));
    let photo_data = vec![0x5au8; args.photo_kb * 1024];
    async fn tick(i: &mut Option<tokio::time::Interval>) {
        match i {
            Some(i) => {
                i.tick().await;
            }
            None => std::future::pending().await,
        }
    }
    while !sh.stop.load(Ordering::Relaxed) {
        let f = tokio::select! {
            _ = tick(&mut msg) => ClientFrame::Send {
                to: pick_other(&sh.devices, &me),
                id: Id::random(),
                blob: stamp(sh.now_us(), args.msg_size),
            },
            _ = tick(&mut sig) => ClientFrame::Signal { to: pick_other(&sh.devices, &me), blob: stamp(sh.now_us(), 400) },
            _ = tick(&mut photo) => {
                let id = Id::random();
                pending.lock().unwrap().insert(id, sh.now_us());
                ClientFrame::Upload { id, data: photo_data.clone() }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => continue,
        };
        sh.sent.fetch_add(1, Ordering::Relaxed);
        if tx.send(f).await.is_err() {
            sh.stats.lock().unwrap().err("connection lost");
            break;
        }
    }
    // Let in-flight deliveries land before closing.
    tokio::time::sleep(Duration::from_secs(2)).await;
    drop(tx);
    reader.abort();
    let _ = writer.await;
}

fn pct(v: &mut [u64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i] as f64 / 1000.0
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if args.new_root {
        let k = proto::generate_signing_key();
        println!("root seed  {}", hex::encode(k.to_bytes()));
        println!("root pub   {}", hex::encode(k.verifying_key().to_bytes()));
        return;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut _files_dir = None;
    let (url, root) = match &args.url {
        Some(url) => {
            let seed = args.root_seed.as_deref().expect("--url needs --root-seed");
            let seed: [u8; 32] = hex::decode(seed).ok().and_then(|b| b.try_into().ok()).expect("seed: 32 bytes hex");
            (url.clone(), SigningKey::from_bytes(&seed))
        }
        None => {
            let root = proto::generate_signing_key();
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = relay::Config::new(root.verifying_key().to_bytes(), proto::generate_signing_key());
            cfg.files = Some(Arc::new(relay::files::Files::open(dir.path(), Default::default()).unwrap()));
            _files_dir = Some(dir);
            let r = relay::Relay::new(cfg);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, relay::router(r)).await.unwrap() });
            (format!("ws://{addr}/ws"), root)
        }
    };

    // One user per device keeps certs small; the relay treats devices independently anyway.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut devs = vec![];
    for i in 0..args.devices {
        let (id, key) = (Id::random(), proto::generate_signing_key());
        let cert = Cert {
            user_id: Id::random(),
            name: format!("load{i}"),
            issued_at: now - 60,
            expires_at: now + 86400,
            devices: vec![Device { device_id: id, sign_pub: key.verifying_key().to_bytes(), enc_pub: [7; 32] }],
        }
        .sign(&root)
        .unwrap();
        devs.push((id, key, cert));
    }
    let sh = Arc::new(Shared {
        start: Instant::now(),
        stats: Mutex::default(),
        devices: devs.iter().map(|d| d.0).collect(),
        stop: AtomicBool::new(false),
        sent: AtomicU64::new(0),
    });
    let args = Arc::new(args);
    let gate = Arc::new(Semaphore::new(args.connect_parallel));
    println!(
        "{} devices → {url}: {}/s messages of {} B, {}/s signals, {}/min photos of {} KiB each, {} s",
        args.devices, args.msg_rate, args.msg_size, args.signal_rate, args.photos_per_min, args.photo_kb, args.secs
    );
    let tasks: Vec<_> = devs
        .into_iter()
        .map(|(id, key, cert)| tokio::spawn(device(sh.clone(), args.clone(), url.clone(), id, key, cert, gate.clone())))
        .collect();
    tokio::time::sleep(Duration::from_secs(args.secs)).await;
    sh.stop.store(true, Ordering::Relaxed);
    for t in tasks {
        let _ = t.await;
    }

    let secs = sh.start.elapsed().as_secs_f64();
    let mut st = sh.stats.lock().unwrap();
    println!("\nframes sent: {} ({:.0}/s)", sh.sent.load(Ordering::Relaxed), sh.sent.load(Ordering::Relaxed) as f64 / secs);
    println!("{:<10} {:>8} {:>9} {:>9} {:>9} {:>9}", "kind", "count", "p50 ms", "p95 ms", "p99 ms", "max ms");
    for kind in ["handshake", "message", "signal", "upload", "download"] {
        let v = st.lat.entry(kind).or_default();
        v.sort_unstable();
        let max = v.last().copied().unwrap_or(0) as f64 / 1000.0;
        println!(
            "{kind:<10} {:>8} {:>9.1} {:>9.1} {:>9.1} {:>9.1}",
            v.len(),
            pct(v, 0.5),
            pct(v, 0.95),
            pct(v, 0.99),
            max
        );
    }
    if st.errors.is_empty() {
        println!("errors: none");
    } else {
        println!("errors:");
        let mut e: Vec<_> = st.errors.iter().collect();
        e.sort_by(|a, b| b.1.cmp(a.1));
        for (k, n) in e {
            println!("  {n:>6}  {k}");
        }
    }
}

/// Tiny error type so the example needs no extra dependency.
mod anyhow_like {
    pub type Result<T> = std::result::Result<T, Error>;
    #[derive(Debug)]
    pub struct Error(pub String);
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl From<String> for Error {
        fn from(s: String) -> Self {
            Error(s)
        }
    }
    impl From<&str> for Error {
        fn from(s: &str) -> Self {
            Error(s.into())
        }
    }
    impl From<serde_json::Error> for Error {
        fn from(e: serde_json::Error) -> Self {
            Error(e.to_string())
        }
    }
}
