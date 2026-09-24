//! Canonical binary encoding: big-endian integers, strings as u16 length + UTF-8.

use crate::{Error, Id, Result};

pub const MAX_STR: usize = 1024;

pub fn valid_str(s: &str, allow_empty: bool) -> bool {
    s.len() <= MAX_STR && (allow_empty || !s.is_empty())
}

#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn raw(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    pub fn id(&mut self, id: &Id) {
        self.raw(&id.0);
    }
    /// Caller must have validated the length.
    pub fn str(&mut self, s: &str) {
        self.u16(s.len() as u16);
        self.raw(s.as_bytes());
    }
}

pub struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::Truncated);
        }
        let (h, t) = self.0.split_at(n);
        self.0 = t;
        Ok(h)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }
    pub fn fixed_vec(&mut self, n: usize) -> Result<Vec<u8>> {
        Ok(self.take(n)?.to_vec())
    }
    pub fn id(&mut self) -> Result<Id> {
        Ok(Id(self.fixed()?))
    }
    pub fn str(&mut self, allow_empty: bool) -> Result<String> {
        let n = self.u16()? as usize;
        let s = std::str::from_utf8(self.take(n)?).map_err(|_| Error::Invalid)?;
        if !valid_str(s, allow_empty) {
            return Err(Error::Invalid);
        }
        Ok(s.to_owned())
    }
    pub fn finish(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Trailing)
        }
    }
}
