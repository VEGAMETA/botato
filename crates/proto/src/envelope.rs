//! E2E envelope for everything that travels through the relay between devices:
//! call signaling, messages, receipts. The relay only ever sees `blob`.
//!
//! inner     = u8 format(1) | u8 kind | id sender_device | u64 sent_at | u32 len | payload
//! sig       = Ed25519(sender, ENVELOPE || recipient_device || eph_pub || inner)
//! key       = HKDF-SHA256(salt = eph_pub || recipient_enc_pub, ikm = X25519(eph, recipient_enc_pub),
//!                         info = "botato/v1/envelope-key") -> 32 bytes
//! header    = u8 version(1) | [32] eph_pub | [24] nonce
//! blob      = header || XChaCha20-Poly1305(key, nonce, inner || sig, AAD = header || recipient_device)
//!
//! The signature binds the recipient and the ephemeral key, so a recipient can't
//! re-encrypt a signed envelope to someone else. Opening yields an `Unverified` value:
//! the caller must look up the sender's key in its certificate and call `verify`.

use chacha20poly1305::aead::array::Array;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::codec::{Reader, Writer};
use crate::device::DeviceSecret;
use crate::{Error, Id, KEY_LEN, Result, SIG_LEN, ctx, sign, verify};

const VERSION: u8 = 1;
const INNER_FORMAT: u8 = 1;
const HEADER_LEN: usize = 1 + 32 + 24;
const KEY_INFO: &[u8] = b"botato/v1/envelope-key";
/// Leaves room for the envelope overhead under `frames::MAX_BLOB`.
pub const MAX_PAYLOAD: usize = crate::frames::MAX_BLOB - 256;

pub const KIND_SIGNAL: u8 = 1;
pub const KIND_MESSAGE: u8 = 2;
pub const KIND_RECEIPT: u8 = 3;
pub const KIND_UPDATE: u8 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub kind: u8,
    pub sender: Id,
    pub sent_at: u64,
    pub payload: Vec<u8>,
}

/// Decrypted but not yet authenticated.
pub struct Unverified {
    opened: Opened,
    recipient: Id,
    eph_pub: [u8; KEY_LEN],
    inner: Vec<u8>,
    sig: Vec<u8>,
}

impl Unverified {
    /// Claimed sender device: look up its `sign_pub` in a verified certificate.
    pub fn sender(&self) -> Id {
        self.opened.sender
    }

    pub fn verify(self, sender_sign_pub: &[u8]) -> Result<Opened> {
        verify(
            sender_sign_pub,
            ctx::ENVELOPE,
            &[&self.recipient.0, &self.eph_pub, &self.inner],
            &self.sig,
        )?;
        Ok(self.opened)
    }
}

fn derive_key(shared: &[u8; 32], eph_pub: &[u8; 32], recipient_enc_pub: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let salt = [&eph_pub[..], &recipient_enc_pub[..]].concat();
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared)
        .expand(KEY_INFO, key.as_mut())
        .expect("32 bytes is a valid HKDF output length");
    key
}

fn encode_inner(kind: u8, sender: &Id, sent_at: u64, payload: &[u8]) -> Result<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return Err(Error::Invalid);
    }
    let mut w = Writer::default();
    w.u8(INNER_FORMAT);
    w.u8(kind);
    w.id(sender);
    w.u64(sent_at);
    w.raw(&(payload.len() as u32).to_be_bytes());
    w.raw(payload);
    Ok(w.0)
}

pub fn seal(
    sender: &DeviceSecret,
    recipient: &Id,
    recipient_enc_pub: &[u8; KEY_LEN],
    kind: u8,
    sent_at: u64,
    payload: &[u8],
) -> Result<Vec<u8>> {
    seal_with(
        sender,
        recipient,
        recipient_enc_pub,
        kind,
        sent_at,
        payload,
        StaticSecret::from(crate::random_bytes::<32>()),
        crate::random_bytes::<24>(),
    )
}

/// Deterministic variant for test vectors.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn seal_with(
    sender: &DeviceSecret,
    recipient: &Id,
    recipient_enc_pub: &[u8; KEY_LEN],
    kind: u8,
    sent_at: u64,
    payload: &[u8],
    eph: StaticSecret,
    nonce: [u8; 24],
) -> Result<Vec<u8>> {
    let eph_pub = PublicKey::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&PublicKey::from(*recipient_enc_pub));
    if !shared.was_contributory() {
        return Err(Error::Invalid);
    }
    let key = derive_key(shared.as_bytes(), &eph_pub, recipient_enc_pub);

    let inner = encode_inner(kind, &sender.device_id, sent_at, payload)?;
    let sig = sign(sender.signing_key(), ctx::ENVELOPE, &[&recipient.0, &eph_pub, &inner]);
    let mut plain = Zeroizing::new(inner);
    plain.extend_from_slice(&sig);

    let mut blob = Vec::with_capacity(HEADER_LEN + plain.len() + 16);
    blob.push(VERSION);
    blob.extend_from_slice(&eph_pub);
    blob.extend_from_slice(&nonce);
    let aad = [&blob[..], &recipient.0[..]].concat();
    let ct = XChaCha20Poly1305::new(&Array::from(*key))
        .encrypt(&XNonce::from(nonce), Payload { msg: &plain, aad: &aad })
        .map_err(|_| Error::Invalid)?;
    blob.extend_from_slice(&ct);
    Ok(blob)
}

/// Decrypts an envelope addressed to `me`. The result must still be verified.
pub fn open(me: &DeviceSecret, blob: &[u8]) -> Result<Unverified> {
    if blob.len() < HEADER_LEN + 16 {
        return Err(Error::Truncated);
    }
    if blob[0] != VERSION {
        return Err(Error::Format);
    }
    let (header, ct) = blob.split_at(HEADER_LEN);
    let eph_pub: [u8; 32] = header[1..33].try_into().unwrap();
    let nonce: [u8; 24] = header[33..].try_into().unwrap();
    let shared = me.enc_secret().diffie_hellman(&PublicKey::from(eph_pub));
    if !shared.was_contributory() {
        return Err(Error::Invalid);
    }
    let key = derive_key(shared.as_bytes(), &eph_pub, &me.enc_pub());
    let aad = [header, &me.device_id.0[..]].concat();
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new(&Array::from(*key))
            .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad: &aad })
            .map_err(|_| Error::Signature)?,
    );
    if plain.len() < SIG_LEN {
        return Err(Error::Truncated);
    }
    let (inner, sig) = plain.split_at(plain.len() - SIG_LEN);
    let mut r = Reader(inner);
    if r.u8()? != INNER_FORMAT {
        return Err(Error::Format);
    }
    let kind = r.u8()?;
    let sender = r.id()?;
    let sent_at = r.u64()?;
    let len = u32::from_be_bytes(r.fixed()?) as usize;
    let payload = r.fixed_vec(len)?;
    r.finish()?;
    Ok(Unverified {
        opened: Opened { kind, sender, sent_at, payload },
        recipient: me.device_id,
        eph_pub,
        inner: inner.to_vec(),
        sig: sig.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_attacks() {
        let alice = DeviceSecret::generate();
        let bob = DeviceSecret::generate();
        let eve = DeviceSecret::generate();
        let blob = seal(&alice, &bob.device_id, &bob.enc_pub(), KIND_SIGNAL, 42, b"offer").unwrap();

        let u = open(&bob, &blob).unwrap();
        assert_eq!(u.sender(), alice.device_id);
        let o = u.verify(&alice.sign_pub()).unwrap();
        assert_eq!((o.kind, o.sent_at, &o.payload[..]), (KIND_SIGNAL, 42, &b"offer"[..]));

        // Wrong recipient can't open; wrong sender key doesn't verify.
        assert!(open(&eve, &blob).is_err());
        assert!(open(&bob, &blob).unwrap().verify(&eve.sign_pub()).is_err());

        // Tampering anywhere fails.
        for i in [0, 5, 40, 60, blob.len() - 1] {
            let mut t = blob.clone();
            t[i] ^= 1;
            assert!(open(&bob, &t).and_then(|u| u.verify(&alice.sign_pub())).is_err(), "byte {i}");
        }

        // Low-order recipient key is refused.
        assert!(seal(&alice, &bob.device_id, &[0; 32], KIND_SIGNAL, 0, b"x").is_err());
        assert!(seal(&alice, &bob.device_id, &bob.enc_pub(), KIND_SIGNAL, 0, &vec![0; MAX_PAYLOAD + 1]).is_err());
    }
}
