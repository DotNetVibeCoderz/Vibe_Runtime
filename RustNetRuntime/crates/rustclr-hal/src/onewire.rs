
#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use crate::HalResult;

/// Dallas/Maxim 1-Wire bus master.
///
/// Timing-critical bit banging lives below this trait (chip crates use a
/// hardware peripheral or cycle-counted GPIO; the simulator is
/// instantaneous), so drivers written against it are portable.
pub trait OneWireBus: Send {
    /// Bus reset. Returns true when at least one slave answered presence.
    fn reset(&mut self) -> HalResult<bool>;
    fn write_byte(&mut self, byte: u8) -> HalResult<()>;
    fn read_byte(&mut self) -> HalResult<u8>;
    fn write(&mut self, data: &[u8]) -> HalResult<()> {
        for b in data {
            self.write_byte(*b)?;
        }
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8]) -> HalResult<()> {
        for b in buf.iter_mut() {
            *b = self.read_byte()?;
        }
        Ok(())
    }
    /// Enumerate all slave ROM codes on the bus (SEARCH ROM).
    fn search(&mut self) -> HalResult<Vec<u64>>;
    /// Address one slave (MATCH ROM + 8 ROM bytes).
    fn select(&mut self, rom: u64) -> HalResult<()>;
    /// Address all slaves at once (SKIP ROM).
    fn skip(&mut self) -> HalResult<()>;
}

/// Dallas CRC-8 (poly 0x31 reflected = 0x8C), used by ROM codes and
/// DS18B20 scratchpads.
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for byte in data {
        let mut b = *byte;
        for _ in 0..8 {
            let mix = (crc ^ b) & 0x01;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            b >>= 1;
        }
    }
    crc
}
