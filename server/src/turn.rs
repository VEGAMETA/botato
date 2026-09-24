//! coturn `use-auth-secret` (TURN REST API) credentials.

use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

#[derive(Clone)]
pub struct TurnConfig {
    pub secret: Vec<u8>,
    pub urls: Vec<String>,
    pub ttl_secs: u64,
}

pub struct Creds {
    pub username: String,
    pub credential: String,
}

impl TurnConfig {
    /// The username is only the expiry timestamp: coturn needs nothing else,
    /// and it keeps user ids out of TURN.
    pub fn issue(&self, now: u64) -> Creds {
        let username = (now + self.ttl_secs).to_string();
        let mut mac =
            Hmac::<Sha1>::new_from_slice(&self.secret).expect("hmac accepts any key length");
        mac.update(username.as_bytes());
        Creds {
            username,
            credential: STANDARD.encode(mac.finalize().into_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_coturn_formula() {
        // printf 1000 | openssl dgst -sha1 -hmac secret -binary | base64
        let t = TurnConfig {
            secret: b"secret".to_vec(),
            urls: vec![],
            ttl_secs: 1000,
        };
        let c = t.issue(0);
        assert_eq!(c.username, "1000");
        assert_eq!(c.credential, "7hm3prHy1HDk8BqmbHgqJ94SrkI=");
    }
}
