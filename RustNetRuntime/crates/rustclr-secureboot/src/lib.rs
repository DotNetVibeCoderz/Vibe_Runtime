//! Signed image containers, for firmware and for application packages.
//!
//! ```text
//! "RCSB" | u16 version | u8 kind | u8 chip | u32 payload_len | u32 tag_len
//!        | payload | tag(HMAC-SHA256 over the header with tag_len zeroed, then payload)
//! ```
//!
//! # A MAC, not a signature — and what that costs
//!
//! The reference this was ported from verifies an **RSA PKCS#1 v1.5**
//! signature. This one verifies an **HMAC-SHA256** against a key provisioned
//! into the device at flash time, and the difference is worth stating plainly
//! rather than glossing:
//!
//! - **What it still gives you.** Nobody without the key can produce an image
//!   this accepts. A corrupted download, a truncated flash write, a
//!   man-in-the-middle on the update channel and an image built for another
//!   chip are all rejected before a byte of payload is executed.
//! - **What it does not give you.** The verifying key is also a *signing* key.
//!   A device that can check an image can forge one, so extracting the key
//!   from any one device lets an attacker sign images for all of them. Public
//!   key cryptography does not have that property, which is why real secure
//!   boot uses it.
//!
//! The reason for the trade is in `docs/limitations.md` and in
//! [`rustclr_crypto`]: RSA needs bignum arithmetic, this workspace has no
//! external dependencies, and a hand-rolled bignum with a timing side channel
//! is *worse* than no signature check — because it is a check that reports
//! success. A MAC that is honestly described beats a signature that is not
//! really one.
//!
//! Per-device keys narrow the blast radius: provision a different key to each
//! board and a single extraction compromises one device rather than the fleet.
//!
//! Builds `no_std` with `alloc`, so a bootloader verifies a container with
//! exactly the code the host tools used to build it.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[allow(unused_imports)]
use alloc::{format, string::{String, ToString}, vec::Vec};

use rustclr_crypto::{constant_time_eq, hmac_sha256};

pub const MAGIC: &[u8; 4] = b"RCSB";
pub const VERSION: u16 = 1;
/// Bytes of header before the payload.
const HEADER_LEN: usize = 4 + 2 + 1 + 1 + 4 + 4;
/// Bytes in an HMAC-SHA256 tag.
const TAG_LEN: usize = 32;

/// What the payload is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Firmware = 0,
    App = 1,
    Data = 2,
    BootImage = 3,
}

impl ImageKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => ImageKind::Firmware,
            1 => ImageKind::App,
            2 => ImageKind::Data,
            3 => ImageKind::BootImage,
            _ => return None,
        })
    }
}

/// Which chip the payload was built for.
///
/// Checked, not decorative: flashing an ESP32 image onto an STM32 bricks the
/// board until somebody attaches a probe, and the container knows enough to
/// refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipFamily {
    Any = 0,
    Esp32 = 1,
    Stm32 = 2,
    HostSim = 3,
    /// ESP32-C3 (RISC-V RV32IMC, single core).
    Esp32C3 = 4,
    /// Kendryte K210 (RISC-V RV64GC, dual core).
    K210 = 5,
    /// Raspberry Pi RP2040 (dual Cortex-M0+, execute-in-place QSPI flash).
    Rp2040 = 6,
}

impl ChipFamily {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => ChipFamily::Any,
            1 => ChipFamily::Esp32,
            2 => ChipFamily::Stm32,
            3 => ChipFamily::HostSim,
            4 => ChipFamily::Esp32C3,
            5 => ChipFamily::K210,
            6 => ChipFamily::Rp2040,
            _ => return None,
        })
    }

    /// Whether an image built for `self` may run on `device`.
    ///
    /// `Any` runs anywhere; everything else must match exactly. There is no
    /// "close enough" — an ESP32 image is not an ESP32-C3 image, and the two
    /// are not even the same instruction set.
    pub fn accepts(self, device: ChipFamily) -> bool {
        self == ChipFamily::Any || self == device
    }
}

/// Why a container was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootError {
    /// Not a container at all.
    BadMagic,
    /// A container from a newer format than this device understands.
    UnsupportedVersion(u16),
    /// The header says the payload is longer than the bytes provided.
    Truncated,
    /// The tag does not match. Either the image was altered, or it was signed
    /// with a different key.
    BadTag,
    /// The image was built for another chip.
    WrongChip { image: ChipFamily, device: ChipFamily },
    /// A field held a value this version does not define.
    Malformed(String),
}

impl core::fmt::Display for BootError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BootError::BadMagic => write!(f, "not a RustCLR signed image"),
            BootError::UnsupportedVersion(v) => {
                write!(f, "container version {v} is newer than this device understands")
            }
            BootError::Truncated => write!(f, "the image is shorter than its header says"),
            BootError::BadTag => {
                write!(f, "the image was altered, or signed with a different key")
            }
            BootError::WrongChip { image, device } => {
                write!(f, "the image is for {image:?} and this is a {device:?}")
            }
            BootError::Malformed(what) => write!(f, "malformed image: {what}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BootError {}

/// A container that has been checked, and what was inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedImage<'a> {
    pub kind: ImageKind,
    pub chip: ChipFamily,
    pub payload: &'a [u8],
}

/// Wraps a payload in a signed container.
///
/// Host-side: this is what a build tool calls. The key here is the same key
/// the device holds, which is the whole limitation — see the crate docs.
pub fn seal(key: &[u8], kind: ImageKind, chip: ChipFamily, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.push(kind as u8);
    out.push(chip as u8);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    // The tag length is written as zero while the tag is computed, so that
    // verification can reconstruct exactly these bytes. Writing the real
    // length first and zeroing it later would make the two disagree.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(payload);

    let tag = hmac_sha256(key, &out);
    out[HEADER_LEN - 4..HEADER_LEN].copy_from_slice(&(TAG_LEN as u32).to_le_bytes());
    out.extend_from_slice(&tag);
    out
}

/// Checks a container against a key and a chip, returning what was inside.
///
/// The order of the checks is deliberate: shape first, then chip, then the
/// tag. A caller that gets `WrongChip` learns something useful about a valid
/// image; a caller that gets `BadTag` learns only that the bytes are not
/// trustworthy, which is all it should learn.
pub fn verify<'a>(
    image: &'a [u8],
    key: &[u8],
    device: ChipFamily,
) -> Result<VerifiedImage<'a>, BootError> {
    if image.len() < HEADER_LEN || &image[..4] != MAGIC {
        return Err(BootError::BadMagic);
    }
    let version = u16::from_le_bytes([image[4], image[5]]);
    if version > VERSION {
        return Err(BootError::UnsupportedVersion(version));
    }
    let kind = ImageKind::from_u8(image[6])
        .ok_or_else(|| BootError::Malformed(format!("image kind {}", image[6])))?;
    let chip = ChipFamily::from_u8(image[7])
        .ok_or_else(|| BootError::Malformed(format!("chip family {}", image[7])))?;

    let payload_len = u32::from_le_bytes([image[8], image[9], image[10], image[11]]) as usize;
    let tag_len = u32::from_le_bytes([image[12], image[13], image[14], image[15]]) as usize;
    if tag_len != TAG_LEN {
        return Err(BootError::Malformed(format!(
            "tag length {tag_len}, expected {TAG_LEN}"
        )));
    }
    // Checked with `checked_add` rather than `+`: a payload length of
    // 0xFFFFFFFF would otherwise wrap on a 32-bit target and pass a length
    // test it should fail, which is how a truncated image gets read past its
    // end.
    let end = HEADER_LEN
        .checked_add(payload_len)
        .and_then(|n| n.checked_add(tag_len))
        .ok_or(BootError::Truncated)?;
    if image.len() < end {
        return Err(BootError::Truncated);
    }

    if !chip.accepts(device) {
        return Err(BootError::WrongChip { image: chip, device });
    }

    // Reconstruct exactly what `seal` hashed: the header with the tag length
    // back at zero, then the payload.
    let mut signed = Vec::with_capacity(HEADER_LEN + payload_len);
    signed.extend_from_slice(&image[..HEADER_LEN]);
    signed[HEADER_LEN - 4..HEADER_LEN].copy_from_slice(&0u32.to_le_bytes());
    signed.extend_from_slice(&image[HEADER_LEN..HEADER_LEN + payload_len]);

    let expected = hmac_sha256(key, &signed);
    let actual = &image[HEADER_LEN + payload_len..end];
    if !constant_time_eq(&expected, actual) {
        return Err(BootError::BadTag);
    }

    Ok(VerifiedImage {
        kind,
        chip,
        payload: &image[HEADER_LEN..HEADER_LEN + payload_len],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"a provisioned device key, 32 by!";

    #[test]
    fn a_sealed_image_verifies_and_comes_back_whole() {
        let payload = b"the firmware bytes";
        let image = seal(KEY, ImageKind::Firmware, ChipFamily::K210, payload);
        let verified = verify(&image, KEY, ChipFamily::K210).expect("verifies");
        assert_eq!(verified.payload, payload);
        assert_eq!(verified.kind, ImageKind::Firmware);
        assert_eq!(verified.chip, ChipFamily::K210);
    }

    #[test]
    fn an_altered_payload_is_rejected() {
        let mut image = seal(KEY, ImageKind::App, ChipFamily::Any, b"hello");
        let last = image.len() - TAG_LEN - 1;
        image[last] ^= 0x01;
        assert_eq!(verify(&image, KEY, ChipFamily::K210), Err(BootError::BadTag));
    }

    /// The header is covered too. Without this, an attacker could relabel a
    /// data blob as firmware and the tag would still match.
    #[test]
    fn an_altered_header_is_rejected() {
        let mut image = seal(KEY, ImageKind::Data, ChipFamily::Any, b"hello");
        image[6] = ImageKind::Firmware as u8;
        assert_eq!(verify(&image, KEY, ChipFamily::K210), Err(BootError::BadTag));
    }

    #[test]
    fn another_key_is_rejected() {
        let image = seal(KEY, ImageKind::App, ChipFamily::Any, b"hello");
        assert_eq!(
            verify(&image, b"a different device key, 32 byte!", ChipFamily::K210),
            Err(BootError::BadTag)
        );
    }

    /// Flashing an ESP32 image onto an STM32 bricks the board. The container
    /// knows enough to refuse, and it refuses *before* it trusts the payload.
    #[test]
    fn an_image_for_another_chip_is_refused() {
        let image = seal(KEY, ImageKind::Firmware, ChipFamily::Esp32, b"x");
        assert_eq!(
            verify(&image, KEY, ChipFamily::Stm32),
            Err(BootError::WrongChip { image: ChipFamily::Esp32, device: ChipFamily::Stm32 })
        );
        // `Any` runs anywhere, which is what a data blob wants.
        let anywhere = seal(KEY, ImageKind::Data, ChipFamily::Any, b"x");
        assert!(verify(&anywhere, KEY, ChipFamily::Stm32).is_ok());
    }

    #[test]
    fn a_truncated_image_is_refused_rather_than_read_past_its_end() {
        let image = seal(KEY, ImageKind::App, ChipFamily::Any, b"a longer payload here");
        for cut in [0, 4, HEADER_LEN, HEADER_LEN + 5, image.len() - 1] {
            let short = &image[..cut];
            assert!(
                verify(short, KEY, ChipFamily::Any).is_err(),
                "a {cut}-byte prefix must not verify"
            );
        }
    }

    #[test]
    fn something_that_is_not_a_container_says_so() {
        assert_eq!(verify(b"not an image at all!!!!", KEY, ChipFamily::Any), Err(BootError::BadMagic));
        assert_eq!(verify(b"", KEY, ChipFamily::Any), Err(BootError::BadMagic));
    }

    #[test]
    fn an_empty_payload_is_legal() {
        let image = seal(KEY, ImageKind::Data, ChipFamily::Any, b"");
        let verified = verify(&image, KEY, ChipFamily::Any).expect("verifies");
        assert!(verified.payload.is_empty());
    }
}
