use crate::HalResult;

pub trait AdcChannel: Send {
    /// Raw reading at the channel's native resolution.
    fn read_raw(&mut self) -> HalResult<u16>;
    /// Native resolution in bits (e.g. 12 for a 12-bit ADC).
    fn resolution_bits(&self) -> u8;
    /// Reading converted to millivolts.
    fn read_millivolts(&mut self) -> HalResult<u32> {
        let raw = self.read_raw()? as u32;
        let max = (1u32 << self.resolution_bits()) - 1;
        Ok(raw * 3300 / max)
    }
}
