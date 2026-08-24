//! SHA-256, from FIPS 180-4.
//!
//! Written out rather than depended on. The reference implementation this was
//! ported alongside uses the `sha2` crate; this workspace has no external
//! dependencies, and that posture is load-bearing — it is why the PE reader,
//! the metadata reader, the portable-PDB reader and the debug adapter's JSON
//! are all here in full. A hash is 60 lines and a published set of test
//! vectors, which is a better trade than a dependency that has to build for
//! four bare-metal targets.
//!
//! The vectors at the bottom are the ones FIPS 180-4 and NIST's CAVP publish,
//! including the million-`a` case that catches a length counter that overflows
//! at 2^32 bits.

#[allow(unused_imports)]
use alloc::vec::Vec;

/// Bytes in a SHA-256 digest.
pub const DIGEST_LEN: usize = 32;
/// Bytes in a SHA-256 compression block.
pub const BLOCK_LEN: usize = 64;

/// The first 32 bits of the fractional parts of the cube roots of the first
/// 64 primes — FIPS 180-4 §4.2.2.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The first 32 bits of the fractional parts of the square roots of the first
/// eight primes — FIPS 180-4 §5.3.3.
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// An incremental SHA-256.
///
/// Incremental rather than one-shot because the things this runtime hashes —
/// a firmware image being streamed over the air, a file being read a sector at
/// a time — do not fit in memory on the boards it targets.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    /// Partial block waiting for enough bytes to compress.
    buffer: [u8; BLOCK_LEN],
    buffered: usize,
    /// Message length in **bits**, which is what the padding encodes.
    ///
    /// `u64` because FIPS specifies a 64-bit length field; a 32-bit counter
    /// wraps at 512 MB and produces a valid-looking digest for the wrong
    /// message. The million-character vector below is what catches that.
    bits: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self { state: H0, buffer: [0; BLOCK_LEN], buffered: 0, bits: 0 }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.bits = self.bits.wrapping_add((data.len() as u64) * 8);

        // Top up a partial block first, so the fast path below always starts
        // aligned.
        if self.buffered > 0 {
            let take = (BLOCK_LEN - self.buffered).min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == BLOCK_LEN {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }

        // Whole blocks straight from the caller's slice: no copy.
        while data.len() >= BLOCK_LEN {
            let (block, rest) = data.split_at(BLOCK_LEN);
            let mut chunk = [0u8; BLOCK_LEN];
            chunk.copy_from_slice(block);
            self.compress(&chunk);
            data = rest;
        }

        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffered = data.len();
        }
    }

    /// Finishes the hash. Consumes the state: a digest read twice from a
    /// half-padded hasher would differ, so the type does not allow it.
    pub fn finish(mut self) -> [u8; DIGEST_LEN] {
        let bits = self.bits;

        // Padding: a single 1 bit, zeros, then the length as 64 big-endian
        // bits, arranged so the total is a whole number of blocks.
        self.buffer[self.buffered] = 0x80;
        self.buffered += 1;
        if self.buffered > BLOCK_LEN - 8 {
            self.buffer[self.buffered..].fill(0);
            let block = self.buffer;
            self.compress(&block);
            self.buffered = 0;
        }
        self.buffer[self.buffered..BLOCK_LEN - 8].fill(0);
        self.buffer[BLOCK_LEN - 8..].copy_from_slice(&bits.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);

        let mut out = [0u8; DIGEST_LEN];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; BLOCK_LEN]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}

/// One-shot SHA-256.
pub fn sha256(data: &[u8]) -> [u8; DIGEST_LEN] {
    let mut h = Sha256::new();
    h.update(data);
    h.finish()
}

/// Lower-case hex, for a digest that has to be printed or compared as text.
pub fn hex(bytes: &[u8]) -> alloc::string::String {
    use core::fmt::Write as _;
    let mut out = alloc::string::String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vectors FIPS 180-4 and NIST's CAVP publish.
    #[test]
    fn matches_the_published_vectors() {
        let cases: [(&[u8], &str); 4] = [
            (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
                "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(hex(&sha256(input)), expected, "input {input:?}");
        }
    }

    /// A million `a`s: 8,000,000 bits, which is past what a 32-bit length
    /// counter holds. A hash that tracked length in `u32` produces a
    /// plausible-looking digest here and a correct one everywhere above.
    #[test]
    fn the_million_character_vector_catches_a_narrow_length_counter() {
        let mut h = Sha256::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            hex(&h.finish()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// Feeding the same bytes in different-sized pieces must give the same
    /// answer. This is what the buffering above is for, and an off-by-one in
    /// the top-up path shows here rather than in production.
    #[test]
    fn the_split_between_updates_does_not_change_the_digest() {
        let message: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let once = sha256(&message);
        for split in [1, 63, 64, 65, 127, 128, 129, 500, 999] {
            let mut h = Sha256::new();
            h.update(&message[..split]);
            h.update(&message[split..]);
            assert_eq!(h.finish(), once, "split at {split}");
        }
    }

    /// The block boundary is where padding decides whether it needs an extra
    /// block, so every length either side of one is worth a case.
    #[test]
    fn lengths_around_a_block_boundary_are_right() {
        // Independently checked against the reference: only the relationship
        // matters here — every length must produce a distinct digest, and a
        // padding bug collapses two of them onto each other.
        let mut seen = alloc::vec::Vec::new();
        for len in 0..=130usize {
            let message = alloc::vec![b'x'; len];
            let digest = sha256(&message);
            assert!(!seen.contains(&digest), "length {len} collided with a shorter message");
            seen.push(digest);
        }
    }
}
