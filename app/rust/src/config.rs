//! Trust anchors compiled into the client.

/// Admin root public key: the only key that can issue certificates and manifests.
pub fn root_pub() -> [u8; 32] {
    let hex = include_str!("../../assets/root_pub.hex").trim();
    hex::decode(hex).ok().and_then(|b| b.try_into().ok()).expect("assets/root_pub.hex: 32 bytes hex")
}

/// Signed manifest shipped with the build; may be empty.
/// From pubspec.yaml at build time.
pub const APP_VERSION: &str = env!("APP_VERSION");
pub fn app_build() -> u64 {
    env!("APP_BUILD").parse().unwrap_or(0)
}

/// Release platform of this build (protocol/README.md §3b), if updates are offered for it.
pub fn release_platform() -> Option<&'static str> {
    if cfg!(all(target_os = "android", target_arch = "aarch64")) {
        Some(proto::release::ANDROID_ARM64)
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(proto::release::WINDOWS_X64)
    } else {
        None
    }
}

pub const BUILTIN_MANIFEST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/manifest.bin"));

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
