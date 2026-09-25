//! Admin state: users, devices, revocations and the manifest source.
//! Everything here is public data except `root.key`, which is password-encrypted.
//!
//! Layout of the admin directory:
//!   root.key        encrypted root key (see `rootkey`)
//!   state.json      users and manifest source
//!   manifest.bin    last signed manifest
//!   certs/<user>.txt  last issued cert of each user, text form

pub mod rootkey;

use std::fs;
use std::path::{Path, PathBuf};

use proto::cert::{self, Cert, Device};
use proto::enroll::{self, Request};
use proto::manifest::{Manifest, ServerEntry};
use proto::{Id, SigningKey};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CERT_DAYS: u64 = 365;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

macro_rules! ctx {
    ($e:expr, $what:expr) => {
        $e.map_err(|e| Error(format!("{}: {}", $what, e)))
    };
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRec {
    pub device_id: String,
    pub sign_pub: String,
    pub enc_pub: String,
    pub added_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserRec {
    pub user_id: String,
    pub name: String,
    pub devices: Vec<DeviceRec>,
    /// Devices removed from this user; revoked forever.
    #[serde(default)]
    pub removed_devices: Vec<String>,
    pub issued_at: u64,
    pub expires_at: u64,
    #[serde(default)]
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerRec {
    pub url: String,
    #[serde(default)]
    pub ip_fallback: String,
    pub server_pub: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSrc {
    /// Version of the last signed manifest.
    pub version: u64,
    pub servers: Vec<ServerRec>,
    pub stun: Vec<String>,
    pub nostr_relays: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    /// Root public key (hex); public, kept here so reading it needs no password.
    #[serde(default)]
    pub root_pub: String,
    pub users: Vec<UserRec>,
    pub manifest: ManifestSrc,
}

pub struct Admin {
    dir: PathBuf,
    pub state: State,
}

pub struct Issued {
    pub user_id: Id,
    pub cert: Vec<u8>,
    pub text: String,
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    hex::decode(s)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| Error(format!("bad 32-byte hex: {s}")))
}

pub fn parse_id(s: &str) -> Result<Id> {
    Id::from_hex(s.trim()).ok_or_else(|| Error(format!("bad id: {s}")))
}

impl Admin {
    pub fn root_key_path(dir: &Path) -> PathBuf {
        dir.join("root.key")
    }

    /// Creates the directory with a new root key. Refuses to overwrite.
    pub fn init(dir: &Path, password: &[u8], kdf: rootkey::Kdf) -> Result<SigningKey> {
        let path = Self::root_key_path(dir);
        if path.exists() {
            return err(format!("{} already exists", path.display()));
        }
        ctx!(fs::create_dir_all(dir.join("certs")), "create dir")?;
        let key = proto::generate_signing_key();
        write_new(&path, &rootkey::seal(&key, password, kdf))?;
        Admin {
            dir: dir.to_owned(),
            state: State {
                root_pub: hex::encode(key.verifying_key().to_bytes()),
                ..Default::default()
            },
        }
        .save()?;
        Ok(key)
    }

    pub fn open(dir: &Path) -> Result<Admin> {
        let raw = ctx!(
            fs::read(dir.join("state.json")),
            format!("read {}/state.json (run init?)", dir.display())
        )?;
        let state = ctx!(serde_json::from_slice(&raw), "parse state.json")?;
        Ok(Admin {
            dir: dir.to_owned(),
            state,
        })
    }

    pub fn unlock(&self, password: &[u8]) -> Result<SigningKey> {
        let raw = ctx!(fs::read(Self::root_key_path(&self.dir)), "read root.key")?;
        let key = rootkey::open(&raw, password).map_err(|e| match e {
            rootkey::Error::Decrypt => Error("wrong password".into()),
            rootkey::Error::Format => Error("root.key is corrupted".into()),
        })?;
        let pub_hex = hex::encode(key.verifying_key().to_bytes());
        if !self.state.root_pub.is_empty() && self.state.root_pub != pub_hex {
            return err("root.key does not match root_pub in state.json");
        }
        Ok(key)
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.state).unwrap();
        write_atomic(&self.dir.join("state.json"), &json)
    }

    fn user_mut(&mut self, id: &Id) -> Result<&mut UserRec> {
        let hex = id.to_hex();
        match self.state.users.iter_mut().find(|u| u.user_id == hex) {
            Some(u) => Ok(u),
            None => err(format!("unknown user {hex}")),
        }
    }

    fn device_known(&self, id: &str) -> bool {
        self.state.users.iter().any(|u| {
            u.user_id == id
                || u.devices.iter().any(|d| d.device_id == id)
                || u.removed_devices.iter().any(|d| d == id)
        })
    }

    /// Issues a cert for a new user (`user = None`) or adds the device to an existing user.
    pub fn enroll(
        &mut self,
        root: &SigningKey,
        req: &Request,
        name: Option<&str>,
        user: Option<Id>,
        days: u64,
        now: u64,
    ) -> Result<Issued> {
        let dev_hex = req.device_id.to_hex();
        if self.device_known(&dev_hex) {
            return err(format!(
                "device {dev_hex} is already known (or was removed)"
            ));
        }
        let rec = DeviceRec {
            device_id: dev_hex,
            sign_pub: hex::encode(req.sign_pub),
            enc_pub: hex::encode(req.enc_pub),
            added_at: now,
        };
        let user_id = match user {
            Some(uid) => {
                let u = self.user_mut(&uid)?;
                if u.revoked {
                    return err("user is revoked");
                }
                if u.devices.len() >= cert::MAX_DEVICES {
                    return err(format!("user already has {} devices", cert::MAX_DEVICES));
                }
                u.devices.push(rec);
                if let Some(n) = name {
                    u.name = n.to_owned();
                }
                uid
            }
            None => {
                let name = name.map(str::to_owned).unwrap_or_else(|| req.name.clone());
                if name.is_empty() {
                    return err("name is required (--name)");
                }
                let uid = loop {
                    let id = Id::random();
                    if !self.device_known(&id.to_hex()) {
                        break id;
                    }
                };
                self.state.users.push(UserRec {
                    user_id: uid.to_hex(),
                    name,
                    devices: vec![rec],
                    removed_devices: vec![],
                    issued_at: 0,
                    expires_at: 0,
                    revoked: false,
                });
                uid
            }
        };
        self.issue(root, &user_id, days, now)
    }

    /// (Re)issues the user's cert with the current device list.
    /// `issued_at` strictly increases so the new cert always supersedes the old one.
    pub fn issue(
        &mut self,
        root: &SigningKey,
        user_id: &Id,
        days: u64,
        now: u64,
    ) -> Result<Issued> {
        let u = self.user_mut(user_id)?;
        if u.revoked {
            return err("user is revoked");
        }
        if u.devices.is_empty() {
            return err("user has no devices");
        }
        let issued_at = now.max(u.issued_at + 1);
        let c = Cert {
            user_id: *user_id,
            name: u.name.clone(),
            issued_at,
            expires_at: issued_at + days * 86400,
            devices: u
                .devices
                .iter()
                .map(|d| {
                    Ok(Device {
                        device_id: parse_id(&d.device_id)?,
                        sign_pub: hex32(&d.sign_pub)?,
                        enc_pub: hex32(&d.enc_pub)?,
                    })
                })
                .collect::<Result<_>>()?,
        };
        let signed = ctx!(c.sign(root), "sign cert")?;
        u.issued_at = c.issued_at;
        u.expires_at = c.expires_at;
        let text = enroll::to_text(enroll::CERT_PREFIX, &signed);
        write_atomic(
            &self
                .dir
                .join("certs")
                .join(format!("{}.txt", user_id.to_hex())),
            text.as_bytes(),
        )?;
        self.save()?;
        Ok(Issued {
            user_id: *user_id,
            cert: signed,
            text,
        })
    }

    /// Removes a device: reissues the owner's cert without it and revokes it in the next manifest.
    /// Returns `None` for the cert if it was the user's last device.
    pub fn remove_device(
        &mut self,
        root: &SigningKey,
        device: &Id,
        days: u64,
        now: u64,
    ) -> Result<Option<Issued>> {
        let hex = device.to_hex();
        let Some(u) = self
            .state
            .users
            .iter_mut()
            .find(|u| u.devices.iter().any(|d| d.device_id == hex))
        else {
            return err(format!("unknown device {hex}"));
        };
        u.devices.retain(|d| d.device_id != hex);
        u.removed_devices.push(hex);
        let uid = parse_id(&u.user_id)?;
        if u.devices.is_empty() || u.revoked {
            self.save()?;
            return Ok(None);
        }
        self.issue(root, &uid, days, now).map(Some)
    }

    pub fn revoke_user(&mut self, user_id: &Id) -> Result<()> {
        self.user_mut(user_id)?.revoked = true;
        self.save()
    }

    pub fn revoked(&self) -> Vec<Id> {
        let mut out = vec![];
        for u in &self.state.users {
            if u.revoked {
                out.extend(parse_id(&u.user_id));
            }
            out.extend(u.removed_devices.iter().filter_map(|d| Id::from_hex(d)));
        }
        out
    }

    pub fn add_server(&mut self, url: &str, server_pub: &str, ip_fallback: &str) -> Result<()> {
        hex32(server_pub)?;
        if !url.starts_with("wss://") {
            return err("server url must start with wss://");
        }
        let m = &mut self.state.manifest;
        if m.servers
            .iter()
            .any(|s| s.url == url || s.server_pub == server_pub)
        {
            return err("server with this url or key is already listed");
        }
        m.servers.push(ServerRec {
            url: url.into(),
            ip_fallback: ip_fallback.into(),
            server_pub: server_pub.to_lowercase(),
        });
        self.save()
    }

    /// Removes by url or server_pub.
    pub fn remove_server(&mut self, key: &str) -> Result<()> {
        let m = &mut self.state.manifest;
        let before = m.servers.len();
        m.servers
            .retain(|s| s.url != key && s.server_pub != key.to_lowercase());
        if m.servers.len() == before {
            return err(format!("no server {key}"));
        }
        self.save()
    }

    pub fn build_manifest(&self, version: u64, now: u64) -> Result<Manifest> {
        let m = &self.state.manifest;
        Ok(Manifest {
            version,
            issued_at: now,
            servers: m
                .servers
                .iter()
                .map(|s| {
                    Ok(ServerEntry {
                        url: s.url.clone(),
                        ip_fallback: s.ip_fallback.clone(),
                        server_pub: hex32(&s.server_pub)?,
                    })
                })
                .collect::<Result<_>>()?,
            stun: m.stun.clone(),
            nostr_relays: m.nostr_relays.clone(),
            revoked: self.revoked(),
        })
    }

    /// Signs the current state as the next manifest version and writes `manifest.bin`.
    pub fn sign_manifest(&mut self, root: &SigningKey, now: u64) -> Result<(u64, PathBuf)> {
        let version = self.state.manifest.version + 1;
        let m = self.build_manifest(version, now)?;
        let signed = ctx!(m.sign(root), "sign manifest (need at least one server)")?;
        let path = self.dir.join("manifest.bin");
        write_atomic(&path, &signed)?;
        self.state.manifest.version = version;
        self.save()?;
        Ok((version, path))
    }

    /// Signs an app release (protocol/README.md §3b): copies the files into `releases/` under
    /// their published names, hashes them and writes `releases/release.bin`. Older files there
    /// are removed, so the folder always mirrors what the server should serve.
    pub fn sign_release(
        &self,
        root: &SigningKey,
        version: &str,
        build: u64,
        files: &[(&str, &Path, String)],
        now: u64,
    ) -> Result<PathBuf> {
        use sha2::{Digest, Sha256};
        let dir = self.dir.join("releases");
        ctx!(fs::create_dir_all(&dir), "create releases dir")?;
        let mut entries = vec![];
        for (platform, src, name) in files {
            let data = ctx!(fs::read(src), format!("read {}", src.display()))?;
            let dest = dir.join(name);
            ctx!(fs::write(&dest, &data), format!("write {}", dest.display()))?;
            entries.push(proto::release::ReleaseFile {
                platform: platform.to_string(),
                file: name.clone(),
                size: data.len() as u64,
                sha256: Sha256::digest(&data).into(),
            });
        }
        let rel = proto::release::Release { version: version.into(), build, issued_at: now, files: entries };
        let signed = ctx!(rel.sign(root), "sign release")?;
        let path = dir.join("release.bin");
        write_atomic(&path, &signed)?;
        let keep: Vec<&str> = files.iter().map(|f| f.2.as_str()).chain(["release.bin"]).collect();
        for e in ctx!(fs::read_dir(&dir), "list releases dir")?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep.contains(&name.as_str()) {
                let _ = fs::remove_file(e.path());
            }
        }
        Ok(path)
    }

    pub fn cert_text(&self, user_id: &Id) -> Result<String> {
        let p = self
            .dir
            .join("certs")
            .join(format!("{}.txt", user_id.to_hex()));
        ctx!(fs::read_to_string(&p), format!("read {}", p.display()))
    }
}

fn write_new(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = ctx!(o.open(path), format!("create {}", path.display()))?;
    ctx!(f.write_all(data), format!("write {}", path.display()))?;
    ctx!(f.sync_all(), format!("sync {}", path.display()))
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    write_new(&tmp, data)?;
    ctx!(
        fs::rename(&tmp, path),
        format!("replace {}", path.display())
    )
}
