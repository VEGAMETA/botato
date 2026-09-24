//! Encrypted attachment stored on the relay for ≤24 h. A random per-file key travels
//! only inside the E2E message (§6.2), so the relay sees ciphertext and size only.
//!
//! blob = u8 version(1) | [24] nonce | XChaCha20-Poly1305(key, nonce, plaintext, AAD = FILE ctx || file_id)
//!
//! The file id is bound through the AAD: a relay can't serve one file under another's id.

use chacha20poly1305::aead::array::Array;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};

use crate::{Error, Id, Result, ctx, random_bytes};

const VERSION: u8 = 1;
const HEADER_LEN: usize = 1 + 24;
const TAG_LEN: usize = 16;

/// Largest blob the relay accepts.
pub const MAX_FILE: usize = 2 * 1024 * 1024;
/// Largest plaintext that fits in `MAX_FILE`.
pub const MAX_PLAINTEXT: usize = MAX_FILE - HEADER_LEN - TAG_LEN;

pub fn seal(key: &[u8; 32], file_id: &Id, plaintext: &[u8]) -> Result<Vec<u8>> {
    seal_with(key, file_id, plaintext, random_bytes())
}

/// Deterministic variant for test vectors.
pub fn seal_with(key: &[u8; 32], file_id: &Id, plaintext: &[u8], nonce: [u8; 24]) -> Result<Vec<u8>> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(Error::Invalid);
    }
    let aad = [ctx::FILE, &file_id.0].concat();
    let ct = XChaCha20Poly1305::new(&Array::from(*key))
        .encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad: &aad })
        .map_err(|_| Error::Invalid)?;
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn open(key: &[u8; 32], file_id: &Id, blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < HEADER_LEN + TAG_LEN {
        return Err(Error::Truncated);
    }
    if blob[0] != VERSION {
        return Err(Error::Format);
    }
    let nonce: [u8; 24] = blob[1..HEADER_LEN].try_into().unwrap();
    let aad = [ctx::FILE, &file_id.0].concat();
    XChaCha20Poly1305::new(&Array::from(*key))
        .decrypt(&XNonce::from(nonce), Payload { msg: &blob[HEADER_LEN..], aad: &aad })
        .map_err(|_| Error::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper() {
        let (key, id) = ([5u8; 32], Id([6; 16]));
        let blob = seal(&key, &id, b"jpeg bytes").unwrap();
        assert_eq!(open(&key, &id, &blob).unwrap(), b"jpeg bytes");
        assert!(open(&key, &Id([7; 16]), &blob).is_err(), "other id");
        assert!(open(&[4; 32], &id, &blob).is_err(), "other key");
        let mut bad = blob.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&key, &id, &bad).is_err());
        assert_eq!(open(&key, &id, &blob[..10]), Err(Error::Truncated));
        assert!(seal(&key, &id, &vec![0; MAX_PLAINTEXT + 1]).is_err());
        assert_eq!(seal(&key, &id, &vec![0; MAX_PLAINTEXT]).unwrap().len(), MAX_FILE);
    }
}
