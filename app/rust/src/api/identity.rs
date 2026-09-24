//! Device identity: keys, enrollment request, certificate import.
//! Secrets cross into Dart only as an opaque blob for platform secure storage.

use proto::cert::Cert;
use proto::device::DeviceSecret;
use proto::enroll;

use crate::config;

pub struct DeviceInfo {
    pub device_id: String,
    /// 20 digits, compared with the admin by voice.
    pub fingerprint: String,
}

pub struct CertInfo {
    pub user_id: String,
    pub name: String,
    pub expires_at: u64,
    pub devices: u32,
}

fn load(secret: &[u8]) -> anyhow::Result<DeviceSecret> {
    DeviceSecret::from_bytes(secret).map_err(|e| anyhow::anyhow!("device secret: {e}"))
}

/// New device keys, as a blob for secure storage.
#[flutter_rust_bridge::frb(sync)]
pub fn device_generate() -> Vec<u8> {
    DeviceSecret::generate().to_bytes().to_vec()
}

#[flutter_rust_bridge::frb(sync)]
pub fn device_info(secret: Vec<u8>) -> anyhow::Result<DeviceInfo> {
    let d = load(&secret)?;
    Ok(DeviceInfo { device_id: d.device_id.to_hex(), fingerprint: d.fingerprint() })
}

/// `botato-req1:...` text for the admin (shown as QR and copyable text).
#[flutter_rust_bridge::frb(sync)]
pub fn enrollment_request(secret: Vec<u8>, name: String) -> anyhow::Result<String> {
    let d = load(&secret)?;
    d.enrollment_request(name.trim()).map_err(|e| anyhow::anyhow!("request: {e}"))
}

/// Verifies a `botato-cert1:...` text: admin signature, validity, and that it lists
/// this very device with its keys. Returns the normalized text to store.
#[flutter_rust_bridge::frb(sync)]
pub fn cert_import(secret: Vec<u8>, text: String) -> anyhow::Result<(String, CertInfo)> {
    let d = load(&secret)?;
    let raw = enroll::from_text(enroll::CERT_PREFIX, &text).map_err(|_| anyhow::anyhow!("это не сертификат botato"))?;
    let c = Cert::verify(&config::root_pub(), &raw, config::now()).map_err(|e| anyhow::anyhow!("сертификат не принят: {e}"))?;
    let me = c.device(&d.device_id).ok_or_else(|| anyhow::anyhow!("сертификат выписан для другого устройства"))?;
    if me.sign_pub != d.sign_pub() || me.enc_pub != d.enc_pub() {
        return Err(anyhow::anyhow!("ключи в сертификате не совпадают с ключами устройства"));
    }
    Ok((enroll::to_text(enroll::CERT_PREFIX, &raw), cert_info(&c)))
}

fn cert_info(c: &Cert) -> CertInfo {
    CertInfo { user_id: c.user_id.to_hex(), name: c.name.clone(), expires_at: c.expires_at, devices: c.devices.len() as u32 }
}
