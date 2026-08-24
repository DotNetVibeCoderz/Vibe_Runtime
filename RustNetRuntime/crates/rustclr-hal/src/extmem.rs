use crate::HalResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtMemKind {
    /// Memory-mapped or command-mode QSPI NOR flash (erase before write).
    QspiFlash,
    /// External SDRAM (byte-addressable RAM, no erase).
    Sdram,
}

/// External memory device hanging off the MCU (QSPI flash, SDRAM, ...).
pub trait ExtMemory: Send {
    fn kind(&self) -> ExtMemKind;
    fn size(&self) -> u32;
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> HalResult<()>;
    /// For QSPI flash the range must have been erased first (bits only go
    /// 1 -> 0); SDRAM writes freely.
    fn write(&mut self, addr: u32, data: &[u8]) -> HalResult<()>;
    /// Erase to 0xFF. `NotSupported` for SDRAM. `len` rounds up to the
    /// device's sector size.
    fn erase(&mut self, addr: u32, len: u32) -> HalResult<()>;
    fn sector_size(&self) -> u32;
}
