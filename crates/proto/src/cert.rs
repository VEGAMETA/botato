//! User certificate: binds a user to a set of device keys; signed by the admin root key.
//!
//! body = u8 format(1) | id user_id | str name | u64 issued_at | u64 expires_at
//!      | u8 n_devices | n × (id device_id | [32] sign_pub | [32] enc_pub)

use crate::codec::{Reader, Writer, valid_str};
use crate::{Error, Id, KEY_LEN, Result, SigningKey, ctx, sign, split_signed, verify};

const FORMAT: u8 = 1;
pub const MAX_NAME: usize = 64;
pub const MAX_DEVICES: usize = 8;
/// Tolerated clock difference for `issued_at`.
pub const CLOCK_SKEW_SECS: u64 = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub device_id: Id,
    /// Ed25519
    pub sign_pub: [u8; KEY_LEN],
    /// X25519
    pub enc_pub: [u8; KEY_LEN],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cert {
    pub user_id: Id,
    pub name: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub devices: Vec<Device>,
}

impl Cert {
    fn validate(&self) -> Result<()> {
        let ok = !self.user_id.is_zero()
            && valid_str(&self.name, false)
            && self.name.len() <= MAX_NAME
            && self.expires_at > self.issued_at
            && (1..=MAX_DEVICES).contains(&self.devices.len())
            && self.devices.iter().enumerate().all(|(i, d)| {
                !d.device_id.is_zero()
                    && d.device_id != self.user_id
                    && self.devices[..i].iter().all(|o| o.device_id != d.device_id)
            });
        if ok { Ok(()) } else { Err(Error::Invalid) }
    }

    pub fn encode_body(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut w = Writer::default();
        w.u8(FORMAT);
        w.id(&self.user_id);
        w.str(&self.name);
        w.u64(self.issued_at);
        w.u64(self.expires_at);
        w.u8(self.devices.len() as u8);
        for d in &self.devices {
            w.id(&d.device_id);
            w.raw(&d.sign_pub);
            w.raw(&d.enc_pub);
        }
        Ok(w.0)
    }

    /// Returns the signed certificate: `body || sig`.
    pub fn sign(&self, root: &SigningKey) -> Result<Vec<u8>> {
        let mut b = self.encode_body()?;
        let sig = sign(root, ctx::CERT, &[&b]);
        b.extend_from_slice(&sig);
        Ok(b)
    }

    pub fn device(&self, id: &Id) -> Option<&Device> {
        self.devices.iter().find(|d| d.device_id == *id)
    }

    fn decode_body(body: &[u8]) -> Result<Cert> {
        let mut r = Reader(body);
        if r.u8()? != FORMAT {
            return Err(Error::Format);
        }
        let user_id = r.id()?;
        let name = r.str(false)?;
        let issued_at = r.u64()?;
        let expires_at = r.u64()?;
        let n = r.u8()? as usize;
        let mut devices = Vec::with_capacity(n.min(MAX_DEVICES));
        for _ in 0..n {
            devices.push(Device {
                device_id: r.id()?,
                sign_pub: r.fixed()?,
                enc_pub: r.fixed()?,
            });
        }
        r.finish()?;
        let c = Cert {
            user_id,
            name,
            issued_at,
            expires_at,
            devices,
        };
        c.validate()?;
        Ok(c)
    }

    /// Checks the admin signature and the validity period at `now` (unix seconds).
    pub fn verify(root_pub: &[u8], signed: &[u8], now: u64) -> Result<Cert> {
        let (body, sig) = split_signed(signed)?;
        verify(root_pub, ctx::CERT, &[body], sig)?;
        let c = Self::decode_body(body)?;
        if c.issued_at > now.saturating_add(CLOCK_SKEW_SECS) {
            return Err(Error::NotYetValid);
        }
        if c.expires_at <= now {
            return Err(Error::Expired);
        }
        Ok(c)
    }
}
