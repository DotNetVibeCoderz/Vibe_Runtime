
#[allow(unused_imports)]
use alloc::{vec, vec::Vec};
use crate::HalResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpiMode {
    Mode0,
    Mode1,
    Mode2,
    Mode3,
}

pub trait SpiBus: Send {
    fn configure(&mut self, hz: u32, mode: SpiMode) -> HalResult<()>;
    /// Full-duplex transfer: `tx` is shifted out while `rx` fills.
    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) -> HalResult<()>;
    fn write(&mut self, tx: &[u8]) -> HalResult<()> {
        let mut sink = vec![0u8; tx.len()];
        self.transfer(tx, &mut sink)
    }
    fn read(&mut self, rx: &mut [u8]) -> HalResult<()> {
        let tx = vec![0u8; rx.len()];
        self.transfer(&tx, rx)
    }
}
