//! Attachment store: E2E-encrypted blobs on disk for ≤24 h (too big for RAM).
//!
//! Every file is encrypted once more with a key that exists only in this process, and the
//! directory is wiped at start: after a restart old files are unreadable anyway. The index
//! (owner, size, expiry) lives in RAM only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chacha20poly1305::aead::array::Array;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use proto::{Id, random_bytes};
use zeroize::Zeroizing;

#[derive(Clone, Debug)]
pub struct FileLimits {
    pub ttl_secs: u64,
    pub per_device_bytes: u64,
    pub per_device_files: usize,
    pub total_bytes: u64,
}

impl Default for FileLimits {
    fn default() -> Self {
        FileLimits {
            ttl_secs: 24 * 3600,
            per_device_bytes: 64 * 1024 * 1024,
            per_device_files: 200,
            total_bytes: 1024 * 1024 * 1024,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileError {
    Quota,
    Io,
}

struct Entry {
    owner: Id,
    size: u64,
    expires: u64,
}

pub struct Files {
    dir: PathBuf,
    key: Zeroizing<[u8; 32]>,
    limits: FileLimits,
    index: Mutex<HashMap<Id, Entry>>,
}

impl Files {
    /// Creates the directory if needed and removes whatever a previous run left there.
    pub fn open(dir: &Path, limits: FileLimits) -> std::io::Result<Files> {
        std::fs::create_dir_all(dir)?;
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            if e.file_type()?.is_file() {
                std::fs::remove_file(e.path())?;
            }
        }
        Ok(Files {
            dir: dir.to_owned(),
            key: Zeroizing::new(random_bytes()),
            limits,
            index: Mutex::new(HashMap::new()),
        })
    }

    fn path(&self, id: &Id) -> PathBuf {
        self.dir.join(id.to_hex())
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&Array::from(*self.key))
    }

    /// Stores `data` under `id`. An id already stored is a no-op success.
    pub async fn put(&self, owner: Id, id: Id, data: &[u8], now: u64) -> Result<(), FileError> {
        let size = data.len() as u64;
        {
            let mut index = self.index.lock().unwrap();
            if index.contains_key(&id) {
                return Ok(());
            }
            let (mut mine, mut count, mut total) = (0u64, 0usize, 0u64);
            for e in index.values() {
                total += e.size;
                if e.owner == owner {
                    mine += e.size;
                    count += 1;
                }
            }
            if mine + size > self.limits.per_device_bytes
                || count >= self.limits.per_device_files
                || total + size > self.limits.total_bytes
            {
                return Err(FileError::Quota);
            }
            // Reserve the slot before the write so concurrent uploads count against quota.
            index.insert(id, Entry { owner, size, expires: now + self.limits.ttl_secs });
        }
        let nonce: [u8; 24] = random_bytes();
        let ct = self
            .cipher()
            .encrypt(&XNonce::from(nonce), Payload { msg: data, aad: &id.0 })
            .map_err(|_| FileError::Io)?;
        let tmp = self.dir.join(format!("{}.tmp", id.to_hex()));
        let res = async {
            tokio::fs::write(&tmp, [&nonce[..], &ct].concat()).await?;
            tokio::fs::rename(&tmp, self.path(&id)).await
        }
        .await;
        if res.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
            self.index.lock().unwrap().remove(&id);
            return Err(FileError::Io);
        }
        Ok(())
    }

    /// The stored bytes, or `None` if unknown, expired or unreadable.
    pub async fn get(&self, id: &Id, now: u64) -> Option<Vec<u8>> {
        {
            let index = self.index.lock().unwrap();
            let e = index.get(id)?;
            if e.expires <= now {
                return None;
            }
        }
        let raw = tokio::fs::read(self.path(id)).await.ok()?;
        if raw.len() < 24 {
            return None;
        }
        let (nonce, ct) = raw.split_at(24);
        self.cipher()
            .decrypt(&XNonce::from(<[u8; 24]>::try_from(nonce).unwrap()), Payload { msg: ct, aad: &id.0 })
            .ok()
    }

    /// Deletes expired files, and every file uploaded by one of `owners` (revoked devices).
    pub fn sweep(&self, now: u64, owners: &[Id]) {
        let gone: Vec<Id> = {
            let mut index = self.index.lock().unwrap();
            let gone: Vec<Id> = index
                .iter()
                .filter(|(_, e)| e.expires <= now || owners.contains(&e.owner))
                .map(|(id, _)| *id)
                .collect();
            for id in &gone {
                index.remove(id);
            }
            gone
        };
        for id in gone {
            let _ = std::fs::remove_file(self.path(&id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(limits: FileLimits) -> (tempfile::TempDir, Files) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("leftover"), b"old").unwrap();
        let f = Files::open(dir.path(), limits).unwrap();
        assert!(!dir.path().join("leftover").exists());
        (dir, f)
    }

    #[tokio::test]
    async fn put_get_expire_quota() {
        let limits = FileLimits { per_device_bytes: 10, ..Default::default() };
        let (dir, f) = store(limits);
        let (a, b) = (Id([1; 16]), Id([2; 16]));
        f.put(a, Id([9; 16]), b"hello", 100).await.unwrap();
        f.put(a, Id([9; 16]), b"hello", 100).await.unwrap(); // idempotent
        assert_eq!(f.get(&Id([9; 16]), 101).await.unwrap(), b"hello");
        // On disk it is not the plaintext.
        let raw = std::fs::read(dir.path().join(Id([9; 16]).to_hex())).unwrap();
        assert!(!raw.windows(5).any(|w| w == b"hello"));

        assert_eq!(f.put(a, Id([8; 16]), b"123456", 100).await, Err(FileError::Quota));
        f.put(b, Id([8; 16]), b"123456", 100).await.unwrap();

        f.sweep(100, &[b]);
        assert!(f.get(&Id([8; 16]), 101).await.is_none());
        assert!(f.get(&Id([9; 16]), 100 + 24 * 3600).await.is_none());
        f.sweep(100 + 24 * 3600, &[]);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
