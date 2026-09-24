//! Password-encrypted root key file.
//!
//! file = "botato-root1\n" | u32 m_cost_kib | u32 t_cost | u32 p_cost | [16] salt | [24] nonce
//!      | XChaCha20-Poly1305(key = Argon2id(password, salt), seed[32]), AAD = everything before the ciphertext

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::Aead;
use chacha20poly1305::aead::array::Array;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use proto::SigningKey;
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"botato-root1\n";
const HEADER_LEN: usize = MAGIC.len() + 12 + 16 + 24;

#[derive(Clone, Copy, Debug)]
pub struct Kdf {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Kdf {
    /// ~256 MiB, a second or two on a laptop. Used for real keys.
    pub const STRONG: Kdf = Kdf {
        m_cost_kib: 256 * 1024,
        t_cost: 3,
        p_cost: 1,
    };
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Format,
    /// Wrong password or corrupted file.
    Decrypt,
}

fn derive(password: &[u8], salt: &[u8], kdf: Kdf) -> Result<Zeroizing<[u8; 32]>, Error> {
    let params =
        Params::new(kdf.m_cost_kib, kdf.t_cost, kdf.p_cost, Some(32)).map_err(|_| Error::Format)?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password, salt, out.as_mut())
        .map_err(|_| Error::Format)?;
    Ok(out)
}

pub fn seal(key: &SigningKey, password: &[u8], kdf: Kdf) -> Vec<u8> {
    let salt: [u8; 16] = proto::random_bytes();
    let nonce: [u8; 24] = proto::random_bytes();
    let mut out = MAGIC.to_vec();
    for v in [kdf.m_cost_kib, kdf.t_cost, kdf.p_cost] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    let k = derive(password, &salt, kdf).expect("valid kdf params");
    let cipher = XChaCha20Poly1305::new(&Array::from(*k));
    let seed = Zeroizing::new(key.to_bytes());
    let ct = cipher
        .encrypt(
            &XNonce::from(nonce),
            chacha20poly1305::aead::Payload {
                msg: seed.as_ref(),
                aad: &out,
            },
        )
        .expect("encryption cannot fail");
    out.extend_from_slice(&ct);
    out
}

pub fn open(file: &[u8], password: &[u8]) -> Result<SigningKey, Error> {
    if file.len() != HEADER_LEN + 32 + 16 || !file.starts_with(MAGIC) {
        return Err(Error::Format);
    }
    let (header, ct) = file.split_at(HEADER_LEN);
    let u32_at = |i: usize| {
        u32::from_be_bytes(
            header[MAGIC.len() + i * 4..MAGIC.len() + i * 4 + 4]
                .try_into()
                .unwrap(),
        )
    };
    let kdf = Kdf {
        m_cost_kib: u32_at(0),
        t_cost: u32_at(1),
        p_cost: u32_at(2),
    };
    // Params are read before authentication: bound them so a corrupt file can't demand 16 GiB.
    if kdf.m_cost_kib > 1024 * 1024 || kdf.t_cost > 16 || kdf.p_cost > 8 {
        return Err(Error::Format);
    }
    let salt = &header[MAGIC.len() + 12..MAGIC.len() + 28];
    let nonce: [u8; 24] = header[MAGIC.len() + 28..].try_into().unwrap();
    let k = derive(password, salt, kdf)?;
    let cipher = XChaCha20Poly1305::new(&Array::from(*k));
    let seed = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                chacha20poly1305::aead::Payload {
                    msg: ct,
                    aad: header,
                },
            )
            .map_err(|_| Error::Decrypt)?,
    );
    let seed: [u8; 32] = seed.as_slice().try_into().map_err(|_| Error::Format)?;
    Ok(SigningKey::from_bytes(&seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: Kdf = Kdf {
        m_cost_kib: 64,
        t_cost: 1,
        p_cost: 1,
    };

    #[test]
    fn roundtrip_and_wrong_password() {
        let k = proto::generate_signing_key();
        let f = seal(&k, b"correct horse", FAST);
        assert_eq!(open(&f, b"correct horse").unwrap().to_bytes(), k.to_bytes());
        assert_eq!(open(&f, b"wrong").unwrap_err(), Error::Decrypt);
        let mut tampered = f.clone();
        tampered[MAGIC.len() + 3] ^= 1; // KDF params are authenticated too (m_cost 64 -> 65)
        assert_eq!(
            open(&tampered, b"correct horse").unwrap_err(),
            Error::Decrypt
        );
        let mut huge = f.clone();
        huge[MAGIC.len()] ^= 1; // m_cost would be 16 GiB
        assert_eq!(open(&huge, b"correct horse").unwrap_err(), Error::Format);
    }
}
