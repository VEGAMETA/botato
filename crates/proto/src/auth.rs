//! Mutual authentication handshake over WebSocket.
//!
//! ```text
//! C->S hello     {v, nonce: cn}
//! S->C challenge {nonce: sn, server_pub, sig: Sign(server, AUTH_SERVER || cn || sn)}
//! C->S auth      {cert, device_id, sig: Sign(device, AUTH_CLIENT || sn || server_pub || cn)}
//! S->C ready     {user_id, device_id}
//! ```

use crate::{KEY_LEN, Result, SIG_LEN, SigningKey, ctx, sign, verify};

pub const NONCE_LEN: usize = 32;
pub type Nonce = [u8; NONCE_LEN];

pub fn sign_server(key: &SigningKey, client_nonce: &Nonce, server_nonce: &Nonce) -> [u8; SIG_LEN] {
    sign(key, ctx::AUTH_SERVER, &[client_nonce, server_nonce])
}

pub fn verify_server(
    server_pub: &[u8],
    client_nonce: &Nonce,
    server_nonce: &Nonce,
    sig: &[u8],
) -> Result<()> {
    verify(
        server_pub,
        ctx::AUTH_SERVER,
        &[client_nonce, server_nonce],
        sig,
    )
}

pub fn sign_client(
    device_key: &SigningKey,
    server_nonce: &Nonce,
    server_pub: &[u8; KEY_LEN],
    client_nonce: &Nonce,
) -> [u8; SIG_LEN] {
    sign(
        device_key,
        ctx::AUTH_CLIENT,
        &[server_nonce, server_pub, client_nonce],
    )
}

pub fn verify_client(
    device_pub: &[u8],
    server_nonce: &Nonce,
    server_pub: &[u8; KEY_LEN],
    client_nonce: &Nonce,
    sig: &[u8],
) -> Result<()> {
    verify(
        device_pub,
        ctx::AUTH_CLIENT,
        &[server_nonce, server_pub, client_nonce],
        sig,
    )
}
