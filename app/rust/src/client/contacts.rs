//! Verified certificates of everyone we know (including ourselves), newest per user.
//! Persisted as a JSON list of base64 certs; re-verified on every load.

use std::collections::HashMap;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use proto::Id;
use proto::cert::{Cert, Device};
use proto::manifest::Manifest;

pub struct Contacts {
    path: PathBuf,
    root_pub: [u8; 32],
    certs: HashMap<Id, (Cert, Vec<u8>)>,
}

impl Contacts {
    pub fn load(path: PathBuf, root_pub: [u8; 32], now: u64) -> Self {
        let mut c = Contacts { path, root_pub, certs: HashMap::new() };
        let stored: Vec<String> = std::fs::read(&c.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for s in stored {
            if let Ok(raw) = STANDARD.decode(s) {
                c.insert(&raw, now, None);
            }
        }
        c
    }

    fn save(&self) {
        let list: Vec<String> = self.certs.values().map(|(_, raw)| STANDARD.encode(raw)).collect();
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_vec(&list).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// Adds a cert if it verifies, isn't revoked and is newer than what we have.
    fn insert(&mut self, raw: &[u8], now: u64, manifest: Option<&Manifest>) -> bool {
        let Ok(cert) = Cert::verify(&self.root_pub, raw, now) else { return false };
        if manifest.is_some_and(|m| m.is_revoked(&cert.user_id)) {
            return false;
        }
        if self.certs.get(&cert.user_id).is_some_and(|(old, _)| old.issued_at >= cert.issued_at) {
            return false;
        }
        self.certs.insert(cert.user_id, (cert, raw.to_vec()));
        true
    }

    /// Merges a directory answer; returns true if anything changed.
    pub fn merge(&mut self, certs: &[Vec<u8>], now: u64, manifest: &Manifest) -> bool {
        let mut changed = false;
        for raw in certs {
            changed |= self.insert(raw, now, Some(manifest));
        }
        // Drop users revoked by a newer manifest.
        let before = self.certs.len();
        self.certs.retain(|u, _| !manifest.is_revoked(u));
        changed |= self.certs.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    /// A device we accept signals from: listed in a valid cert and not revoked.
    pub fn device(&self, id: &Id, manifest: &Manifest) -> Option<(&Cert, &Device)> {
        if manifest.is_revoked(id) {
            return None;
        }
        self.certs.values().find_map(|(c, _)| c.device(id).map(|d| (c, d)))
    }

    pub fn all(&self) -> impl Iterator<Item = &Cert> {
        self.certs.values().map(|(c, _)| c)
    }
}
