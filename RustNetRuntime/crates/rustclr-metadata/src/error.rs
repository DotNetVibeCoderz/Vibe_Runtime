//! Error type shared by the metadata reader.

#[allow(unused_imports)]
use crate::prelude::*;

use core::fmt;

/// Everything that can go wrong while decoding a PE image or its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataError {
    /// The buffer ended before the requested number of bytes could be read.
    UnexpectedEof { offset: usize, needed: usize },
    /// `MZ` / `PE\0\0` / `BSJB` magic did not match.
    BadMagic { what: &'static str, found: u32 },
    /// An RVA did not fall inside any section of the image.
    RvaNotMapped(u32),
    /// The image has no CLI header, i.e. it is a plain native binary.
    NotManaged,
    /// A metadata heap referenced by a stream header is missing.
    MissingHeap(&'static str),
    /// A table index pointed past the end of its table.
    RowOutOfRange { table: &'static str, index: u32 },
    /// A coded index used a tag with no corresponding table.
    BadCodedIndex { kind: &'static str, tag: u32 },
    /// A metadata signature blob was malformed.
    BadSignature(&'static str),
    /// The image targets a PE format we do not decode (e.g. ROM images).
    UnsupportedPeMagic(u16),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof { offset, needed } => {
                write!(f, "unexpected end of image at 0x{offset:x} (needed {needed} bytes)")
            }
            Self::BadMagic { what, found } => write!(f, "bad {what} signature: 0x{found:08x}"),
            Self::RvaNotMapped(rva) => write!(f, "RVA 0x{rva:08x} is not mapped by any section"),
            Self::NotManaged => write!(f, "image has no CLI header (not a managed assembly)"),
            Self::MissingHeap(name) => write!(f, "metadata heap `{name}` is missing"),
            Self::RowOutOfRange { table, index } => {
                write!(f, "row {index} is out of range for table `{table}`")
            }
            Self::BadCodedIndex { kind, tag } => {
                write!(f, "coded index `{kind}` has invalid tag {tag}")
            }
            Self::BadSignature(why) => write!(f, "malformed metadata signature: {why}"),
            Self::UnsupportedPeMagic(m) => {
                write!(f, "unsupported PE optional-header magic 0x{m:04x}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MetadataError {}

pub type Result<T> = core::result::Result<T, MetadataError>;
