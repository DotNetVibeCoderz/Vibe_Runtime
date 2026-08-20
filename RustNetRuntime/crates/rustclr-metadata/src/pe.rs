//! PE/COFF container parsing, down to the CLI (COR20) header.
//!
//! We only decode what the runtime actually needs: the section table (to
//! translate RVAs), the CLI header (to find the metadata root and entry point),
//! and enough of the optional header to tell PE32 from PE32+.

#[allow(unused_imports)]
use crate::prelude::*;

use crate::error::{MetadataError, Result};
use crate::reader::Reader;

/// Machine types we recognise, matching the architectures in the requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    I386,
    Amd64,
    Arm,
    Arm64,
    RiscV32,
    RiscV64,
    Unknown(u16),
}

impl Machine {
    pub fn from_raw(v: u16) -> Self {
        match v {
            0x014C => Self::I386,
            0x8664 => Self::Amd64,
            0x01C0 | 0x01C4 => Self::Arm,
            0xAA64 => Self::Arm64,
            0x5032 => Self::RiscV32,
            0x5064 => Self::RiscV64,
            other => Self::Unknown(other),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::I386 => "x86",
            Self::Amd64 => "x64",
            Self::Arm => "arm",
            Self::Arm64 => "arm64",
            Self::RiscV32 => "riscv32",
            Self::RiscV64 => "riscv64",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DataDirectory {
    pub rva: u32,
    pub size: u32,
}

impl DataDirectory {
    pub const fn is_empty(&self) -> bool {
        self.rva == 0 || self.size == 0
    }
}

#[derive(Debug, Clone)]
pub struct Section {
    pub name: [u8; 8],
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub raw_size: u32,
    pub raw_pointer: u32,
    pub characteristics: u32,
}

impl Section {
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(8);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }

    /// True when `rva` falls inside this section's virtual extent.
    fn contains(&self, rva: u32) -> bool {
        // Use the larger of virtual/raw size: linkers may emit virtual_size 0.
        let span = self.virtual_size.max(self.raw_size);
        rva >= self.virtual_address && rva < self.virtual_address.saturating_add(span)
    }
}

/// The CLI header (ECMA-335 II.25.3.3), the bridge from PE into managed land.
#[derive(Debug, Clone, Copy)]
pub struct CliHeader {
    pub cb: u32,
    pub major_runtime_version: u16,
    pub minor_runtime_version: u16,
    pub metadata: DataDirectory,
    pub flags: u32,
    /// Token of the entry point method, or an RVA when `NATIVE_ENTRYPOINT` is set.
    pub entry_point_token: u32,
    pub resources: DataDirectory,
    pub strong_name_signature: DataDirectory,
    pub vtable_fixups: DataDirectory,
}

pub mod cor_flags {
    pub const ILONLY: u32 = 0x0000_0001;
    pub const REQUIRE_32BIT: u32 = 0x0000_0002;
    pub const STRONGNAMESIGNED: u32 = 0x0000_0008;
    pub const NATIVE_ENTRYPOINT: u32 = 0x0000_0010;
    pub const PREFER_32BIT: u32 = 0x0002_0000;
}

/// A parsed PE image. Borrows the whole file buffer.
#[derive(Debug, Clone)]
pub struct PeImage<'a> {
    data: &'a [u8],
    pub machine: Machine,
    pub is_pe32_plus: bool,
    pub image_base: u64,
    pub sections: Vec<Section>,
    pub cli_header: CliHeader,
}

impl<'a> PeImage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);

        // --- MS-DOS stub -----------------------------------------------------
        let mz = r.u16()?;
        if mz != 0x5A4D {
            return Err(MetadataError::BadMagic { what: "MZ", found: mz as u32 });
        }
        r.seek(0x3C);
        let pe_offset = r.u32()? as usize;

        // --- PE signature + COFF header --------------------------------------
        r.seek(pe_offset);
        let sig = r.u32()?;
        if sig != 0x0000_4550 {
            return Err(MetadataError::BadMagic { what: "PE", found: sig });
        }
        let machine = Machine::from_raw(r.u16()?);
        let num_sections = r.u16()? as usize;
        r.skip(12)?; // timestamp, symbol table pointer, symbol count
        let opt_header_size = r.u16()? as usize;
        r.skip(2)?; // characteristics

        let opt_start = r.position();

        // --- Optional header --------------------------------------------------
        let magic = r.u16()?;
        let is_pe32_plus = match magic {
            0x010B => false,
            0x020B => true,
            other => return Err(MetadataError::UnsupportedPeMagic(other)),
        };
        r.skip(if is_pe32_plus { 22 } else { 26 })?;
        let image_base = if is_pe32_plus { r.u64()? } else { r.u32()? as u64 };

        // Skip the Windows-specific fields up to NumberOfRvaAndSizes:
        // alignment (8) + versions (12) + Win32VersionValue/sizes/checksum (16)
        // + subsystem and DLL characteristics (4) + the four stack/heap sizes
        // (16 on PE32, 32 on PE32+) + LoaderFlags (4).
        r.skip(if is_pe32_plus { 76 } else { 60 })?;
        let num_dirs = r.u32()? as usize;

        let mut dirs = Vec::with_capacity(num_dirs);
        for _ in 0..num_dirs {
            dirs.push(DataDirectory { rva: r.u32()?, size: r.u32()? });
        }

        // --- Section table ----------------------------------------------------
        r.seek(opt_start + opt_header_size);
        let mut sections = Vec::with_capacity(num_sections);
        for _ in 0..num_sections {
            let raw = r.bytes(8)?;
            let mut name = [0u8; 8];
            name.copy_from_slice(raw);
            let virtual_size = r.u32()?;
            let virtual_address = r.u32()?;
            let raw_size = r.u32()?;
            let raw_pointer = r.u32()?;
            r.skip(12)?; // relocations / line numbers pointers and counts
            let characteristics = r.u32()?;
            sections.push(Section {
                name,
                virtual_size,
                virtual_address,
                raw_size,
                raw_pointer,
                characteristics,
            });
        }

        // --- CLI header (data directory 14) ----------------------------------
        let cli_dir = dirs.get(14).copied().unwrap_or(DataDirectory { rva: 0, size: 0 });
        if cli_dir.is_empty() {
            return Err(MetadataError::NotManaged);
        }

        let mut partial = PeImage {
            data,
            machine,
            is_pe32_plus,
            image_base,
            sections,
            cli_header: CliHeader {
                cb: 0,
                major_runtime_version: 0,
                minor_runtime_version: 0,
                metadata: DataDirectory { rva: 0, size: 0 },
                flags: 0,
                entry_point_token: 0,
                resources: DataDirectory { rva: 0, size: 0 },
                strong_name_signature: DataDirectory { rva: 0, size: 0 },
                vtable_fixups: DataDirectory { rva: 0, size: 0 },
            },
        };

        let off = partial.rva_to_offset(cli_dir.rva)?;
        let mut c = Reader::at(data, off);
        partial.cli_header = CliHeader {
            cb: c.u32()?,
            major_runtime_version: c.u16()?,
            minor_runtime_version: c.u16()?,
            metadata: DataDirectory { rva: c.u32()?, size: c.u32()? },
            flags: c.u32()?,
            entry_point_token: c.u32()?,
            resources: DataDirectory { rva: c.u32()?, size: c.u32()? },
            strong_name_signature: DataDirectory { rva: c.u32()?, size: c.u32()? },
            vtable_fixups: {
                c.skip(8)?; // CodeManagerTable is reserved and always zero
                DataDirectory { rva: c.u32()?, size: c.u32()? }
            },
        };

        Ok(partial)
    }

    /// Translates a relative virtual address into a file offset.
    pub fn rva_to_offset(&self, rva: u32) -> Result<usize> {
        self.sections
            .iter()
            .find(|s| s.contains(rva))
            .map(|s| (rva - s.virtual_address + s.raw_pointer) as usize)
            .ok_or(MetadataError::RvaNotMapped(rva))
    }

    /// Returns `len` bytes starting at `rva`.
    pub fn slice_at_rva(&self, rva: u32, len: usize) -> Result<&'a [u8]> {
        let off = self.rva_to_offset(rva)?;
        self.data
            .get(off..off + len)
            .ok_or(MetadataError::UnexpectedEof { offset: off, needed: len })
    }

    /// Returns everything from `rva` to the end of its containing section.
    pub fn slice_from_rva(&self, rva: u32) -> Result<&'a [u8]> {
        let section = self
            .sections
            .iter()
            .find(|s| s.contains(rva))
            .ok_or(MetadataError::RvaNotMapped(rva))?;
        let off = (rva - section.virtual_address + section.raw_pointer) as usize;
        let end = (section.raw_pointer + section.raw_size) as usize;
        let end = end.min(self.data.len());
        self.data
            .get(off..end)
            .ok_or(MetadataError::UnexpectedEof { offset: off, needed: 0 })
    }

    pub const fn data(&self) -> &'a [u8] {
        self.data
    }

    pub const fn is_il_only(&self) -> bool {
        self.cli_header.flags & cor_flags::ILONLY != 0
    }
}
