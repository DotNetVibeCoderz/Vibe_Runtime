//! CRC-32 (IEEE 802.3), for integrity rather than authenticity.
//!
//! Used by the flash store and the OTA transport to notice a corrupted block.
//! It is **not** a security check and this file says so where someone reaching
//! for it will read it: a CRC is trivially forgeable, and anything that has to
//! resist an attacker uses [`crate::hmac_sha256`].

/// The reflected polynomial, which is what IEEE 802.3 uses.
const POLY: u32 = 0xEDB88320;

/// CRC-32 of a byte string.
///
/// Table-free: 256 entries would be 1 KB of RAM on a board where the whole
/// heap is measured in kilobytes, to save a few microseconds on a checksum
/// that runs once per block.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (POLY & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The check value every CRC-32 implementation publishes.
    #[test]
    fn matches_the_published_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7BE43);
    }

    #[test]
    fn a_changed_byte_changes_the_checksum() {
        assert_ne!(crc32(b"hello"), crc32(b"hellp"));
        // Order matters: a sum rather than a CRC would give these the same value.
        assert_ne!(crc32(b"ab"), crc32(b"ba"));
    }
}
