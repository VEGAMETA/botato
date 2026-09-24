//! Device secret material. Generated on first launch, kept by the client in platform
//! secure storage, never leaves the device.
//!
//! blob = u8 format(1) | id device_id | [32] Ed25519 seed | [32] X25519 secret

use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::codec::{Reader, Writer};
use crate::enroll::{self, Request};
use crate::{Error, Id, KEY_LEN, Result, SigningKey};

const FORMAT: u8 = 1;

pub struct DeviceSecret {
    pub device_id: Id,
    sign: SigningKey,
    enc: StaticSecret,
}

impl DeviceSecret {
    pub fn generate() -> Self {
        DeviceSecret {
            device_id: Id::random(),
            sign: crate::generate_signing_key(),
            enc: StaticSecret::from(crate::random_bytes::<32>()),
        }
    }

    pub fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut w = Writer::default();
        w.u8(FORMAT);
        w.id(&self.device_id);
        w.raw(&self.sign.to_bytes());
        w.raw(self.enc.as_bytes());
        Zeroizing::new(w.0)
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let mut r = Reader(b);
        if r.u8()? != FORMAT {
            return Err(Error::Format);
        }
        let device_id = r.id()?;
        let seed = Zeroizing::new(r.fixed::<32>()?);
        let enc = Zeroizing::new(r.fixed::<32>()?);
        r.finish()?;
        if device_id.is_zero() {
            return Err(Error::Invalid);
        }
        Ok(DeviceSecret {
            device_id,
            sign: SigningKey::from_bytes(&seed),
            enc: StaticSecret::from(*enc),
        })
    }

    pub(crate) fn enc_secret(&self) -> &StaticSecret {
        &self.enc
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.sign
    }

    pub fn sign_pub(&self) -> [u8; KEY_LEN] {
        self.sign.verifying_key().to_bytes()
    }

    pub fn enc_pub(&self) -> [u8; KEY_LEN] {
        PublicKey::from(&self.enc).to_bytes()
    }

    pub fn fingerprint(&self) -> String {
        enroll::fingerprint(&self.device_id, &self.sign_pub(), &self.enc_pub())
    }

    /// Signed enrollment request in text form (`botato-req1:...`).
    pub fn enrollment_request(&self, name: &str) -> Result<String> {
        let req = Request {
            device_id: self.device_id,
            sign_pub: self.sign_pub(),
            enc_pub: self.enc_pub(),
            name: name.to_owned(),
        };
        Ok(enroll::to_text(enroll::REQUEST_PREFIX, &req.sign(&self.sign)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_request() {
        let d = DeviceSecret::generate();
        let d2 = DeviceSecret::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(d2.device_id, d.device_id);
        assert_eq!(d2.sign_pub(), d.sign_pub());
        assert_eq!(d2.enc_pub(), d.enc_pub());
        let text = d.enrollment_request("Алиса").unwrap();
        let req = Request::verify(&enroll::from_text(enroll::REQUEST_PREFIX, &text).unwrap()).unwrap();
        assert_eq!(req.fingerprint(), d.fingerprint());
        assert!(DeviceSecret::from_bytes(&[1; 10]).is_err());
    }
}
