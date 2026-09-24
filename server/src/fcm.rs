//! FCM HTTP v1 wake-up pushes. Every push is identical (same data, same TTL, high priority):
//! Google learns only that *some* wake-up was sent to a token, not whether it is a call or a
//! message, and never any content. The app wakes, connects to the relay and fetches the rest.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::push::{PushKind, PushToken, Pusher};

const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// Long enough for a phone in Doze to pick it up; the relay holds call signals only 60 s anyway.
const TTL: &str = "600s";

#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

struct Inner {
    project: String,
    email: String,
    token_uri: String,
    key: EncodingKey,
    http: reqwest::Client,
    access: tokio::sync::Mutex<Option<(String, Instant)>>,
}

#[derive(Clone)]
pub struct FcmPusher(Arc<Inner>);

impl FcmPusher {
    pub fn from_json(json: &[u8]) -> Result<Self, String> {
        let sa: ServiceAccount = serde_json::from_slice(json).map_err(|e| format!("service account: {e}"))?;
        let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes()).map_err(|e| format!("service account key: {e}"))?;
        // ring (pure Rust + asm) rather than aws-lc: keeps the static musl build simple.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(FcmPusher(Arc::new(Inner {
            project: sa.project_id,
            email: sa.client_email,
            token_uri: sa.token_uri,
            key,
            http,
            access: tokio::sync::Mutex::new(None),
        })))
    }

    fn assertion(&self, now: u64) -> Result<String, String> {
        let claims = Claims { iss: &self.0.email, scope: SCOPE, aud: &self.0.token_uri, iat: now, exp: now + 3600 };
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.0.key).map_err(|e| format!("jwt: {e}"))
    }

    async fn access_token(&self) -> Result<String, String> {
        let mut cached = self.0.access.lock().await;
        if let Some((token, until)) = cached.as_ref() {
            if Instant::now() + Duration::from_secs(300) < *until {
                return Ok(token.clone());
            }
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let assertion = self.assertion(now)?;
        let resp = self
            .0
            .http
            .post(&self.0.token_uri)
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", assertion.as_str())])
            .send()
            .await
            .map_err(|e| format!("oauth: {}", e.without_url()))?;
        if !resp.status().is_success() {
            return Err(format!("oauth: HTTP {}", resp.status()));
        }
        let t: TokenResponse = resp.json().await.map_err(|e| format!("oauth: {}", e.without_url()))?;
        *cached = Some((t.access_token.clone(), Instant::now() + Duration::from_secs(t.expires_in)));
        Ok(t.access_token)
    }

    /// Obtains an OAuth access token: proves the service account works (used at startup).
    pub async fn check(&self) -> Result<(), String> {
        self.access_token().await.map(|_| ())
    }

    async fn send(&self, device_token: &str) -> Result<(), String> {
        let access = self.access_token().await?;
        let body = serde_json::json!({
            "message": {
                "token": device_token,
                "data": { "w": "1" },
                "android": { "priority": "HIGH", "ttl": TTL },
            }
        });
        let url = format!("https://fcm.googleapis.com/v1/projects/{}/messages:send", self.0.project);
        let resp = self
            .0
            .http
            .post(url)
            .bearer_auth(access)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("send: {}", e.without_url()))?;
        if !resp.status().is_success() {
            // Never log the token or the response body (it echoes the token).
            return Err(format!("send: HTTP {}", resp.status()));
        }
        Ok(())
    }
}

impl Pusher for FcmPusher {
    fn push(&self, token: &PushToken, _kind: PushKind) {
        if token.platform != proto::frames::Platform::Fcm {
            return;
        }
        let this = self.clone();
        let device_token = token.token.clone();
        tokio::spawn(async move {
            if let Err(e) = this.send(&device_token).await {
                eprintln!("push: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Throwaway 2048-bit key generated for this test only.
    const TEST_KEY: &str = include_str!("../tests/data/test_rsa_key.pem");

    #[test]
    fn builds_a_signed_assertion() {
        let json = serde_json::json!({
            "type": "service_account",
            "project_id": "p",
            "client_email": "svc@p.iam.gserviceaccount.com",
            "private_key": TEST_KEY,
            "token_uri": "https://oauth2.googleapis.com/token",
        });
        let p = FcmPusher::from_json(json.to_string().as_bytes()).unwrap();
        let jwt = p.assertion(1_800_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        use base64::Engine;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&claims).unwrap();
        assert_eq!(v["iss"], "svc@p.iam.gserviceaccount.com");
        assert_eq!(v["scope"], SCOPE);
        assert_eq!(v["exp"], 1_800_003_600u64);
        assert!(FcmPusher::from_json(b"{}").is_err());
    }
}
