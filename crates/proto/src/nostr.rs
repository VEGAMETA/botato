//! Nostr fallback transport (protocol/README.md §7): routing tags between two devices, and
//! NIP-01 events that carry our envelopes or the signed manifest over public relays.
//!
//! Nostr is only a pipe: the envelope inside is sealed and signed as over our relay. The
//! secp256k1 (BIP-340) signature on an event exists because relays require one.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::{Error, Id, Result};

/// Call signals: ephemeral range, relays forward and don't store.
pub const KIND_SIGNAL: u64 = 25580;
/// The signed manifest (NIP-78 parameterized replaceable).
pub const KIND_MANIFEST: u64 = 30078;

const PAIR_INFO: &[u8] = b"botato/v1/nostr-pair";
const TAG_CTX: &[u8] = b"botato/v1/nostr-tag\0";
const MANIFEST_CTX: &[u8] = b"botato/v1/nostr-manifest\0";

/// A secret two devices share for addressing each other on Nostr.
pub struct PairKey(Zeroizing<[u8; 32]>);

/// `None` for a non-contributory peer key (all-zero shared secret).
pub fn pair_key(own_secret: &StaticSecret, peer_enc_pub: &[u8; 32]) -> Option<PairKey> {
    let own_pub = PublicKey::from(own_secret);
    let shared = own_secret.diffie_hellman(&PublicKey::from(*peer_enc_pub));
    if !shared.was_contributory() {
        return None;
    }
    let (a, b) = if own_pub.as_bytes() <= peer_enc_pub { (own_pub.to_bytes(), *peer_enc_pub) } else { (*peer_enc_pub, own_pub.to_bytes()) };
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(&a);
    salt[32..].copy_from_slice(&b);
    let mut k = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes()).expand(PAIR_INFO, &mut *k).expect("32 bytes");
    Some(PairKey(k))
}

impl PairKey {
    /// The tag events to `recipient` carry on `day` (see [`day`]).
    pub fn tag(&self, recipient: &Id, day: u32) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.0[..]).expect("any key length");
        mac.update(TAG_CTX);
        mac.update(&recipient.0);
        mac.update(&day.to_be_bytes());
        hex::encode(&mac.finalize().into_bytes()[..16])
    }
}

/// UTC day number of a unix time.
pub fn day(unix: u64) -> u32 {
    (unix / 86_400) as u32
}

/// The `d` tag of the manifest event for this admin root key.
pub fn manifest_d_tag(root_pub: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(MANIFEST_CTX);
    h.update(root_pub);
    hex::encode(&h.finalize()[..16])
}

/// A NIP-01 event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

/// NIP-01 event id: SHA-256 of `[0, pubkey, created_at, kind, tags, content]` serialized
/// without whitespace.
pub fn event_id(pubkey: &str, created_at: u64, kind: u64, tags: &[Vec<String>], content: &str) -> [u8; 32] {
    let ser = serde_json::to_string(&json!([0, pubkey, created_at, kind, tags, content])).expect("json");
    Sha256::digest(ser.as_bytes()).into()
}

impl Event {
    /// Signs with `secret` (a secp256k1 key); `aux` is BIP-340 auxiliary randomness.
    pub fn sign(secret: &[u8; 32], aux: &[u8; 32], created_at: u64, kind: u64, tags: Vec<Vec<String>>, content: String) -> Result<Event> {
        let key = k256::schnorr::SigningKey::from_slice(secret).map_err(|_| Error::Invalid)?;
        let pubkey = hex::encode(key.verifying_key().to_bytes());
        let id = event_id(&pubkey, created_at, kind, &tags, &content);
        let sig = key.sign_raw(&id, aux).map_err(|_| Error::Signature)?;
        Ok(Event { id: hex::encode(id), pubkey, created_at, kind, tags, content, sig: hex::encode(sig.to_bytes()) })
    }

    /// Checks the id and the signature (relays should have; we don't rely on them).
    pub fn verify(&self) -> Result<()> {
        let id = event_id(&self.pubkey, self.created_at, self.kind, &self.tags, &self.content);
        if hex::encode(id) != self.id {
            return Err(Error::Invalid);
        }
        let pk = hex::decode(&self.pubkey).map_err(|_| Error::Invalid)?;
        let sig = hex::decode(&self.sig).map_err(|_| Error::Signature)?;
        let vk = k256::schnorr::VerifyingKey::from_slice(&pk).map_err(|_| Error::Invalid)?;
        let sig = k256::schnorr::Signature::try_from(sig.as_slice()).map_err(|_| Error::Signature)?;
        vk.verify_raw(&id, &sig).map_err(|_| Error::Signature)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "pubkey": self.pubkey,
            "created_at": self.created_at,
            "kind": self.kind,
            "tags": self.tags,
            "content": self.content,
            "sig": self.sig,
        })
    }

    pub fn from_json(v: &Value) -> Result<Event> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned).ok_or(Error::Invalid);
        let n = |k: &str| v.get(k).and_then(Value::as_u64).ok_or(Error::Invalid);
        let tags = v
            .get("tags")
            .and_then(Value::as_array)
            .ok_or(Error::Invalid)?
            .iter()
            .map(|t| {
                t.as_array()
                    .ok_or(Error::Invalid)?
                    .iter()
                    .map(|x| x.as_str().map(str::to_owned).ok_or(Error::Invalid))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Event { id: s("id")?, pubkey: s("pubkey")?, created_at: n("created_at")?, kind: n("kind")?, tags, content: s("content")?, sig: s("sig")? })
    }

    /// The first value of tag `name`, if any.
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags.iter().find(|t| t.first().is_some_and(|n| n == name)).and_then(|t| t.get(1)).map(String::as_str)
    }

    /// The bytes carried in `content` (an envelope or a signed manifest).
    pub fn payload(&self) -> Result<Vec<u8>> {
        B64.decode(&self.content).map_err(|_| Error::Invalid)
    }
}

/// A signal event for `tag`, signed with a fresh random key (relays can't link events).
pub fn signal_event(blob: &[u8], tag: &str, now: u64) -> Result<Event> {
    let key = Zeroizing::new(random_secp_key());
    Event::sign(&key, &crate::random_bytes::<32>(), now, KIND_SIGNAL, vec![vec!["t".into(), tag.into()]], B64.encode(blob))
}

/// The manifest event, signed with the admin's Nostr key.
pub fn manifest_event(nostr_key: &[u8; 32], signed_manifest: &[u8], root_pub: &[u8; 32], now: u64) -> Result<Event> {
    Event::sign(
        nostr_key,
        &crate::random_bytes::<32>(),
        now,
        KIND_MANIFEST,
        vec![vec!["d".into(), manifest_d_tag(root_pub)]],
        B64.encode(signed_manifest),
    )
}

/// A valid secp256k1 secret key (retries the ~2^-128 chance of an out-of-range value).
pub fn random_secp_key() -> [u8; 32] {
    loop {
        let k = crate::random_bytes::<32>();
        if k256::schnorr::SigningKey::from_slice(&k).is_ok() {
            return k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_key_is_symmetric_and_tags_differ_by_recipient_and_day() {
        let a = StaticSecret::from([1u8; 32]);
        let b = StaticSecret::from([2u8; 32]);
        let ka = pair_key(&a, PublicKey::from(&b).as_bytes()).unwrap();
        let kb = pair_key(&b, PublicKey::from(&a).as_bytes()).unwrap();
        let to_b = Id([0xbb; 16]);
        let to_a = Id([0xaa; 16]);
        assert_eq!(ka.tag(&to_b, 20000), kb.tag(&to_b, 20000));
        assert_ne!(ka.tag(&to_b, 20000), ka.tag(&to_a, 20000));
        assert_ne!(ka.tag(&to_b, 20000), ka.tag(&to_b, 20001));
        assert!(pair_key(&a, &[0u8; 32]).is_none());
    }

    /// BIP-340 test vector 0: the signatures relays check are standard Schnorr.
    #[test]
    fn bip340_vector() {
        let mut sk = [0u8; 32];
        sk[31] = 3;
        let key = k256::schnorr::SigningKey::from_slice(&sk).unwrap();
        assert_eq!(
            hex::encode(key.verifying_key().to_bytes()),
            "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"
        );
        let sig = key.sign_raw(&[0u8; 32], &[0u8; 32]).unwrap();
        assert_eq!(
            hex::encode(sig.to_bytes()),
            "e907831f80848d1069a5371b402410364bdf1c5f8307b0084c55f1ce2dca821525f66a4a85ea8b71e482a74f382d2ce5ebeee8fdb2172f477df4900d310536c0"
        );
    }

    #[test]
    fn events_sign_verify_and_roundtrip() {
        let ev = signal_event(b"envelope", "abcd", 1_700_000_000).unwrap();
        ev.verify().unwrap();
        let back = Event::from_json(&ev.to_json()).unwrap();
        assert_eq!(back, ev);
        assert_eq!(back.payload().unwrap(), b"envelope");
        assert_eq!(back.tag("t"), Some("abcd"));
        let mut bad = ev.clone();
        bad.content = B64.encode(b"other");
        assert!(bad.verify().is_err());
    }
}
