//! Smoke test of the bridge: proves the Rust core (and `proto`) runs on each platform.

#[flutter_rust_bridge::frb(sync)]
pub fn core_info() -> String {
    // Throwaway key: exercises the OS RNG, Ed25519 and SHA-256 through `proto`.
    let key = proto::generate_signing_key();
    let fp = proto::enroll::fingerprint(&proto::Id::random(), &key.verifying_key().to_bytes(), &[0; 32]);
    format!("botato core, protocol v{}, sample fingerprint {fp}", proto::frames::PROTOCOL_VERSION)
}

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}
