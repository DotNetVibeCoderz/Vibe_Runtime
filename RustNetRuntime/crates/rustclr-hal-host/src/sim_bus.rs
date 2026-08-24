use rustclr_hal::i2c::I2cBus;
use rustclr_hal::spi::{SpiBus, SpiMode};
use rustclr_hal::{HalError, HalResult};
use std::collections::HashMap;

/// Implement this to emulate an I2C peripheral (sensor, display, ...).
pub trait I2cDevice: Send {
    fn write(&mut self, data: &[u8]);
    fn read(&mut self, buf: &mut [u8]);
}

pub struct SimI2cBus {
    pub(crate) devices: HashMap<u8, Box<dyn I2cDevice>>,
    pub(crate) frequency: u32,
}

impl SimI2cBus {
    pub(crate) fn new() -> Self {
        Self { devices: HashMap::new(), frequency: 100_000 }
    }
}

impl I2cBus for SimI2cBus {
    fn set_frequency(&mut self, hz: u32) -> HalResult<()> {
        self.frequency = hz;
        Ok(())
    }

    fn write(&mut self, addr: u8, data: &[u8]) -> HalResult<()> {
        match self.devices.get_mut(&addr) {
            Some(dev) => {
                dev.write(data);
                Ok(())
            }
            None => Err(HalError::Bus("NACK: no device at address")),
        }
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> HalResult<()> {
        match self.devices.get_mut(&addr) {
            Some(dev) => {
                dev.read(buf);
                Ok(())
            }
            None => Err(HalError::Bus("NACK: no device at address")),
        }
    }
}

/// Implement this to emulate an SPI peripheral.
pub trait SpiDevice: Send {
    /// Full-duplex byte exchange.
    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]);
}

pub struct SimSpiBus {
    pub(crate) device: Option<Box<dyn SpiDevice>>,
    pub(crate) hz: u32,
    pub(crate) mode: SpiMode,
    /// Every byte ever shifted out, for test assertions.
    pub(crate) tx_log: Vec<u8>,
}

impl SimSpiBus {
    pub(crate) fn new() -> Self {
        Self { device: None, hz: 1_000_000, mode: SpiMode::Mode0, tx_log: Vec::new() }
    }
}

impl SpiBus for SimSpiBus {
    fn configure(&mut self, hz: u32, mode: SpiMode) -> HalResult<()> {
        self.hz = hz;
        self.mode = mode;
        Ok(())
    }

    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) -> HalResult<()> {
        self.tx_log.extend_from_slice(tx);
        if let Some(dev) = self.device.as_mut() {
            dev.transfer(tx, rx);
        }
        Ok(())
    }
}
