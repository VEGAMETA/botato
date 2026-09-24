//! Admin-signed list of servers, fallback channels and revocations.
//!
//! body = u8 format(1) | u64 version | u64 issued_at
//!      | u8 n_servers | n × (str url | str ip_fallback | [32] server_pub)
//!      | u8 n_stun | n × str | u8 n_nostr | n × str
//!      | u16 n_revoked | n × id

use crate::codec::{Reader, Writer, valid_str};
use crate::{Error, Id, KEY_LEN, Result, SigningKey, ctx, sign, split_signed, verify};

const FORMAT: u8 = 1;
pub const MAX_SERVERS: usize = 16;
pub const MAX_LIST: usize = 64;
pub const MAX_REVOKED: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEntry {
    /// e.g. `wss://relay.example.org/ws`
    pub url: String,
    /// May be empty.
    pub ip_fallback: String,
    pub server_pub: [u8; KEY_LEN],
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Manifest {
    pub version: u64,
    pub issued_at: u64,
    pub servers: Vec<ServerEntry>,
    pub stun: Vec<String>,
    pub nostr_relays: Vec<String>,
    /// User ids and device ids.
    pub revoked: Vec<Id>,
}

impl Manifest {
    fn validate(&self) -> Result<()> {
        let ok = (1..=MAX_SERVERS).contains(&self.servers.len())
            && self
                .servers
                .iter()
                .all(|s| valid_str(&s.url, false) && valid_str(&s.ip_fallback, true))
            && [&self.stun, &self.nostr_relays]
                .iter()
                .all(|l| l.len() <= MAX_LIST && l.iter().all(|s| valid_str(s, false)))
            && self.revoked.len() <= MAX_REVOKED;
        if ok { Ok(()) } else { Err(Error::Invalid) }
    }

    pub fn encode_body(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut w = Writer::default();
        w.u8(FORMAT);
        w.u64(self.version);
        w.u64(self.issued_at);
        w.u8(self.servers.len() as u8);
        for s in &self.servers {
            w.str(&s.url);
            w.str(&s.ip_fallback);
            w.raw(&s.server_pub);
        }
        for l in [&self.stun, &self.nostr_relays] {
            w.u8(l.len() as u8);
            for s in l {
                w.str(s);
            }
        }
        w.u16(self.revoked.len() as u16);
        for id in &self.revoked {
            w.id(id);
        }
        Ok(w.0)
    }

    pub fn sign(&self, root: &SigningKey) -> Result<Vec<u8>> {
        let mut b = self.encode_body()?;
        let sig = sign(root, ctx::MANIFEST, &[&b]);
        b.extend_from_slice(&sig);
        Ok(b)
    }

    fn decode_body(body: &[u8]) -> Result<Manifest> {
        let mut r = Reader(body);
        if r.u8()? != FORMAT {
            return Err(Error::Format);
        }
        let mut m = Manifest {
            version: r.u64()?,
            issued_at: r.u64()?,
            ..Default::default()
        };
        for _ in 0..r.u8()? {
            m.servers.push(ServerEntry {
                url: r.str(false)?,
                ip_fallback: r.str(true)?,
                server_pub: r.fixed()?,
            });
        }
        for _ in 0..r.u8()? {
            m.stun.push(r.str(false)?);
        }
        for _ in 0..r.u8()? {
            m.nostr_relays.push(r.str(false)?);
        }
        for _ in 0..r.u16()? {
            m.revoked.push(r.id()?);
        }
        r.finish()?;
        m.validate()?;
        Ok(m)
    }

    pub fn verify(root_pub: &[u8], signed: &[u8]) -> Result<Manifest> {
        let (body, sig) = split_signed(signed)?;
        verify(root_pub, ctx::MANIFEST, &[body], sig)?;
        Self::decode_body(body)
    }

    pub fn is_revoked(&self, id: &Id) -> bool {
        self.revoked.contains(id)
    }

    pub fn has_server(&self, server_pub: &[u8]) -> bool {
        self.servers.iter().any(|s| s.server_pub == server_pub)
    }
}
