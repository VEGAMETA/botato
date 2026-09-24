//! Trust anchors compiled into the client.

/// Admin root public key: the only key that can issue certificates and manifests.
pub fn root_pub() -> [u8; 32] {
    let hex = include_str!("../../assets/root_pub.hex").trim();
    hex::decode(hex).ok().and_then(|b| b.try_into().ok()).expect("assets/root_pub.hex: 32 bytes hex")
}

/// Signed manifest shipped with the build; may be empty.
pub const BUILTIN_MANIFEST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/manifest.bin"));

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
