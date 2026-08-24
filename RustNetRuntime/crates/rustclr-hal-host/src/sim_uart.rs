use rustclr_hal::uart::{Uart, UartConfig};
use rustclr_hal::HalResult;
use std::collections::VecDeque;

/// Simulated UART. Port 0 is wired in loopback (tx feeds rx); other ports
/// expose `inject`/`drain` so tests and the firmware can bridge them.
pub struct SimUart {
    pub(crate) loopback: bool,
    pub(crate) rx: VecDeque<u8>,
    pub(crate) tx: VecDeque<u8>,
    pub(crate) config: UartConfig,
}

impl SimUart {
    pub(crate) fn new(loopback: bool) -> Self {
        Self { loopback, rx: VecDeque::new(), tx: VecDeque::new(), config: UartConfig::default() }
    }
}

impl Uart for SimUart {
    fn configure(&mut self, config: UartConfig) -> HalResult<()> {
        self.config = config;
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> HalResult<usize> {
        if self.loopback {
            self.rx.extend(data);
        } else {
            self.tx.extend(data);
        }
        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> HalResult<usize> {
        let n = buf.len().min(self.rx.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.rx.pop_front().unwrap();
        }
        Ok(n)
    }

    fn bytes_available(&mut self) -> HalResult<usize> {
        Ok(self.rx.len())
    }

    fn flush(&mut self) -> HalResult<()> {
        Ok(())
    }
}
