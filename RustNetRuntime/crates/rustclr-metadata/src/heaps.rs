//! The four metadata heaps: `#Strings`, `#US`, `#Blob` and `#GUID`.

#[allow(unused_imports)]
use crate::prelude::*;

use crate::error::{MetadataError, Result};
use crate::reader::Reader;

/// UTF-8 string heap. Offsets index directly into the byte array.
#[derive(Debug, Clone, Copy, Default)]
pub struct StringHeap<'a>(pub &'a [u8]);

impl<'a> StringHeap<'a> {
    /// Returns the null-terminated UTF-8 string at `offset`.
    ///
    /// Invalid UTF-8 is lossy-decoded rather than rejected: real-world
    /// assemblies occasionally carry mangled identifiers and refusing to load
    /// them would be worse than showing replacement characters.
    pub fn get(&self, offset: u32) -> Result<&'a str> {
        let off = offset as usize;
        if off >= self.0.len() {
            // Offset 0 in an absent heap is legitimately the empty string.
            return if off == 0 { Ok("") } else { Err(MetadataError::MissingHeap("#Strings")) };
        }
        let rest = &self.0[off..];
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Ok(core::str::from_utf8(&rest[..end]).unwrap_or("<invalid-utf8>"))
    }
}

/// Blob heap: each entry is a compressed length followed by that many bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlobHeap<'a>(pub &'a [u8]);

impl<'a> BlobHeap<'a> {
    pub fn get(&self, offset: u32) -> Result<&'a [u8]> {
        let off = offset as usize;
        if off >= self.0.len() {
            return if off == 0 { Ok(&[]) } else { Err(MetadataError::MissingHeap("#Blob")) };
        }
        let mut r = Reader::at(self.0, off);
        let len = r.compressed_u32()? as usize;
        r.bytes(len)
    }
}

/// GUID heap: a flat array of 16-byte GUIDs, indexed 1-based.
#[derive(Debug, Clone, Copy, Default)]
pub struct GuidHeap<'a>(pub &'a [u8]);

impl<'a> GuidHeap<'a> {
    pub fn get(&self, index: u32) -> Result<[u8; 16]> {
        if index == 0 {
            return Ok([0u8; 16]);
        }
        let off = (index as usize - 1) * 16;
        let slice = self
            .0
            .get(off..off + 16)
            .ok_or(MetadataError::MissingHeap("#GUID"))?;
        let mut out = [0u8; 16];
        out.copy_from_slice(slice);
        Ok(out)
    }
}

/// User-string heap: UTF-16 literals referenced by `ldstr`.
#[derive(Debug, Clone, Copy, Default)]
pub struct UserStringHeap<'a>(pub &'a [u8]);

impl<'a> UserStringHeap<'a> {
    /// Decodes the UTF-16LE literal at `offset` into an owned `String`.
    ///
    /// The stored blob length includes one trailing flag byte that records
    /// whether any character needs special handling; it is not part of the text.
    pub fn get(&self, offset: u32) -> Result<String> {
        let off = offset as usize;
        if off >= self.0.len() {
            return if off == 0 { Ok(String::new()) } else { Err(MetadataError::MissingHeap("#US")) };
        }
        let mut r = Reader::at(self.0, off);
        let len = r.compressed_u32()? as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let bytes = r.bytes(len)?;
        let char_bytes = &bytes[..len - 1]; // drop the terminal flag byte
        let units: Vec<u16> = char_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_heap_reads_null_terminated_entries() {
        let heap = StringHeap(b"\0System\0Console\0");
        assert_eq!(heap.get(0).unwrap(), "");
        assert_eq!(heap.get(1).unwrap(), "System");
        assert_eq!(heap.get(8).unwrap(), "Console");
    }

    #[test]
    fn blob_heap_honours_the_compressed_length() {
        let heap = BlobHeap(&[0x00, 0x03, 0xAA, 0xBB, 0xCC]);
        assert_eq!(heap.get(1).unwrap(), &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn user_string_heap_decodes_utf16_and_drops_the_flag_byte() {
        // "Hi" == 48 00 69 00, plus the trailing flag byte -> length 5.
        let heap = UserStringHeap(&[0x00, 0x05, 0x48, 0x00, 0x69, 0x00, 0x00]);
        assert_eq!(heap.get(1).unwrap(), "Hi");
    }

    #[test]
    fn guid_heap_is_one_based() {
        let mut raw = [0u8; 32];
        raw[16] = 0x7F;
        let heap = GuidHeap(&raw);
        assert_eq!(heap.get(0).unwrap(), [0u8; 16]);
        assert_eq!(heap.get(2).unwrap()[0], 0x7F);
        assert!(heap.get(3).is_err());
    }
}
