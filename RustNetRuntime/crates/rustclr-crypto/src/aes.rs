//! AES-128 and AES-256 in counter mode, from FIPS 197 and NIST SP 800-38A.
//!
//! # Counter mode only
//!
//! No ECB, and not by omission. ECB encrypts equal blocks to equal ciphertext,
//! which is why the penguin picture is the one everybody has seen; offering it
//! next to CTR would be offering a foot-gun with the same shape as the safe
//! option. CTR turns the block cipher into a keystream, needs no padding, and
//! encrypts and decrypts with the same code.
//!
//! # The nonce is the caller's problem, and it is stated
//!
//! CTR is only secure while a (key, counter) pair is never reused. Reusing one
//! XORs two plaintexts together and both are recoverable. This crate cannot
//! enforce that — it does not know what came before — so it does the next best
//! thing: says so here, and in `docs/limitations.md`, rather than implying
//! safety it cannot provide.

#[allow(unused_imports)]
use alloc::vec::Vec;

/// The AES S-box, FIPS 197 §5.1.1.
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// Round constants for the key schedule, FIPS 197 §5.2.
const RCON: [u8; 11] = [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

/// Multiplication by x in GF(2^8) with the AES polynomial.
#[inline]
fn xtime(b: u8) -> u8 {
    (b << 1) ^ (((b >> 7) & 1) * 0x1b)
}

/// An expanded AES key. 128 or 256 bits; 192 is legal AES and simply not
/// offered, because nothing here needs it and an unused branch is an untested
/// one.
pub struct Aes {
    round_keys: [[u8; 16]; 15],
    rounds: usize,
}

impl Aes {
    /// Expands a key. `None` for a length that is not 16 or 32 bytes —
    /// refusing beats silently padding a short key, which would make a
    /// 5-byte password into a valid-looking 128-bit key.
    pub fn new(key: &[u8]) -> Option<Self> {
        let (nk, rounds) = match key.len() {
            16 => (4usize, 10usize),
            32 => (8usize, 14usize),
            _ => return None,
        };

        let words = 4 * (rounds + 1);
        let mut w = [[0u8; 4]; 60];
        for i in 0..nk {
            w[i].copy_from_slice(&key[i * 4..i * 4 + 4]);
        }
        for i in nk..words {
            let mut temp = w[i - 1];
            if i % nk == 0 {
                temp.rotate_left(1);
                for b in temp.iter_mut() {
                    *b = SBOX[*b as usize];
                }
                temp[0] ^= RCON[i / nk];
            } else if nk > 6 && i % nk == 4 {
                for b in temp.iter_mut() {
                    *b = SBOX[*b as usize];
                }
            }
            for j in 0..4 {
                w[i][j] = w[i - nk][j] ^ temp[j];
            }
        }

        let mut round_keys = [[0u8; 16]; 15];
        for r in 0..=rounds {
            for c in 0..4 {
                round_keys[r][c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
            }
        }
        Some(Self { round_keys, rounds })
    }

    /// Encrypts one block in place. CTR never needs the inverse cipher, which
    /// is why there is no `decrypt_block` here to go stale.
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        for (b, k) in block.iter_mut().zip(self.round_keys[0].iter()) {
            *b ^= k;
        }

        for round in 1..=self.rounds {
            for b in block.iter_mut() {
                *b = SBOX[*b as usize];
            }
            shift_rows(block);
            if round != self.rounds {
                mix_columns(block);
            }
            for (b, k) in block.iter_mut().zip(self.round_keys[round].iter()) {
                *b ^= k;
            }
        }
    }
}

fn shift_rows(s: &mut [u8; 16]) {
    // Column-major, as FIPS lays the state out: byte r + 4c is row r, column c.
    let t = *s;
    for r in 1..4 {
        for c in 0..4 {
            s[r + 4 * c] = t[r + 4 * ((c + r) % 4)];
        }
    }
}

fn mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
        let all = col[0] ^ col[1] ^ col[2] ^ col[3];
        for r in 0..4 {
            s[4 * c + r] = col[r] ^ all ^ xtime(col[r] ^ col[(r + 1) % 4]);
        }
    }
}

/// Encrypts or decrypts in place with AES-CTR. The two are the same operation.
///
/// `nonce` is the initial 16-byte counter block, incremented big-endian per
/// block as SP 800-38A specifies. **A (key, nonce) pair must never be reused**:
/// doing so XORs two plaintexts together and both come back out. This function
/// cannot check that, so it says it instead.
///
/// Returns `false` for a key length it does not accept, leaving the data
/// untouched — rather than returning plaintext that looks encrypted.
pub fn aes_ctr(key: &[u8], nonce: &[u8; 16], data: &mut [u8]) -> bool {
    let Some(aes) = Aes::new(key) else { return false };
    let mut counter = *nonce;
    for chunk in data.chunks_mut(16) {
        let mut keystream = counter;
        aes.encrypt_block(&mut keystream);
        for (b, k) in chunk.iter_mut().zip(keystream.iter()) {
            *b ^= k;
        }
        // Big-endian increment across the whole block, so a counter that wraps
        // its low byte carries rather than repeating a keystream block.
        for byte in counter.iter_mut().rev() {
            *byte = byte.wrapping_add(1);
            if *byte != 0 {
                break;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::hex;

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// FIPS 197 Appendix B and C: the worked example, and the two key sizes.
    #[test]
    fn matches_fips_197() {
        // Appendix B: key 000102...0f, plaintext 00112233...ff.
        let key = bytes("000102030405060708090a0b0c0d0e0f");
        let mut block = [0u8; 16];
        block.copy_from_slice(&bytes("00112233445566778899aabbccddeeff"));
        Aes::new(&key).unwrap().encrypt_block(&mut block);
        assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");

        // Appendix C.3: AES-256 over the same plaintext.
        let key = bytes("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let mut block = [0u8; 16];
        block.copy_from_slice(&bytes("00112233445566778899aabbccddeeff"));
        Aes::new(&key).unwrap().encrypt_block(&mut block);
        assert_eq!(hex(&block), "8ea2b7ca516745bfeafc49904b496089");
    }

    /// NIST SP 800-38A §F.5.1, AES-128-CTR.
    #[test]
    fn matches_sp_800_38a_counter_mode() {
        let key = bytes("2b7e151628aed2a6abf7158809cf4f3c");
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&bytes("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff"));
        let mut data = bytes(
            "6bc1bee22e409f96e93d7e117393172a\
             ae2d8a571e03ac9c9eb76fac45af8e51\
             30c81c46a35ce411e5fbc1191a0a52ef\
             f69f2445df4f9b17ad2b417be66c3710",
        );
        assert!(aes_ctr(&key, &nonce, &mut data));
        assert_eq!(
            hex(&data),
            "874d6191b620e3261bef6864990db6ce\
             9806f66b7970fdff8617187bb9fffdff\
             5ae4df3edbd5d35e5b4f09020db03eab\
             1e031dda2fbe03d1792170a0f3009cee"
                .replace(' ', "")
        );
    }

    #[test]
    fn counter_mode_round_trips() {
        let key = [0x42u8; 32];
        let nonce = [7u8; 16];
        let original: Vec<u8> = (0..200u32).map(|i| (i % 256) as u8).collect();
        let mut data = original.clone();
        assert!(aes_ctr(&key, &nonce, &mut data));
        assert_ne!(data, original, "the ciphertext is the plaintext");
        assert!(aes_ctr(&key, &nonce, &mut data));
        assert_eq!(data, original, "decrypting is the same operation");
    }

    /// A length that is not a whole number of blocks must still work — CTR
    /// needs no padding, and a implementation that padded would change the
    /// length.
    #[test]
    fn a_partial_final_block_is_handled() {
        let key = [1u8; 16];
        let nonce = [0u8; 16];
        for len in [1usize, 15, 16, 17, 33] {
            let original: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let mut data = original.clone();
            assert!(aes_ctr(&key, &nonce, &mut data));
            assert_eq!(data.len(), len);
            assert!(aes_ctr(&key, &nonce, &mut data));
            assert_eq!(data, original, "length {len}");
        }
    }

    #[test]
    fn a_key_of_the_wrong_length_is_refused_rather_than_padded() {
        let mut data = [1u8, 2, 3];
        let before = data;
        assert!(!aes_ctr(&[0u8; 5], &[0u8; 16], &mut data));
        assert_eq!(data, before, "refused, and the data is untouched");
        assert!(Aes::new(&[0u8; 24]).is_none(), "192-bit keys are not offered");
    }
}
