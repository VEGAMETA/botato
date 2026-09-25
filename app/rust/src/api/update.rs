//! App updates (protocol/README.md §3b): the release list comes from the relay host but is
//! trusted only through the admin signature; a downloaded file is installed only if its
//! length and SHA-256 match that list.

use proto::release::Release;
use sha2::{Digest, Sha256};

use crate::client::load_manifest;
use crate::config;
use crate::lang::tr;

pub struct AppVersion {
    pub version: String,
    pub build: u64,
}

pub struct UpdateInfo {
    pub version: String,
    pub build: u64,
    /// Where to download the file from.
    pub url: String,
    pub file: String,
    pub size: u64,
    /// Hex; pass back to `update_verify`.
    pub sha256: String,
}

#[flutter_rust_bridge::frb(sync)]
pub fn app_version() -> AppVersion {
    AppVersion { version: config::APP_VERSION.into(), build: config::app_build() }
}

/// `https://<relay host>/releases/` for every server in the manifest.
#[flutter_rust_bridge::frb(sync)]
pub fn update_bases() -> Vec<String> {
    let Ok(m) = load_manifest() else { return vec![] };
    m.servers
        .iter()
        .filter_map(|s| {
            let rest = s.url.strip_prefix("wss://")?;
            let host = rest.split('/').next()?;
            Some(format!("https://{host}/releases/"))
        })
        .collect()
}

/// Checks a downloaded `release.bin`: admin signature, a newer build, a file for this platform.
/// `None` = nothing to install.
#[flutter_rust_bridge::frb(sync)]
pub fn update_check(signed: Vec<u8>, base: String) -> anyhow::Result<Option<UpdateInfo>> {
    let rel = Release::verify(&config::root_pub(), &signed).map_err(|e| anyhow::anyhow!("{}: {e}", tr("релиз не подписан админом", "the release is not signed by the admin")))?;
    if rel.build <= config::app_build() {
        return Ok(None);
    }
    let Some(platform) = config::release_platform() else { return Ok(None) };
    let Some(f) = rel.file_for(platform) else { return Ok(None) };
    Ok(Some(UpdateInfo {
        version: rel.version.clone(),
        build: rel.build,
        url: format!("{base}{}", f.file),
        file: f.file.clone(),
        size: f.size,
        sha256: hex::encode(f.sha256),
    }))
}

/// True if the file at `path` is exactly the signed one.
pub fn update_verify(path: String, size: u64, sha256: String) -> anyhow::Result<bool> {
    let data = std::fs::read(&path)?;
    if data.len() as u64 != size {
        return Ok(false);
    }
    Ok(hex::encode(Sha256::digest(&data)) == sha256)
}
