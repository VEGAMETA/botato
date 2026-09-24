//! Enrollment: a device asks the admin for a certificate.
//!
//! request body = u8 format(1) | id device_id | [32] sign_pub | [32] enc_pub | str name (may be empty)
//! request      = body || Ed25519(device, ENROLL || body)      (proof of possession of sign_pub)
//!
//! Text forms (for QR codes and copy-paste): `botato-req1:` / `botato-cert1:` + base64url without padding.
//!
//! The request travels over an untrusted channel, so the admin and the user compare
//! the fingerprint out of band (voice or in person) before a cert is issued.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::codec::{Reader, Writer, valid_str};
use crate::{Error, Id, KEY_LEN, Result, SigningKey, ctx, sign, split_signed, verify};

const FORMAT: u8 = 1;
pub const REQUEST_PREFIX: &str = "botato-req1:";
pub const CERT_PREFIX: &str = "botato-cert1:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub device_id: Id,
    pub sign_pub: [u8; KEY_LEN],
    pub enc_pub: [u8; KEY_LEN],
    /// Suggested display name; the admin decides.
    pub name: String,
}

impl Request {
    fn validate(&self) -> Result<()> {
        let ok = !self.device_id.is_zero()
            && valid_str(&self.name, true)
            && self.name.len() <= crate::cert::MAX_NAME;
        if ok { Ok(()) } else { Err(Error::Invalid) }
    }

    fn encode_body(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut w = Writer::default();
        w.u8(FORMAT);
        w.id(&self.device_id);
        w.raw(&self.sign_pub);
        w.raw(&self.enc_pub);
        w.str(&self.name);
        Ok(w.0)
    }

    /// `device_key` must be the key whose public part is `sign_pub`.
    pub fn sign(&self, device_key: &SigningKey) -> Result<Vec<u8>> {
        if device_key.verifying_key().to_bytes() != self.sign_pub {
            return Err(Error::Invalid);
        }
        let mut b = self.encode_body()?;
        let sig = sign(device_key, ctx::ENROLL, &[&b]);
        b.extend_from_slice(&sig);
        Ok(b)
    }

    pub fn verify(signed: &[u8]) -> Result<Request> {
        let (body, sig) = split_signed(signed)?;
        let mut r = Reader(body);
        if r.u8()? != FORMAT {
            return Err(Error::Format);
        }
        let req = Request {
            device_id: r.id()?,
            sign_pub: r.fixed()?,
            enc_pub: r.fixed()?,
            name: r.str(true)?,
        };
        r.finish()?;
        req.validate()?;
        verify(&req.sign_pub, ctx::ENROLL, &[body], sig)?;
        Ok(req)
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.device_id, &self.sign_pub, &self.enc_pub)
    }
}

/// 20 decimal digits in 4 groups (~66 bits), derived from all device key material.
pub fn fingerprint(device_id: &Id, sign_pub: &[u8; KEY_LEN], enc_pub: &[u8; KEY_LEN]) -> String {
    let h = Sha256::new()
        .chain_update(ctx::FINGERPRINT)
        .chain_update(device_id.0)
        .chain_update(sign_pub)
        .chain_update(enc_pub)
        .finalize();
    h.chunks(5)
        .take(4)
        .map(|c| {
            let v = c.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64);
            format!("{:05}", v % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn to_text(prefix: &str, bytes: &[u8]) -> String {
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Accepts surrounding whitespace and line breaks (copy-paste from chats).
pub fn from_text(prefix: &str, s: &str) -> Result<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let rest = s.strip_prefix(prefix).ok_or(Error::Format)?;
    URL_SAFE_NO_PAD.decode(rest).map_err(|_| Error::Invalid)
}
