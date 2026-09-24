use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Random 128-bit identifier: user, device or message. Lowercase hex in JSON.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub [u8; 16]);

impl Id {
    pub fn random() -> Self {
        Id(crate::random_bytes())
    }
    pub fn is_zero(&self) -> bool {
        self.0 == [0; 16]
    }
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    pub fn from_hex(s: &str) -> Option<Self> {
        let mut b = [0u8; 16];
        // Only canonical lowercase hex, so every id has exactly one encoding.
        if s.len() != 32 || s.bytes().any(|c| c.is_ascii_uppercase()) {
            return None;
        }
        hex::decode_to_slice(s, &mut b).ok()?;
        Some(Id(b))
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.to_hex())
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        Id::from_hex(&s).ok_or_else(|| serde::de::Error::custom("bad id"))
    }
}
