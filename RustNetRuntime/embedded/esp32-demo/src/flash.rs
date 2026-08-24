//! The `storage` partition as a [`rustclr_hal::extmem::ExtMemory`], so an
//! ESP32 board can keep applications across a power cycle.
//!
//! # The partition is found, not assumed
//!
//! The offset and size come from the partition table the bootloader already
//! reads at `0x8000`, looked up by type. Hard-coding `0x210000` would work
//! until somebody edited `partitions.csv` — and the failure mode of a wrong
//! flash offset is not an error message, it is a firmware that overwrites
//! itself and a board that no longer boots.
//!
//! Every access is then **bounds-checked against the partition and translated
//! into it**, so an address the store computes cannot reach the `factory`
//! partition however wrong it is. That check is the reason this type exists at
//! all rather than handing [`esp_storage::FlashStorage`] straight to the
//! filesystem.
//!
//! # Why `NorFlash` rather than `Storage`
//!
//! `embedded_storage::Storage::write` presents flash as if it were RAM: it
//! reads the surrounding sector, erases it, and writes the whole thing back.
//! That is the wrong shape for an append-only log — writing one record would
//! erase and rewrite every record sharing its sector, which is both slow and a
//! power-loss window over data that was previously fine.
//!
//! [`rustclr_flashfs`] wants what NOR actually offers: explicit erase, and
//! writes that only clear bits. `NorFlash` is that, and it is what
//! [`rustclr_hal::extmem::ExtMemory`] describes.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_storage::FlashStorage;

use rustclr_hal::extmem::{ExtMemKind, ExtMemory};
use rustclr_hal::{HalError, HalResult};

/// Where the ESP-IDF partition table lives, and how much of it to read.
///
/// Fixed by the bootloader, not by us: the second-stage bootloader looks here
/// too, which is why the number is the same in `partitions.csv`.
const PARTITION_TABLE_BYTES: usize = 3072;

/// A bounded window onto one flash partition.
pub struct EspFlash {
    flash: FlashStorage<'static>,
    /// Absolute offset of the partition in the flash device.
    base: u32,
    /// Its length. Every address handed in is checked against this.
    len: u32,
}

impl EspFlash {
    /// Find the `storage` partition and open a window onto it.
    ///
    /// `None` when there is no such partition — a board flashed without a
    /// partition table, or with one that has no data/fat entry. Refused rather
    /// than falling back to a guessed offset, because a guess that is wrong
    /// writes over the firmware.
    pub fn open(flash: esp_hal::peripherals::FLASH<'static>) -> Option<Self> {
        use esp_bootloader_esp_idf::partitions;

        let mut storage = FlashStorage::new(flash);
        let mut buffer = [0u8; PARTITION_TABLE_BYTES];
        let table = partitions::read_partition_table(&mut storage, &mut buffer).ok()?;

        let entry = table
            .find_partition(partitions::PartitionType::Data(
                partitions::DataPartitionSubType::Fat,
            ))
            .ok()??;

        let base = entry.offset();
        let len = entry.len();

        // The partition table itself lives below the first partition, and the
        // firmware below that. A window that started at zero would be a window
        // over the bootloader.
        if base == 0 || len == 0 {
            return None;
        }

        Some(Self { flash: storage, base, len })
    }

    /// Total capacity of the window.
    pub fn capacity(&self) -> u32 {
        self.len
    }

    /// Translate a partition-relative address, or refuse it.
    ///
    /// The whole safety argument of this module is in these four lines: an
    /// address is inside the partition or it is an error, and what reaches the
    /// flash driver is always `base + offset`.
    fn absolute(&self, addr: u32, len: u32) -> HalResult<u32> {
        let end = addr.checked_add(len).ok_or(HalError::InvalidArgument(
            "flash access length overflows its address",
        ))?;
        if end > self.len {
            return Err(HalError::InvalidArgument(
                "flash access outside the storage partition",
            ));
        }
        Ok(self.base + addr)
    }
}

impl ExtMemory for EspFlash {
    fn kind(&self) -> ExtMemKind {
        ExtMemKind::QspiFlash
    }

    fn size(&self) -> u32 {
        self.len
    }

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> HalResult<()> {
        let at = self.absolute(addr, buf.len() as u32)?;
        // Named explicitly: `FlashStorage` implements both `ReadNorFlash` and
        // `ReadStorage`, and they have the same signature. The NOR one is the
        // right half of the pair this module uses for writing.
        ReadNorFlash::read(&mut self.flash, at, buf)
            .map_err(|_| HalError::Bus("flash read failed"))
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> HalResult<()> {
        // Four-byte words, and the driver enforces it. `rustclr-flashfs` pads
        // every record to a word boundary, so this holds for the store; it is
        // checked here anyway because the error it produces otherwise names
        // an alignment nobody chose.
        if addr % FlashStorage::WORD_SIZE != 0 || data.len() as u32 % FlashStorage::WORD_SIZE != 0 {
            return Err(HalError::InvalidArgument(
                "ESP32 flash writes are four-byte aligned and four-byte sized",
            ));
        }
        let at = self.absolute(addr, data.len() as u32)?;
        NorFlash::write(&mut self.flash, at, data)
            .map_err(|_| HalError::Bus("flash write failed"))
    }

    fn erase(&mut self, addr: u32, len: u32) -> HalResult<()> {
        // Erase clears a whole sector whatever is asked for, so the range is
        // widened to sector boundaries *before* it is bounds-checked. Checking
        // the request and then erasing wider than it would be the one way this
        // module could still reach outside its partition.
        let sector = FlashStorage::SECTOR_SIZE;
        let start = addr - (addr % sector);
        let end = addr
            .checked_add(len)
            .map(|e| e.div_ceil(sector) * sector)
            .ok_or(HalError::InvalidArgument("erase length overflows its address"))?;
        let at = self.absolute(start, end - start)?;
        NorFlash::erase(&mut self.flash, at, at + (end - start))
            .map_err(|_| HalError::Bus("flash erase failed"))
    }

    fn sector_size(&self) -> u32 {
        FlashStorage::SECTOR_SIZE
    }
}
