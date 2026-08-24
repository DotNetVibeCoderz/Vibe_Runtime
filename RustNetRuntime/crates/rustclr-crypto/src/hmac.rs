//! HMAC-SHA256, from RFC 2104, with RFC 4231's test vectors.
//!
//! Small enough to write out, and worth writing out: this is what signs an OTA
//! manifest and what authenticates a device to a broker, so a version whose
//! provenance is "a crate resolved at build time" is a worse answer than one
//! whose provenance is a published vector in the file beside it.

use crate::sha256::{Sha256, BLOCK_LEN, DIGEST_LEN};

#[allow(unused_imports)]
use alloc::vec::Vec;

/// HMAC-SHA256 of `message` under `key`.
///
/// A key longer than the block is hashed first and a shorter one is padded, as
/// RFC 2104 §2 specifies — so two different long keys cannot collide onto the
/// same padded key, which is the mistake a naive truncation makes.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; DIGEST_LEN] {
    let mut block = [0u8; BLOCK_LEN];
    if key.len() > BLOCK_LEN {
        let hashed = crate::sha256::sha256(key);
        block[..DIGEST_LEN].copy_from_slice(&hashed);
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36u8; BLOCK_LEN];
    let mut outer_pad = [0x5cu8; BLOCK_LEN];
    for i in 0..BLOCK_LEN {
        inner_pad[i] ^= block[i];
        outer_pad[i] ^= block[i];
    }

    let mut inner = Sha256::new();
    inner.update(&inner_pad);
    inner.update(message);
    let inner_digest = inner.finish();

    let mut outer = Sha256::new();
    outer.update(&outer_pad);
    outer.update(&inner_digest);
    outer.finish()
}

/// Compares two byte strings without an early exit.
///
/// A `==` on a MAC leaks where the first differing byte is through how long
/// the comparison took, which is enough to forge one byte at a time. This
/// reads every byte of both regardless, and differing lengths fail without
/// comparing at all — a length is not secret.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        difference |= x ^ y;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::hex;

    /// RFC 4231 §4, the cases that use SHA-256 and a full-length output.
    #[test]
    fn matches_rfc_4231() {
        // Case 1
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        // Case 2: a key shorter than the block, and a message that is text.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Case 3
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 20], &[0xdd; 50])),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        // Case 6: a key *longer* than the block, so it is hashed first. This
        // is the case that fails if the key is truncated instead.
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn constant_time_eq_answers_the_same_as_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    /// One flipped bit anywhere must change the tag. Without this the test
    /// above would pass against an HMAC that ignored its message.
    #[test]
    fn a_changed_message_changes_the_tag() {
        let key = b"a key";
        let base = hmac_sha256(key, b"the quick brown fox");
        assert_ne!(base, hmac_sha256(key, b"the quick brown fox."));
        assert_ne!(base, hmac_sha256(b"a keY", b"the quick brown fox"));
    }
}
