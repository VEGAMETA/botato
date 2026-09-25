//! Shared relay state: sessions, held signals, push tokens, cert directory, manifest.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use proto::cert::Cert;
use proto::manifest::Manifest;
use proto::{Id, SigningKey};
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use crate::files::Files;
use crate::mailbox::{Limits, Mailbox};
use crate::push::{NoopPusher, PushKind, PushToken, Pusher};
use crate::turn::TurnConfig;

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    })
}

pub struct Config {
    pub root_pub: [u8; 32],
    pub server_key: SigningKey,
    pub turn: Option<TurnConfig>,
    pub pusher: Arc<dyn Pusher>,
    /// Attachment store; `upload`/`download` answer `unavailable` without it.
    pub files: Option<Arc<Files>>,
    pub clock: Clock,
    pub limits: Limits,
    pub auth_timeout: Duration,
    pub write_timeout: Duration,
    pub ping_interval: Duration,
    /// No inbound traffic (including pongs) for this long closes the session.
    pub idle_timeout: Duration,
    pub signal_hold_secs: u64,
    pub max_held_signals: usize,
    pub max_handshakes: usize,
    pub rate_per_sec: f64,
    pub rate_burst: f64,
    pub push_debounce_msg_secs: u64,
    pub push_debounce_call_secs: u64,
}

impl Config {
    pub fn new(root_pub: [u8; 32], server_key: SigningKey) -> Self {
        Config {
            root_pub,
            server_key,
            turn: None,
            pusher: Arc::new(NoopPusher),
            files: None,
            clock: system_clock(),
            limits: Limits::default(),
            auth_timeout: Duration::from_secs(10),
            // A 2 MiB attachment must reach a slow phone.
            write_timeout: Duration::from_secs(30),
            ping_interval: Duration::from_secs(30),
            // Clients ping every 10 s: a connection that died silently is dropped soon, so
            // senders learn "offline" (and use the Nostr fallback) instead of a dead socket.
            idle_timeout: Duration::from_secs(35),
            signal_hold_secs: 60,
            max_held_signals: 32,
            // Each unauthenticated socket may buffer one message up to READ_LIMIT (~2.8 MB,
            // sized for attachments): 16 of them stay far below the container's memory limit.
            max_handshakes: 16,
            rate_per_sec: 20.0,
            rate_burst: 100.0,
            push_debounce_msg_secs: 30,
            push_debounce_call_secs: 5,
        }
    }
}

pub(crate) enum Out {
    Frame(proto::frames::ServerFrame),
    Close(u16, &'static str),
}

#[derive(Clone)]
pub(crate) struct SessionHandle {
    pub conn: u64,
    pub user: Id,
    pub tx: mpsc::Sender<Out>,
    pub sig_tx: mpsc::Sender<Out>,
    pub wake: Arc<Notify>,
    pub cancel: CancellationToken,
}

impl SessionHandle {
    pub fn kick(&self, code: u16, reason: &'static str) {
        if self.tx.try_send(Out::Close(code, reason)).is_err() {
            self.cancel.cancel();
        }
    }
}

pub(crate) struct Held {
    pub from: Id,
    pub blob: Vec<u8>,
    pub expires: u64,
}

#[derive(Default)]
struct PushState {
    token: Option<PushToken>,
    last_msg: Option<u64>,
    last_call: Option<u64>,
}

struct DirEntry {
    cert: Cert,
    raw: Arc<[u8]>,
}

struct ManifestState {
    raw: Arc<[u8]>,
    m: Manifest,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<Id, SessionHandle>,
    held: HashMap<Id, VecDeque<Held>>,
    pushes: HashMap<Id, PushState>,
    /// user_id -> newest cert seen
    directory: HashMap<Id, DirEntry>,
    /// device_id -> user_id, from the directory
    device_owner: HashMap<Id, Id>,
    manifest: Option<ManifestState>,
    next_conn: u64,
    closed: bool,
}

impl Inner {
    fn revoked(&self, id: &Id) -> bool {
        self.manifest.as_ref().is_some_and(|m| m.m.is_revoked(id))
    }

    fn device_revoked(&self, device: &Id) -> bool {
        self.revoked(device)
            || self
                .device_owner
                .get(device)
                .is_some_and(|u| self.revoked(u))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManifestError {
    Invalid(proto::Error),
    Stale,
}

pub struct Relay {
    pub(crate) cfg: Config,
    pub(crate) server_pub: [u8; 32],
    pub(crate) handshakes: Arc<Semaphore>,
    inner: Mutex<Inner>,
    mailbox: Mutex<Mailbox>,
}

pub(crate) enum AuthCheck {
    Ok,
    Revoked,
    Superseded,
}

impl Relay {
    pub fn new(cfg: Config) -> Arc<Self> {
        Arc::new(Relay {
            server_pub: cfg.server_key.verifying_key().to_bytes(),
            handshakes: Arc::new(Semaphore::new(cfg.max_handshakes)),
            mailbox: Mutex::new(Mailbox::new(cfg.limits.clone())),
            inner: Mutex::new(Inner::default()),
            cfg,
        })
    }

    pub fn server_pub(&self) -> [u8; 32] {
        self.server_pub
    }

    pub(crate) fn now(&self) -> u64 {
        (self.cfg.clock)()
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub fn mailbox(&self) -> MutexGuard<'_, Mailbox> {
        self.mailbox.lock().unwrap()
    }

    pub fn manifest_raw(&self) -> Option<Arc<[u8]>> {
        self.inner().manifest.as_ref().map(|m| m.raw.clone())
    }

    /// Installs a newer signed manifest and applies its revocations.
    pub fn set_manifest(&self, raw: &[u8]) -> Result<(), ManifestError> {
        let m = Manifest::verify(&self.cfg.root_pub, raw).map_err(ManifestError::Invalid)?;
        let (kick, purge) = {
            let mut inner = self.inner();
            if let Some(cur) = &inner.manifest
                && m.version <= cur.m.version
            {
                return if &*cur.raw == raw {
                    Ok(())
                } else {
                    Err(ManifestError::Stale)
                };
            }
            if !m.has_server(&self.server_pub) {
                eprintln!("manifest: this server's key is not listed");
            }
            inner.manifest = Some(ManifestState { raw: raw.into(), m });
            let inner = &mut *inner;
            let mut purge: Vec<Id> = vec![];
            for (dev, user) in &inner.device_owner {
                if inner.manifest.as_ref().unwrap().m.is_revoked(user) {
                    purge.push(*dev);
                }
            }
            purge.extend(inner.manifest.as_ref().unwrap().m.revoked.iter().copied());
            let kick: Vec<SessionHandle> = inner
                .sessions
                .iter()
                .filter(|(dev, h)| purge.contains(dev) || purge.contains(&h.user))
                .map(|(_, h)| h.clone())
                .collect();
            for id in &purge {
                inner.held.remove(id);
                inner.pushes.remove(id);
                // device_owner is kept so that sends to a revoked user's devices are refused.
                inner.directory.remove(id);
            }
            (kick, purge)
        };
        for h in kick {
            h.kick(proto::frames::close::REVOKED, "revoked");
        }
        self.mailbox().purge(&purge);
        if let Some(f) = &self.cfg.files {
            f.sweep(self.now(), &purge);
        }
        Ok(())
    }

    /// Checks revocation and cert freshness, then records the cert in the directory.
    pub(crate) fn admit(&self, cert: &Cert, raw: &[u8], device: &Id) -> AuthCheck {
        let mut inner = self.inner();
        if inner.revoked(&cert.user_id) || inner.revoked(device) {
            return AuthCheck::Revoked;
        }
        match inner.directory.get(&cert.user_id) {
            // A newer cert without this device means it was removed.
            Some(e) if e.cert.issued_at > cert.issued_at => {
                if e.cert.device(device).is_none() {
                    return AuthCheck::Superseded;
                }
            }
            Some(e) if e.cert.issued_at == cert.issued_at => {}
            _ => {
                if let Some(old) = inner.directory.remove(&cert.user_id) {
                    for d in old.cert.devices {
                        inner.device_owner.remove(&d.device_id);
                    }
                }
                for d in &cert.devices {
                    inner.device_owner.insert(d.device_id, cert.user_id);
                }
                inner.directory.insert(
                    cert.user_id,
                    DirEntry {
                        cert: cert.clone(),
                        raw: raw.into(),
                    },
                );
            }
        }
        AuthCheck::Ok
    }

    /// Makes the session active for its device; returns the displaced session and held signals.
    pub(crate) fn register(
        &self,
        device: Id,
        mut handle: SessionHandle,
    ) -> Option<(SessionHandle, Option<SessionHandle>, Vec<Held>)> {
        let mut inner = self.inner();
        if inner.closed {
            return None;
        }
        inner.next_conn += 1;
        handle.conn = inner.next_conn;
        let old = inner.sessions.insert(device, handle.clone());
        let now = self.now();
        let held = inner
            .held
            .remove(&device)
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.expires > now)
            .collect();
        Some((handle, old, held))
    }

    pub(crate) fn unregister(&self, device: &Id, conn: u64) {
        let mut inner = self.inner();
        if inner.sessions.get(device).is_some_and(|h| h.conn == conn) {
            inner.sessions.remove(device);
        }
    }

    /// The subset of `devices` with a live session.
    pub(crate) fn online(&self, devices: &[Id]) -> Vec<Id> {
        let inner = self.inner();
        devices.iter().filter(|d| inner.sessions.contains_key(d)).copied().collect()
    }

    pub(crate) fn is_device_revoked(&self, device: &Id) -> bool {
        self.inner().device_revoked(device)
    }

    pub(crate) fn directory(&self) -> Vec<Vec<u8>> {
        let inner = self.inner();
        inner.directory.values().map(|e| e.raw.to_vec()).collect()
    }

    /// Wakes the recipient's pump, or sends a wake-up push if it is offline.
    pub(crate) fn notify_message(&self, device: &Id) {
        let h = self.inner().sessions.get(device).cloned();
        match h {
            Some(h) => h.wake.notify_one(),
            None => self.maybe_push(device, PushKind::Message),
        }
    }

    /// Delivers a signal live or holds it briefly. Returns true if delivered live.
    pub(crate) fn route_signal(&self, from: Id, to: Id, blob: Vec<u8>) -> bool {
        let h = self.inner().sessions.get(&to).cloned();
        if let Some(h) = h {
            let frame = proto::frames::ServerFrame::Signal {
                from,
                blob: blob.clone(),
            };
            if h.sig_tx.try_send(Out::Frame(frame)).is_ok() {
                return true;
            }
        }
        {
            let mut inner = self.inner();
            let max = self.cfg.max_held_signals;
            let q = inner.held.entry(to).or_default();
            if q.len() >= max {
                q.pop_front();
            }
            q.push_back(Held {
                from,
                blob,
                expires: self.now() + self.cfg.signal_hold_secs,
            });
        }
        self.maybe_push(&to, PushKind::Call);
        false
    }

    pub(crate) fn set_push_token(&self, device: Id, token: PushToken) {
        self.inner().pushes.entry(device).or_default().token = Some(token);
    }

    fn maybe_push(&self, device: &Id, kind: PushKind) {
        let now = self.now();
        let token = {
            let mut inner = self.inner();
            let Some(ps) = inner.pushes.get_mut(device) else {
                return;
            };
            let Some(token) = ps.token.clone() else {
                return;
            };
            let (last, debounce) = match kind {
                PushKind::Message => (&mut ps.last_msg, self.cfg.push_debounce_msg_secs),
                PushKind::Call => (&mut ps.last_call, self.cfg.push_debounce_call_secs),
            };
            if last.is_some_and(|t| now < t + debounce) {
                return;
            }
            *last = Some(now);
            token
        };
        self.cfg.pusher.push(&token, kind);
    }

    /// Expires mailbox entries and held signals.
    pub fn sweep(&self) {
        let now = self.now();
        self.mailbox().expire(now);
        if let Some(f) = &self.cfg.files {
            f.sweep(now, &[]);
        }
        let mut inner = self.inner();
        inner.held.retain(|_, q| {
            q.retain(|h| h.expires > now);
            !q.is_empty()
        });
    }

    /// Disconnects everyone and refuses new sessions.
    pub fn shutdown(&self) {
        let all: Vec<SessionHandle> = {
            let mut inner = self.inner();
            inner.closed = true;
            inner.sessions.values().cloned().collect()
        };
        for h in all {
            h.kick(1001, "shutdown");
        }
    }
}
