//! botato wire formats. The normative spec is `protocol/README.md`.
//!
//! Everything signed is a canonical binary encoding: `sig = Ed25519(ctx || body)`,
//! signed object = `body || sig`. JSON is only the WebSocket transport envelope.

pub mod auth;
pub mod cert;
pub mod device;
pub mod envelope;
mod codec;
pub mod enroll;
pub mod file;
pub mod frames;
pub mod id;
pub mod manifest;
pub mod release;

pub use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
pub use id::Id;

/// Domain-separation contexts, prepended to every signed body.
pub mod ctx {
    pub const CERT: &[u8] = b"botato/v1/cert\0";
    pub const MANIFEST: &[u8] = b"botato/v1/manifest\0";
    pub const AUTH_SERVER: &[u8] = b"botato/v1/auth-server\0";
    pub const AUTH_CLIENT: &[u8] = b"botato/v1/auth-client\0";
    pub const ENROLL: &[u8] = b"botato/v1/enroll\0";
    pub const ENVELOPE: &[u8] = b"botato/v1/envelope\0";
    pub const RELEASE: &[u8] = b"botato/v1/release\0";
    /// AAD prefix of attachment encryption, not a signature context.
    pub const FILE: &[u8] = b"botato/v1/file\0";
    /// Hash prefix, not a signature context.
    pub const FINGERPRINT: &[u8] = b"botato/v1/fingerprint\0";
}

pub const SIG_LEN: usize = 64;
pub const KEY_LEN: usize = 32;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("truncated input")]
    Truncated,
    #[error("trailing bytes")]
    Trailing,
    #[error("unsupported format")]
    Format,
    #[error("invalid field")]
    Invalid,
    #[error("bad signature")]
    Signature,
    #[error("certificate expired")]
    Expired,
    #[error("certificate not yet valid")]
    NotYetValid,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Fills `buf` from the OS CSPRNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS RNG unavailable");
    b
}

pub fn generate_signing_key() -> SigningKey {
    SigningKey::from_bytes(&random_bytes::<32>())
}

pub(crate) fn sign(key: &SigningKey, ctx: &[u8], parts: &[&[u8]]) -> [u8; SIG_LEN] {
    use ed25519_dalek::Signer;
    key.sign(&concat(ctx, parts)).to_bytes()
}

pub(crate) fn verify(key: &[u8], ctx: &[u8], parts: &[&[u8]], sig: &[u8]) -> Result<()> {
    let key: &[u8; KEY_LEN] = key.try_into().map_err(|_| Error::Invalid)?;
    let sig: &[u8; SIG_LEN] = sig.try_into().map_err(|_| Error::Signature)?;
    let vk = VerifyingKey::from_bytes(key).map_err(|_| Error::Invalid)?;
    vk.verify_strict(&concat(ctx, parts), &Signature::from_bytes(sig))
        .map_err(|_| Error::Signature)
}

fn concat(ctx: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut m = Vec::with_capacity(ctx.len() + parts.iter().map(|p| p.len()).sum::<usize>());
    m.extend_from_slice(ctx);
    for p in parts {
        m.extend_from_slice(p);
    }
    m
}

/// Splits a signed object into `(body, sig)`.
pub(crate) fn split_signed(b: &[u8]) -> Result<(&[u8], &[u8])> {
    if b.len() < SIG_LEN {
        return Err(Error::Truncated);
    }
    Ok(b.split_at(b.len() - SIG_LEN))
}
