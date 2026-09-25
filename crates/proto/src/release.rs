//! Admin-signed description of an app release: which files to install and their hashes.
//! The files themselves come from the (untrusted) server; a client installs one only if
//! its SHA-256 matches this signed list.
//!
//! body = u8 format(1) | str version | u64 build | u64 issued_at
//!      | u8 n_files | n × (str platform | str file | u64 size | [32] sha256)

use crate::codec::{Reader, Writer, valid_str};
use crate::{Error, Result, SigningKey, ctx, sign, split_signed, verify};

const FORMAT: u8 = 1;
pub const MAX_FILES: usize = 8;

/// Platform names used in releases.
pub const ANDROID_ARM64: &str = "android-arm64";
pub const WINDOWS_X64: &str = "windows-x64";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseFile {
    /// e.g. `android-arm64`, `windows-x64`.
    pub platform: String,
    /// File name next to the release on the server (no path).
    pub file: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Release {
    /// Human version, e.g. `0.1.4`.
    pub version: String,
    /// Monotonic build number; a client updates only to a higher one.
    pub build: u64,
    pub issued_at: u64,
    pub files: Vec<ReleaseFile>,
}

fn valid_file_name(s: &str) -> bool {
    valid_str(s, false) && !s.contains(['/', '\\']) && s != "." && s != ".."
}

impl Release {
    fn validate(&self) -> Result<()> {
        let ok = valid_str(&self.version, false)
            && (1..=MAX_FILES).contains(&self.files.len())
            && self.files.iter().all(|f| valid_str(&f.platform, false) && valid_file_name(&f.file));
        if ok { Ok(()) } else { Err(Error::Invalid) }
    }

    pub fn encode_body(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut w = Writer::default();
        w.u8(FORMAT);
        w.str(&self.version);
        w.u64(self.build);
        w.u64(self.issued_at);
        w.u8(self.files.len() as u8);
        for f in &self.files {
            w.str(&f.platform);
            w.str(&f.file);
            w.u64(f.size);
            w.raw(&f.sha256);
        }
        Ok(w.0)
    }

    pub fn sign(&self, root: &SigningKey) -> Result<Vec<u8>> {
        let mut b = self.encode_body()?;
        let sig = sign(root, ctx::RELEASE, &[&b]);
        b.extend_from_slice(&sig);
        Ok(b)
    }

    fn decode_body(body: &[u8]) -> Result<Release> {
        let mut r = Reader(body);
        if r.u8()? != FORMAT {
            return Err(Error::Format);
        }
        let mut rel = Release { version: r.str(false)?, build: r.u64()?, issued_at: r.u64()?, files: vec![] };
        for _ in 0..r.u8()? {
            rel.files.push(ReleaseFile { platform: r.str(false)?, file: r.str(false)?, size: r.u64()?, sha256: r.fixed()? });
        }
        r.finish()?;
        rel.validate()?;
        Ok(rel)
    }

    pub fn verify(root_pub: &[u8], signed: &[u8]) -> Result<Release> {
        let (body, sig) = split_signed(signed)?;
        verify(root_pub, ctx::RELEASE, &[body], sig)?;
        Self::decode_body(body)
    }

    pub fn file_for(&self, platform: &str) -> Option<&ReleaseFile> {
        self.files.iter().find(|f| f.platform == platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Release {
        Release {
            version: "0.1.4".into(),
            build: 4,
            issued_at: 1_800_000_000,
            files: vec![ReleaseFile { platform: ANDROID_ARM64.into(), file: "botato-0.1.4.apk".into(), size: 3, sha256: [1; 32] }],
        }
    }

    #[test]
    fn roundtrip_and_rejects() {
        let root = SigningKey::from_bytes(&[9; 32]);
        let signed = sample().sign(&root).unwrap();
        assert_eq!(Release::verify(root.verifying_key().as_bytes(), &signed).unwrap(), sample());
        let other = SigningKey::from_bytes(&[8; 32]);
        assert!(Release::verify(other.verifying_key().as_bytes(), &signed).is_err());
        let mut bad = signed.clone();
        bad[3] ^= 1;
        assert!(Release::verify(root.verifying_key().as_bytes(), &bad).is_err());
        // Paths are not file names.
        let mut r = sample();
        r.files[0].file = "../x.apk".into();
        assert!(r.sign(&root).is_err());
    }
}
