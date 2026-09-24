use std::io::{BufRead, Write};
use std::path::PathBuf;

use admin::{Admin, DEFAULT_CERT_DAYS, Result, parse_id, rootkey};
use clap::{Parser, Subcommand};
use proto::enroll::{self, Request};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(about = "botato admin: root key, certificates, manifest")]
struct Cli {
    /// Admin directory (keep it offline and backed up).
    #[arg(long, env = "BOTATO_ADMIN_DIR", default_value = "admin-data")]
    dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new root key (encrypted with a password).
    Init,
    /// Print the root public key (hex) to embed into the client.
    RootPub,
    /// Show the fingerprint of an enrollment request.
    Fingerprint { request: String },
    /// Issue a cert for a device: a new user, or `--user` to add a device to an existing one.
    Enroll {
        /// `botato-req1:...` text from the device.
        request: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        user: Option<String>,
        #[arg(long, default_value_t = DEFAULT_CERT_DAYS)]
        days: u64,
        /// Skip the fingerprint confirmation (only if already compared).
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        qr: bool,
    },
    /// List users and devices.
    Users,
    /// Print a user's current cert.
    Cert {
        user: String,
        #[arg(long)]
        qr: bool,
    },
    /// Reissue a user's cert with a fresh expiry.
    Renew {
        user: String,
        #[arg(long, default_value_t = DEFAULT_CERT_DAYS)]
        days: u64,
        #[arg(long)]
        qr: bool,
    },
    /// Remove a device (reissues the owner's cert, revokes the device in the next manifest).
    RemoveDevice {
        device: String,
        #[arg(long, default_value_t = DEFAULT_CERT_DAYS)]
        days: u64,
    },
    /// Revoke a whole user in the next manifest.
    RevokeUser { user: String },
    #[command(subcommand)]
    Manifest(ManifestCmd),
}

#[derive(Subcommand)]
enum ManifestCmd {
    /// Show the manifest source and what the next version would contain.
    Show,
    ServerAdd {
        /// e.g. wss://cdn.example.org/ws
        #[arg(long)]
        url: String,
        /// Server public key (hex), from `relay pubkey`.
        #[arg(long)]
        pub_key: String,
        #[arg(long, default_value = "")]
        ip: String,
    },
    /// Remove a server by url or public key.
    ServerRm { key: String },
    /// Replace the STUN list.
    Stun { urls: Vec<String> },
    /// Replace the Nostr relay list.
    Nostr { urls: Vec<String> },
    /// Sign the next manifest version into manifest.bin.
    Sign,
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("admin: {e}");
        std::process::exit(1);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn password(prompt: &str) -> Result<Zeroizing<String>> {
    if let Ok(p) = std::env::var("BOTATO_ADMIN_PASSWORD") {
        return Ok(Zeroizing::new(p));
    }
    rpassword::prompt_password(prompt)
        .map(Zeroizing::new)
        .map_err(|e| admin::Error(format!("read password: {e}")))
}

fn unlock(a: &Admin) -> Result<proto::SigningKey> {
    a.unlock(password("Root key password: ")?.as_bytes())
}

fn confirm(q: &str) -> bool {
    print!("{q} [y/N] ");
    std::io::stdout().flush().ok();
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s).ok();
    matches!(s.trim(), "y" | "Y" | "yes")
}

fn parse_request(s: &str) -> Result<Request> {
    let raw = enroll::from_text(enroll::REQUEST_PREFIX, s)
        .map_err(|_| admin::Error("not a botato-req1: request".into()))?;
    Request::verify(&raw).map_err(|e| admin::Error(format!("invalid request: {e}")))
}

fn print_cert(text: &str, qr: bool) {
    println!("{text}");
    if qr {
        match qrcode::QrCode::new(text.as_bytes()) {
            Ok(code) => println!(
                "{}",
                code.render::<qrcode::render::unicode::Dense1x2>()
                    .quiet_zone(true)
                    .build()
            ),
            Err(e) => eprintln!("admin: qr: {e}"),
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let dir = cli.dir;
    match cli.cmd {
        Cmd::Init => {
            let p1 = password("New root key password: ")?;
            if std::env::var("BOTATO_ADMIN_PASSWORD").is_err() {
                let p2 = password("Repeat: ")?;
                if *p1 != *p2 {
                    return Err(admin::Error("passwords differ".into()));
                }
            }
            if p1.chars().count() < 12 {
                return Err(admin::Error(
                    "password must be at least 12 characters".into(),
                ));
            }
            let k = Admin::init(&dir, p1.as_bytes(), rootkey::Kdf::STRONG)?;
            println!(
                "root public key: {}",
                hex::encode(k.verifying_key().to_bytes())
            );
            println!(
                "Back up {} to at least two offline places.",
                Admin::root_key_path(&dir).display()
            );
        }
        Cmd::RootPub => {
            let a = Admin::open(&dir)?;
            println!("{}", hex::encode(unlock(&a)?.verifying_key().to_bytes()));
        }
        Cmd::Fingerprint { request } => {
            let r = parse_request(&request)?;
            println!("device {}  name {:?}", r.device_id, r.name);
            println!("fingerprint: {}", r.fingerprint());
        }
        Cmd::Enroll {
            request,
            name,
            user,
            days,
            yes,
            qr,
        } => {
            let r = parse_request(&request)?;
            let mut a = Admin::open(&dir)?;
            let user = user.as_deref().map(parse_id).transpose()?;
            println!("device {}  name {:?}", r.device_id, r.name);
            println!("fingerprint: {}", r.fingerprint());
            if !yes && !confirm("Compared with the user by voice or in person, and it matches?") {
                return Err(admin::Error("aborted".into()));
            }
            let root = unlock(&a)?;
            let issued = a.enroll(&root, &r, name.as_deref(), user, days, now())?;
            println!("user {}", issued.user_id);
            print_cert(&issued.text, qr);
        }
        Cmd::Users => {
            let a = Admin::open(&dir)?;
            for u in &a.state.users {
                let status = if u.revoked { " REVOKED" } else { "" };
                println!(
                    "{}  {}  expires {}{}",
                    u.user_id, u.name, u.expires_at, status
                );
                for d in &u.devices {
                    let fp = enroll::fingerprint(
                        &parse_id(&d.device_id)?,
                        &hex32(&d.sign_pub),
                        &hex32(&d.enc_pub),
                    );
                    println!("    device {}  fp {}", d.device_id, fp);
                }
                for d in &u.removed_devices {
                    println!("    removed {d}");
                }
            }
        }
        Cmd::Cert { user, qr } => {
            let a = Admin::open(&dir)?;
            print_cert(&a.cert_text(&parse_id(&user)?)?, qr);
        }
        Cmd::Renew { user, days, qr } => {
            let mut a = Admin::open(&dir)?;
            let root = unlock(&a)?;
            print_cert(&a.issue(&root, &parse_id(&user)?, days, now())?.text, qr);
        }
        Cmd::RemoveDevice { device, days } => {
            let mut a = Admin::open(&dir)?;
            let root = unlock(&a)?;
            match a.remove_device(&root, &parse_id(&device)?, days, now())? {
                Some(i) => println!("new cert for user {}:\n{}", i.user_id, i.text),
                None => println!("that was the user's last device"),
            }
            println!("Now run `admin manifest sign` and deploy the manifest.");
        }
        Cmd::RevokeUser { user } => {
            let mut a = Admin::open(&dir)?;
            a.revoke_user(&parse_id(&user)?)?;
            println!("Now run `admin manifest sign` and deploy the manifest.");
        }
        Cmd::Manifest(m) => manifest(&dir, m)?,
    }
    Ok(())
}

fn hex32(s: &str) -> [u8; 32] {
    hex::decode(s)
        .ok()
        .and_then(|b| b.try_into().ok())
        .unwrap_or([0; 32])
}

fn manifest(dir: &std::path::Path, cmd: ManifestCmd) -> Result<()> {
    let mut a = Admin::open(dir)?;
    match cmd {
        ManifestCmd::Show => {
            let m = &a.state.manifest;
            println!("last signed version: {}", m.version);
            for s in &m.servers {
                println!(
                    "server {}  pub {}  ip {:?}",
                    s.url, s.server_pub, s.ip_fallback
                );
            }
            println!("stun: {:?}", m.stun);
            println!("nostr: {:?}", m.nostr_relays);
            let revoked: Vec<String> = a.revoked().iter().map(|i| i.to_hex()).collect();
            println!("revoked: {revoked:?}");
        }
        ManifestCmd::ServerAdd { url, pub_key, ip } => a.add_server(&url, &pub_key, &ip)?,
        ManifestCmd::ServerRm { key } => a.remove_server(&key)?,
        ManifestCmd::Stun { urls } => {
            a.state.manifest.stun = urls;
            a.save()?;
        }
        ManifestCmd::Nostr { urls } => {
            a.state.manifest.nostr_relays = urls;
            a.save()?;
        }
        ManifestCmd::Sign => {
            let root = unlock(&a)?;
            let (v, path) = a.sign_manifest(&root, now())?;
            println!("manifest version {v} written to {}", path.display());
        }
    }
    Ok(())
}
