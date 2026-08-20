//! A bounds-checked little-endian cursor.
//!
//! Every read in the metadata layer goes through this type, so a corrupt or
//! hostile assembly yields a `MetadataError` instead of a panic. This is the
//! first place where the Rust rewrite buys us something CoreCLR pays for with
//! hand-audited pointer arithmetic.

#[allow(unused_imports)]
use crate::prelude::*;

use crate::error::{MetadataError, Result};

#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    pub const fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    #[inline]
    pub const fn position(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    #[inline]
    pub const fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    #[inline]
    fn check(&self, n: usize) -> Result<()> {
        if self.remaining() < n {
            Err(MetadataError::UnexpectedEof { offset: self.pos, needed: n })
        } else {
            Ok(())
        }
    }

    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.check(n)?;
        self.pos += n;
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.check(n)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        self.check(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// Reads either a 2- or 4-byte index depending on the target heap size.
    pub fn index(&mut self, wide: bool) -> Result<u32> {
        if wide { self.u32() } else { Ok(self.u16()? as u32) }
    }

    /// ECMA-335 II.23.2 compressed unsigned integer.
    pub fn compressed_u32(&mut self) -> Result<u32> {
        let b0 = self.u8()?;
        if b0 & 0x80 == 0 {
            Ok(b0 as u32)
        } else if b0 & 0xC0 == 0x80 {
            let b1 = self.u8()? as u32;
            Ok((((b0 & 0x3F) as u32) << 8) | b1)
        } else if b0 & 0xE0 == 0xC0 {
            let b1 = self.u8()? as u32;
            let b2 = self.u8()? as u32;
            let b3 = self.u8()? as u32;
            Ok((((b0 & 0x1F) as u32) << 24) | (b1 << 16) | (b2 << 8) | b3)
        } else {
            Err(MetadataError::BadSignature("invalid compressed integer prefix"))
        }
    }

    /// ECMA-335 II.23.2 compressed *signed* integer (rotate-encoded).
    pub fn compressed_i32(&mut self) -> Result<i32> {
        let start = self.pos;
        let raw = self.compressed_u32()?;
        let bits = match self.pos - start {
            1 => 7u32,
            2 => 14,
            _ => 29,
        };
        // The encoded value is rotated left by one; bit 0 carries the sign.
        let negative = raw & 1 == 1;
        let mut value = raw >> 1;
        if negative {
            value |= u32::MAX << (bits - 1);
        }
        Ok(value as i32)
    }

    /// Null-terminated ASCII string, consuming the terminator.
    pub fn cstr(&mut self) -> Result<&'a [u8]> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(MetadataError::UnexpectedEof { offset: start, needed: 1 });
        }
        let s = &self.data[start..self.pos];
        self.pos += 1; // terminator
        Ok(s)
    }

    /// Advances to the next 4-byte boundary measured from `base`.
    pub fn align4_from(&mut self, base: usize) -> Result<()> {
        let pad = (4 - ((self.pos - base) % 4)) % 4;
        self.skip(pad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_unsigned_covers_all_three_widths() {
        assert_eq!(Reader::new(&[0x03]).compressed_u32().unwrap(), 0x03);
        assert_eq!(Reader::new(&[0x7F]).compressed_u32().unwrap(), 0x7F);
        assert_eq!(Reader::new(&[0x80, 0x80]).compressed_u32().unwrap(), 0x80);
        assert_eq!(Reader::new(&[0xBF, 0xFF]).compressed_u32().unwrap(), 0x3FFF);
        assert_eq!(Reader::new(&[0xC0, 0x00, 0x40, 0x00]).compressed_u32().unwrap(), 0x4000);
        assert_eq!(Reader::new(&[0xDF, 0xFF, 0xFF, 0xFF]).compressed_u32().unwrap(), 0x1FFF_FFFF);
    }

    #[test]
    fn compressed_signed_matches_spec_examples() {
        assert_eq!(Reader::new(&[0x06]).compressed_i32().unwrap(), 3);
        assert_eq!(Reader::new(&[0x7B]).compressed_i32().unwrap(), -3);
        assert_eq!(Reader::new(&[0x80, 0x80]).compressed_i32().unwrap(), 64);
        assert_eq!(Reader::new(&[0x01]).compressed_i32().unwrap(), -64);
    }

    #[test]
    fn a_failed_read_leaves_the_cursor_untouched() {
        let mut r = Reader::new(&[0x01, 0x02]);
        assert!(r.u32().is_err());
        assert_eq!(r.position(), 0);
    }

    #[test]
    fn cstr_without_terminator_is_an_error_not_a_panic() {
        assert!(Reader::new(b"abc").cstr().is_err());
    }
}
