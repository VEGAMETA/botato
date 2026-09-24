//! WebSocket frames: JSON text messages tagged by `t`.
//! Binary fields are standard base64 (padded); ids are lowercase hex.
//! Frames are not signed — anything needing integrity is signed inside blobs.

use serde::{Deserialize, Serialize};

use crate::Id;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_BLOB: usize = 64 * 1024;
pub const MAX_PUSH_TOKEN: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Hello {
        v: u32,
        #[serde(with = "b64")]
        nonce: Vec<u8>,
    },
    Auth {
        #[serde(with = "b64")]
        cert: Vec<u8>,
        device_id: Id,
        #[serde(with = "b64")]
        sig: Vec<u8>,
    },
    Send {
        to: Id,
        id: Id,
        #[serde(with = "b64")]
        blob: Vec<u8>,
    },
    Ack {
        id: Id,
    },
    Signal {
        to: Id,
        #[serde(with = "b64")]
        blob: Vec<u8>,
    },
    Directory {},
    Turn {},
    PushToken {
        platform: Platform,
        token: String,
    },
    /// Stores an encrypted attachment (`file::` blob) for 24 h. Same id again is a no-op.
    Upload {
        id: Id,
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    Download {
        id: Id,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame {
    Challenge {
        #[serde(with = "b64")]
        nonce: Vec<u8>,
        #[serde(with = "b64")]
        server_pub: Vec<u8>,
        #[serde(with = "b64")]
        sig: Vec<u8>,
    },
    Ready {
        user_id: Id,
        device_id: Id,
    },
    Stored {
        id: Id,
    },
    Deliver {
        id: Id,
        from: Id,
        #[serde(with = "b64")]
        blob: Vec<u8>,
    },
    Signal {
        from: Id,
        #[serde(with = "b64")]
        blob: Vec<u8>,
    },
    Signaled {
        to: Id,
        online: bool,
    },
    Directory {
        #[serde(with = "b64_list")]
        certs: Vec<Vec<u8>>,
    },
    Turn {
        urls: Vec<String>,
        username: String,
        credential: String,
        ttl: u64,
    },
    Uploaded {
        id: Id,
    },
    File {
        id: Id,
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    Ok {
        r#ref: String,
    },
    Error {
        r#ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<Id>,
        code: ErrorCode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Fcm,
    ApnsVoip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadFrame,
    TooLarge,
    Quota,
    RateLimited,
    Revoked,
    Unavailable,
    Internal,
    /// `download` of an unknown or expired file.
    NotFound,
}

/// WebSocket close codes.
pub mod close {
    pub const PROTOCOL: u16 = 4000;
    pub const AUTH_FAILED: u16 = 4001;
    pub const REPLACED: u16 = 4002;
    pub const REVOKED: u16 = 4003;
    pub const AUTH_TIMEOUT: u16 = 4004;
}

mod b64 {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        STANDARD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

mod b64_list {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};

    pub fn serialize<S: Serializer>(v: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for b in v {
            seq.serialize_element(&STANDARD.encode(b))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {
        Vec::<String>::deserialize(d)?
            .into_iter()
            .map(|s| STANDARD.decode(s).map_err(serde::de::Error::custom))
            .collect()
    }
}
