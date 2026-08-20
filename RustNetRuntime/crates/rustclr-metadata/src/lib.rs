//! # rustclr-metadata
//!
//! ECMA-335 metadata and PE/COFF reading for [RustCLR].
//!
//! This crate is the runtime's eyes: it turns a `.dll`/`.exe` on disk into
//! typed, bounds-checked views of the type system encoded inside it. Nothing
//! here allocates the managed heap or executes code — it only decodes.
//!
//! The entry point is [`Image`], which owns the file bytes and exposes both the
//! PE container ([`pe::PeImage`]) and the metadata tables ([`tables::Metadata`]).
//!
//! ```no_run
//! use rustclr_metadata::Image;
//!
//! let image = Image::from_file("HelloWorld.dll")?;
//! for row in 1..=image.metadata().row_count(rustclr_metadata::TableId::TypeDef) {
//!     let ty = image.metadata().type_def(row)?;
//!     println!("{}", ty.full_name());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [RustCLR]: https://github.com/gravicode/RustNetRuntime

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::{format, string::{String, ToString}, vec::Vec, boxed::Box};

pub mod body;
pub mod error;
pub mod heaps;
pub mod pe;
pub mod reader;
pub mod rows;
pub mod schema;
pub mod signature;
pub mod tables;
pub mod token;

pub use body::{ExceptionClause, HandlerKind, MethodBody};
pub use error::{MetadataError, Result};
pub use pe::{CliHeader, Machine, PeImage, Section};
pub use reader::Reader;
pub use rows::*;
pub use signature::{LocalVarSig, MethodSig, SignatureParser, TypeSig};
pub use tables::{Metadata, RowCursor, TableInfo};
pub use token::{CodedIndex, TableId, Token};

/// A managed image: the PE container plus its decoded metadata.
///
/// `Image` self-references — the metadata borrows from the same buffer the PE
/// header describes — so it stores the bytes in a boxed slice and rebuilds the
/// borrowed views on demand. Callers get `&`-lifetimes tied to the `Image`.
#[cfg(feature = "std")]
pub struct Image {
    bytes: Box<[u8]>,
    /// Byte range of the metadata root within `bytes`.
    metadata_range: core::ops::Range<usize>,
    path: Option<std::path::PathBuf>,
}

#[cfg(feature = "std")]
impl Image {
    /// Loads and validates an image from a file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|_| MetadataError::NotManaged)?;
        let mut image = Self::from_bytes(bytes)?;
        image.path = Some(path.to_path_buf());
        Ok(image)
    }

    /// Loads and validates an image already in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let bytes = bytes.into_boxed_slice();
        let metadata_range = {
            let pe = PeImage::parse(&bytes)?;
            let dir = pe.cli_header.metadata;
            let start = pe.rva_to_offset(dir.rva)?;
            let end = (start + dir.size as usize).min(bytes.len());
            start..end
        };
        // Validate the metadata root eagerly so callers can trust later access.
        Metadata::parse(&bytes[metadata_range.clone()])?;
        Ok(Image { bytes, metadata_range, path: None })
    }

    /// The PE container view.
    pub fn pe(&self) -> PeImage<'_> {
        // Already validated in the constructor.
        PeImage::parse(&self.bytes).expect("image validated at construction")
    }

    /// The metadata tables view.
    pub fn metadata(&self) -> Metadata<'_> {
        Metadata::parse(&self.bytes[self.metadata_range.clone()])
            .expect("metadata validated at construction")
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// Simple name from the `Assembly` table, or the module name as a fallback.
    pub fn assembly_name(&self) -> String {
        let md = self.metadata();
        if md.row_count(TableId::Assembly) > 0 {
            if let Ok(a) = md.assembly(1) {
                return a.name.to_string();
            }
        }
        md.module(1).map(|m| m.name.to_string()).unwrap_or_else(|_| "<unknown>".to_string())
    }

    /// Token of the managed entry point, if this image is an executable.
    pub fn entry_point(&self) -> Option<Token> {
        let pe = self.pe();
        if pe.cli_header.flags & pe::cor_flags::NATIVE_ENTRYPOINT != 0 {
            return None;
        }
        let tok = Token(pe.cli_header.entry_point_token);
        (!tok.is_null()).then_some(tok)
    }

    /// Reads the IL body of a `MethodDef` row, if it has one.
    pub fn method_body(&self, method_row: u32) -> Result<Option<MethodBody<'_>>> {
        let md = self.metadata();
        let m = md.method_def(method_row)?;
        if !m.has_body() {
            return Ok(None);
        }
        let pe = self.pe();
        let start = pe.rva_to_offset(m.rva)?;
        let body = MethodBody::parse(&self.bytes[start..])?;
        Ok(Some(body))
    }
}

#[cfg(feature = "std")]
impl core::fmt::Debug for Image {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Image")
            .field("path", &self.path)
            .field("size", &self.bytes.len())
            .finish()
    }
}
