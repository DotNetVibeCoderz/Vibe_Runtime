use crate::HalResult;

pub trait I2cBus: Send {
    fn set_frequency(&mut self, hz: u32) -> HalResult<()>;
    fn write(&mut self, addr: u8, data: &[u8]) -> HalResult<()>;
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> HalResult<()>;
    fn write_read(&mut self, addr: u8, data: &[u8], buf: &mut [u8]) -> HalResult<()> {
        self.write(addr, data)?;
        self.read(addr, buf)
    }
    /// Probe for a device by issuing a zero-length write.
    fn probe(&mut self, addr: u8) -> bool {
        self.write(addr, &[]).is_ok()
    }
}
