use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, Subcommand};
use proto::SigningKey;
use relay::turn::TurnConfig;
use relay::{Config, Relay};

#[derive(Parser)]
#[command(about = "botato relay")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the relay. Generates the server key on first start.
    Serve(ServeArgs),
    /// Create the server key if missing and print its public part (hex).
    Keygen {
        #[arg(long, env = "RELAY_KEY", default_value = "/secrets/server.key")]
        key: PathBuf,
    },
    /// Print the public part of an existing server key (hex).
    Pubkey {
        #[arg(long, env = "RELAY_KEY", default_value = "/secrets/server.key")]
        key: PathBuf,
    },
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long, env = "RELAY_LISTEN", default_value = "0.0.0.0:8080")]
    listen: String,
    #[arg(long, env = "RELAY_KEY", default_value = "/secrets/server.key")]
    key: PathBuf,
    /// Admin root public key, hex.
    #[arg(long, env = "RELAY_ROOT_PUB")]
    root_pub: String,
    #[arg(long, env = "RELAY_MANIFEST", default_value = "/config/manifest.bin")]
    manifest: PathBuf,
    /// File with the coturn static-auth-secret. TURN is disabled if unset.
    #[arg(long, env = "RELAY_TURN_SECRET_FILE")]
    turn_secret_file: Option<PathBuf>,
    /// TURN URLs handed to clients, comma-separated.
    #[arg(long, env = "RELAY_TURN_URLS", value_delimiter = ',')]
    turn_urls: Vec<String>,
    /// FCM service account JSON. Push is disabled if unset.
    #[arg(long, env = "RELAY_FCM_CREDENTIALS")]
    fcm_credentials: Option<PathBuf>,
    /// Directory for encrypted attachments (wiped at start). Attachments are disabled if unset.
    #[arg(long, env = "RELAY_FILES_DIR")]
    files_dir: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Serve(a) => serve(a),
        Cmd::Keygen { key } => load_or_create_key(&key)
            .map(|k| println!("{}", hex::encode(k.verifying_key().to_bytes()))),
        Cmd::Pubkey { key } => {
            load_key(&key).map(|k| println!("{}", hex::encode(k.verifying_key().to_bytes())))
        }
    };
    if let Err(e) = res {
        eprintln!("relay: {e}");
        std::process::exit(1);
    }
}

type Res<T> = Result<T, String>;

fn load_key(path: &Path) -> Res<SigningKey> {
    let b = std::fs::read(path).map_err(|e| format!("read key: {e}"))?;
    let seed: [u8; 32] = b
        .as_slice()
        .try_into()
        .map_err(|_| "key file must be 32 raw bytes".to_string())?;
    Ok(SigningKey::from_bytes(&seed))
}

fn load_or_create_key(path: &Path) -> Res<SigningKey> {
    if path.exists() {
        return load_key(path);
    }
    let k = proto::generate_signing_key();
    write_private(path, &k.to_bytes()).map_err(|e| format!("write key: {e}"))?;
    Ok(k)
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(data)
}

fn serve(a: ServeArgs) -> Res<()> {
    let root_pub: [u8; 32] = hex::decode(a.root_pub.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or("root-pub must be 32 bytes hex")?;
    let key = load_or_create_key(&a.key)?;
    let mut cfg = Config::new(root_pub, key);
    if let Some(p) = &a.turn_secret_file {
        let secret = std::fs::read(p).map_err(|e| format!("read turn secret: {e}"))?;
        let secret = String::from_utf8_lossy(&secret).trim().as_bytes().to_vec();
        if secret.is_empty() || a.turn_urls.is_empty() {
            return Err("turn secret and turn urls must both be set".into());
        }
        cfg.turn = Some(TurnConfig {
            secret,
            urls: a.turn_urls.clone(),
            ttl_secs: 2 * 3600,
        });
    }

    if let Some(dir) = &a.files_dir {
        let files = relay::files::Files::open(dir, Default::default())
            .map_err(|e| format!("files dir: {e}"))?;
        cfg.files = Some(std::sync::Arc::new(files));
    }

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        if let Some(p) = &a.fcm_credentials {
            let json = std::fs::read(p).map_err(|e| format!("read fcm credentials: {e}"))?;
            let fcm = relay::fcm::FcmPusher::from_json(&json)?;
            // Fail loudly at start rather than silently at the first incoming call.
            fcm.check().await.map_err(|e| format!("fcm: {e}"))?;
            cfg.pusher = std::sync::Arc::new(fcm);
            eprintln!("relay: push via FCM enabled");
        }
        let relay = Relay::new(cfg);
        tokio::spawn(relay::watch_manifest(
            relay.clone(),
            a.manifest.clone(),
            Duration::from_secs(30),
        ));
        tokio::spawn(relay::janitor(relay.clone(), Duration::from_secs(60)));
        let listener = tokio::net::TcpListener::bind(&a.listen)
            .await
            .map_err(|e| format!("bind: {e}"))?;
        eprintln!(
            "relay: listening on {}, server key {}",
            a.listen,
            hex::encode(relay.server_pub())
        );
        let r = relay.clone();
        axum::serve(listener, relay::router(relay))
            .with_graceful_shutdown(async move {
                shutdown_signal().await;
                r.shutdown();
            })
            .await
            .map_err(|e| e.to_string())
    })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
