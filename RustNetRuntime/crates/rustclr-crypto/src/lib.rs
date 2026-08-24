//! Cryptography for a device: SHA-256, HMAC-SHA256, AES-CTR and CRC-32.
//!
//! # Written out, not depended on
//!
//! The reference implementation this was ported alongside reaches for `sha2`,
//! `hmac`, `aes`, `ctr` and `rsa`. This workspace has **no external
//! dependencies**, and that is not tidiness — it is why the PE reader, the
//! ECMA-335 metadata reader, the portable-PDB reader and the debug adapter's
//! JSON are all here in full, and why four bare-metal targets build without a
//! vendoring step.
//!
//! SHA-256, HMAC and AES are each a published specification and a published
//! set of test vectors. Writing them out costs a few hundred lines and buys a
//! stronger claim than a dependency does: the vectors in these files are the
//! ones NIST and the RFCs publish, and they run on every target in
//! `tests/embedded.sh`.
//!
//! # What is deliberately not here
//!
//! **RSA and elliptic curves.** Signature verification for secure boot needs
//! bignum arithmetic, and a hand-rolled bignum with a side-channel is worse
//! than no signature check at all — it is a check that reports success.
//! [`rustclr_secureboot`](../rustclr_secureboot/index.html) therefore verifies
//! an HMAC over a key provisioned at flash time, which is a real guarantee
//! with a real limitation, stated in `docs/limitations.md` rather than dressed
//! up as public-key verification.
//!
//! **A cryptographic random number generator.** Entropy is a property of the
//! hardware, not of this crate: a K210 has a TRNG, an STM32F4 has one on some
//! parts, and a simulated board has none. It comes from the board through the
//! HAL, and a board that has none says so instead of returning a counter.
//!
//! # Constant time
//!
//! [`hmac::constant_time_eq`] is the only comparison that should ever be used
//! on a tag or a digest. AES here is a straightforward table-free
//! implementation; it is not hardened against a power-analysis attacker with
//! physical access to the board, and `docs/limitations.md` says so.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod aes;
pub mod crc32;
pub mod hmac;
pub mod sha256;

pub use aes::{Aes, aes_ctr};
pub use crc32::crc32;
pub use hmac::{constant_time_eq, hmac_sha256};
pub use sha256::{Sha256, sha256};
