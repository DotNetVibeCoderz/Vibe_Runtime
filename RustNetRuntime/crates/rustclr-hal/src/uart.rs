use crate::HalResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Debug, Clone, Copy)]
pub struct UartConfig {
    pub baud: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: Parity,
}

impl Default for UartConfig {
    fn default() -> Self {
        Self { baud: 115_200, data_bits: 8, stop_bits: 1, parity: Parity::None }
    }
}

pub trait Uart: Send {
    fn configure(&mut self, config: UartConfig) -> HalResult<()>;
    fn write(&mut self, data: &[u8]) -> HalResult<usize>;
    /// Non-blocking read; returns number of bytes actually read (may be 0).
    fn read(&mut self, buf: &mut [u8]) -> HalResult<usize>;
    fn bytes_available(&mut self) -> HalResult<usize>;
    fn flush(&mut self) -> HalResult<()>;
}
