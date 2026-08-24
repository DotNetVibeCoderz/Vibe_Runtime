//! The board's SPI NOR flash, as an [`ExtMemory`] region.
//!
//! The K210 has **no internal flash at all**. The mask ROM reads the firmware
//! out of an external SPI NOR part on SPI3 and copies it into SRAM, then jumps
//! there — so unlike the STM32 port, writing to this flash cannot possibly be
//! the firmware erasing itself, and an erase does not stall the core. What it
//! does mean is that persistence is only available at all once someone drives
//! that chip, which is what this module is.
//!
//! Only a reserved window is exposed, and addresses passed in are relative to
//! it. That is the guard: the firmware image lives at offset 0 of the flash, so
//! a region that starts megabytes above it cannot reach the code that is
//! running, whatever the caller asks for.
//!
//! The command set is the universal W25Q/GD25Q subset — JEDEC ID, read status,
//! write enable, 4 KB sector erase, 256-byte page program, and a plain 3-byte
//! read. Every 16 Mbit-and-up part in a Maix board's footprint speaks it, so
//! this does not need to identify the die to work; it reads the ID only to
//! report it.
//!
//! **Unproven.** This is written against the datasheets and Kendryte's own
//! driver, and has never run. See the "what to check first" list in
//! `runtime/firmware-k210/README.md`.

use rustclr_hal::extmem::{ExtMemKind, ExtMemory};
use rustclr_hal::spi::{SpiBus, SpiMode};
use rustclr_hal::{HalError, HalResult};

use crate::spi::{K210Spi, SPI3};

// Commands.
const CMD_WRITE_ENABLE: u8 = 0x06;
const CMD_READ_STATUS: u8 = 0x05;
const CMD_READ_DATA: u8 = 0x03;
const CMD_PAGE_PROGRAM: u8 = 0x02;
const CMD_SECTOR_ERASE: u8 = 0x20;
const CMD_JEDEC_ID: u8 = 0x9F;

const STATUS_BUSY: u8 = 1 << 0;
const STATUS_WEL: u8 = 1 << 1;

/// NOR page size — the most a single program command may touch, and it may not
/// cross a page boundary either.
pub const PAGE: u32 = 256;
/// Smallest erasable unit.
pub const SECTOR: u32 = 4096;

/// Most bytes read in one transaction.
///
/// The part itself would stream the whole device from a single command, but the
/// controller counts receive frames in a 16-bit register — so anything past
/// 65536 has to be a fresh command at an advanced address. Chunking well below
/// the ceiling costs one extra 4-byte command header per chunk and keeps the
/// arithmetic obvious. It matters in practice: a restored application is
/// whatever `rustnet flash` last sent, which is not bounded by 64 KB.
const READ_CHUNK: usize = 32 * 1024;

/// A conservative bus clock. The parts are rated past 50 MHz, so this leaves
/// room for [`crate::sysctl::spi_hz`] to be a factor or two out — which it may
/// be, since the SPI3 divider chain is the least well documented corner of the
/// clock tree — and still land inside spec.
const CLOCK_HZ: u32 = 8_000_000;

/// Polling budget for an erase. A 4 KB sector erase is typically 45 ms and
/// specified up to 400 ms; this allows comfortably more without being unbounded.
const BUSY_POLLS: u32 = 40_000_000;

pub struct SpiFlash {
    spi: K210Spi,
    /// Byte offset of the region within the flash device.
    base: u32,
    len: u32,
    configured: bool,
}

impl SpiFlash {
    /// Describe a window of the flash. `base` must be sector-aligned; the
    /// firmware picks it, because only its linker script and flashing recipe
    /// know how much of the device the image occupies.
    pub const fn new(base: u32, len: u32, source_hz: u32) -> Self {
        Self { spi: K210Spi::new(SPI3, source_hz), base, len, configured: false }
    }

    pub fn set_source_hz(&mut self, hz: u32) {
        self.spi.set_source_hz(hz);
        self.configured = false;
    }

    fn ensure_configured(&mut self) -> HalResult<()> {
        if !self.configured {
            self.spi.configure(CLOCK_HZ, SpiMode::Mode0)?;
            self.configured = true;
        }
        Ok(())
    }

    /// The three JEDEC identification bytes: manufacturer, memory type,
    /// capacity. Reported rather than acted on — a part that answers here has a
    /// live bus, which is the first thing worth knowing on new hardware.
    pub fn jedec_id(&mut self) -> HalResult<[u8; 3]> {
        self.ensure_configured()?;
        let mut id = [0u8; 3];
        self.spi.read_after_command(&[CMD_JEDEC_ID], &mut id)?;
        Ok(id)
    }

    /// Capacity in bytes as encoded in the JEDEC ID, if the third byte is one of
    /// the usual power-of-two codes. `0x18` is 16 MB, which is what a Maix Go
    /// carries.
    pub fn capacity_bytes(id: [u8; 3]) -> Option<u32> {
        match id[2] {
            0x14..=0x1A => Some(1u32 << id[2]),
            _ => None,
        }
    }

    fn status(&mut self) -> HalResult<u8> {
        let mut byte = [0u8; 1];
        self.spi.read_after_command(&[CMD_READ_STATUS], &mut byte)?;
        Ok(byte[0])
    }

    fn wait_ready(&mut self) -> HalResult<()> {
        for _ in 0..BUSY_POLLS {
            if self.status()? & STATUS_BUSY == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(HalError::Timeout)
    }

    /// Latch the write-enable bit and confirm it took.
    ///
    /// Checking is not paranoia. A part still finishing an earlier operation,
    /// or one whose status register has write protection set, accepts the
    /// command and simply does not arm — and then the program that follows is
    /// silently a no-op. Reading `WEL` back turns that into an error here rather
    /// than into data which reads back as `0xFF` an hour later.
    fn write_enable(&mut self) -> HalResult<()> {
        self.wait_ready()?;
        self.spi.write_after_command(&[CMD_WRITE_ENABLE], &[])?;
        if self.status()? & STATUS_WEL == 0 {
            return Err(HalError::Bus("flash refused write-enable (write protected?)"));
        }
        Ok(())
    }

    /// Absolute device address for a region-relative one, range-checked.
    fn absolute(&self, addr: u32, len: usize) -> HalResult<u32> {
        let end = addr.checked_add(len as u32).ok_or(HalError::InvalidArgument("range overflow"))?;
        if end > self.len {
            return Err(HalError::InvalidArgument("outside the reserved flash region"));
        }
        Ok(self.base + addr)
    }

    fn command_with_address(command: u8, addr: u32) -> [u8; 4] {
        [command, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]
    }
}

impl ExtMemory for SpiFlash {
    fn kind(&self) -> ExtMemKind {
        ExtMemKind::QspiFlash
    }

    fn size(&self) -> u32 {
        self.len
    }

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> HalResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let device = self.absolute(addr, buf.len())?;
        self.ensure_configured()?;
        self.wait_ready()?;

        let mut at = device;
        for chunk in buf.chunks_mut(READ_CHUNK) {
            let command = Self::command_with_address(CMD_READ_DATA, at);
            self.spi.read_after_command(&command, chunk)?;
            at += chunk.len() as u32;
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> HalResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        let device = self.absolute(addr, data.len())?;
        self.ensure_configured()?;

        // A page program may not cross a page boundary — the address wraps
        // within the page instead of advancing, so a naive long write corrupts
        // the start of the same page rather than filling the next one.
        let mut at = device;
        let mut cursor = 0usize;
        while cursor < data.len() {
            let room = (PAGE - (at % PAGE)) as usize;
            let chunk = room.min(data.len() - cursor);
            self.write_enable()?;
            let command = Self::command_with_address(CMD_PAGE_PROGRAM, at);
            self.spi.write_after_command(&command, &data[cursor..cursor + chunk])?;
            self.wait_ready()?;
            at += chunk as u32;
            cursor += chunk;
        }
        Ok(())
    }

    fn erase(&mut self, addr: u32, len: u32) -> HalResult<()> {
        if len == 0 {
            return Ok(());
        }
        // Round outwards to whole sectors: erasing is only defined per sector,
        // and a caller asking for less means "at least this". Saturating,
        // because the range check happens below and must not be pre-empted by
        // an overflow up here.
        let start = addr - (addr % SECTOR);
        let end = addr.saturating_add(len).saturating_add(SECTOR - 1) / SECTOR * SECTOR;
        let device_start = self.absolute(start, (end - start) as usize)?;
        self.ensure_configured()?;

        let mut at = device_start;
        while at < device_start + (end - start) {
            self.write_enable()?;
            let command = Self::command_with_address(CMD_SECTOR_ERASE, at);
            self.spi.write_after_command(&command, &[])?;
            self.wait_ready()?;
            at += SECTOR;
        }
        Ok(())
    }

    fn sector_size(&self) -> u32 {
        SECTOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_relative_to_the_region() {
        let flash = SpiFlash::new(0x00FC_0000, 0x0004_0000, 100_000_000);
        assert_eq!(flash.absolute(0, 4).unwrap(), 0x00FC_0000);
        assert_eq!(flash.absolute(0x1000, 16).unwrap(), 0x00FC_1000);
    }

    /// The point of the window: nothing a caller passes can name the firmware
    /// image sitting at offset 0 of the device.
    #[test]
    fn a_read_past_the_region_is_refused_rather_than_clamped() {
        let flash = SpiFlash::new(0x00FC_0000, 0x0004_0000, 100_000_000);
        assert!(flash.absolute(0x0004_0000, 1).is_err());
        assert!(flash.absolute(0x0003_FFFF, 2).is_err());
        assert!(flash.absolute(0x0003_FFFF, 1).is_ok());
        assert!(flash.absolute(u32::MAX, 1).is_err());
    }

    #[test]
    fn command_addresses_go_out_big_endian() {
        assert_eq!(
            SpiFlash::command_with_address(CMD_SECTOR_ERASE, 0x00FC_1234),
            [0x20, 0xFC, 0x12, 0x34]
        );
    }

    /// A restored application can be any size `rustnet flash` accepted, and the
    /// controller's frame counter tops out at 65536 — so a long read has to
    /// become several commands rather than one truncated one.
    #[test]
    fn a_long_read_is_split_below_the_controllers_frame_ceiling() {
        assert!(READ_CHUNK <= crate::spi::MAX_RECEIVE_FRAMES);
        let buf = [0u8; 100 * 1024];
        let mut chunks = 0;
        let mut total = 0;
        for chunk in buf.chunks(READ_CHUNK) {
            assert!(chunk.len() <= crate::spi::MAX_RECEIVE_FRAMES);
            chunks += 1;
            total += chunk.len();
        }
        assert!(chunks > 1, "a 100 KB read should not go out as one transfer");
        assert_eq!(total, buf.len());
    }

    #[test]
    fn jedec_capacity_decodes_the_usual_codes() {
        // W25Q128: manufacturer 0xEF, type 0x40, capacity code 0x18 = 16 MB.
        assert_eq!(SpiFlash::capacity_bytes([0xEF, 0x40, 0x18]), Some(16 * 1024 * 1024));
        assert_eq!(SpiFlash::capacity_bytes([0xC8, 0x40, 0x17]), Some(8 * 1024 * 1024));
        // A dead bus reads all ones or all zeroes; neither is a capacity.
        assert_eq!(SpiFlash::capacity_bytes([0xFF, 0xFF, 0xFF]), None);
        assert_eq!(SpiFlash::capacity_bytes([0x00, 0x00, 0x00]), None);
    }
}
