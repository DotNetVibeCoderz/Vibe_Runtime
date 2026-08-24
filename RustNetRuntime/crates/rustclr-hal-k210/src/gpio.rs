//! GPIOHS — the 32-channel high-speed GPIO block.
//!
//! Two things make this different from a conventional MCU's GPIO. First, a
//! channel is not a pin: GPIOHS offers 32 *channels* which FPIOA can route to
//! any of the 48 pads, so `Board::gpio(pad)` has to allocate a channel and mux
//! it before there is anything to drive. [`crate::K210Board`] owns that
//! bookkeeping; this module is only the register half.
//!
//! Second, every register here is a single 32-bit word with one bit per
//! channel — `output_val`, `output_en`, `input_val` and so on — rather than a
//! per-port block. That makes a pin write one read-modify-write, and it makes
//! the whole block's state readable in a handful of loads.
//!
//! Open-drain is done in software. The pad has no open-drain mode, so a
//! `Low` drives and a `High` releases the pad to float, which is what the
//! bit-banged buses (1-Wire, a hand-rolled I²C) actually need from it.

use alloc::boxed::Box;

use rustclr_hal::gpio::{Edge, GpioPin, Level, PinMode};
use rustclr_hal::{HalError, HalResult};

use crate::reg;

const GPIOHS_BASE: usize = 0x3800_1000;

const INPUT_VAL: usize = GPIOHS_BASE + 0x00;
const INPUT_EN: usize = GPIOHS_BASE + 0x04;
const OUTPUT_EN: usize = GPIOHS_BASE + 0x08;
const OUTPUT_VAL: usize = GPIOHS_BASE + 0x0C;
const RISE_IE: usize = GPIOHS_BASE + 0x18;
const RISE_IP: usize = GPIOHS_BASE + 0x1C;
const FALL_IE: usize = GPIOHS_BASE + 0x20;
const FALL_IP: usize = GPIOHS_BASE + 0x24;

/// GPIOHS channels.
pub const CHANNEL_COUNT: usize = 32;

/// PLIC source number for GPIOHS channel 0; the channels run consecutively
/// from there. Named for whoever wires up [`GpioPin::on_edge`].
pub const IRQ_GPIOHS0: u32 = 34;

/// One high-speed GPIO channel, already routed to a pad.
pub struct K210Pin {
    /// FPIOA pad this channel is muxed to. Kept for reporting and for the
    /// pull writes, which live in FPIOA rather than in GPIOHS.
    pub(crate) pad: u8,
    pub(crate) channel: u8,
    mode: PinMode,
}

impl K210Pin {
    pub(crate) const fn new(channel: u8) -> Self {
        Self { pad: u8::MAX, channel, mode: PinMode::Input }
    }

    #[inline]
    fn bit(&self) -> u32 {
        1 << self.channel
    }

    pub fn pad(&self) -> u8 {
        self.pad
    }

    /// Drive the pad, or release it to float.
    fn drive(&self, enabled: bool) {
        if enabled {
            reg::modify(OUTPUT_EN, 0, self.bit());
        } else {
            reg::modify(OUTPUT_EN, self.bit(), 0);
        }
    }
}

impl GpioPin for K210Pin {
    fn set_mode(&mut self, mode: PinMode) -> HalResult<()> {
        self.mode = mode;

        // Input sensing stays on in every mode. It costs nothing, and it makes
        // an output pin read back the level it is driving — which is what
        // `toggle` and any read-modify-write on a pin depends on.
        reg::modify(INPUT_EN, 0, self.bit());

        match mode {
            PinMode::Output => self.drive(true),
            // Open-drain starts released: the bus is idle until something
            // pulls it, and asserting on entry would be a surprise.
            PinMode::OutputOpenDrain => self.drive(false),
            PinMode::Input | PinMode::InputPullUp | PinMode::InputPullDown => self.drive(false),
        }

        let pull = match mode {
            PinMode::InputPullUp => crate::fpioa::Pull::Up,
            PinMode::InputPullDown => crate::fpioa::Pull::Down,
            _ => crate::fpioa::Pull::None,
        };
        crate::fpioa::set_pull(self.pad, pull);
        Ok(())
    }

    fn write(&mut self, level: Level) -> HalResult<()> {
        if self.mode == PinMode::OutputOpenDrain {
            // Pull low, or let go and let the bus float up.
            match level {
                Level::Low => {
                    reg::modify(OUTPUT_VAL, self.bit(), 0);
                    self.drive(true);
                }
                Level::High => self.drive(false),
            }
            return Ok(());
        }

        match level {
            Level::High => reg::modify(OUTPUT_VAL, 0, self.bit()),
            Level::Low => reg::modify(OUTPUT_VAL, self.bit(), 0),
        }
        Ok(())
    }

    fn read(&mut self) -> HalResult<Level> {
        Ok(if reg::read(INPUT_VAL) & self.bit() != 0 {
            Level::High
        } else {
            Level::Low
        })
    }

    /// Flips `output_val` rather than reading the pad back, so a toggle costs
    /// one read-modify-write and cannot be thrown off by an external driver
    /// fighting the pin.
    fn toggle(&mut self) -> HalResult<()> {
        if self.mode == PinMode::OutputOpenDrain {
            let asserted = reg::read(OUTPUT_EN) & self.bit() != 0;
            return self.write(if asserted { Level::High } else { Level::Low });
        }
        let current = reg::read(OUTPUT_VAL);
        reg::write(OUTPUT_VAL, current ^ self.bit());
        Ok(())
    }

    fn on_edge(&mut self, _edge: Edge, _callback: Box<dyn FnMut(Level) + Send>) -> HalResult<()> {
        // Integration point: GPIOHS has per-channel rise/fall enable and
        // pending bits (RISE_IE/RISE_IP/FALL_IE/FALL_IP above), and each
        // channel raises its own PLIC source at IRQ_GPIOHS0 + channel. What is
        // missing is a home for the callbacks — a static table the firmware's
        // machine-external trap handler can dispatch through, which belongs
        // with the trap handler rather than here.
        Err(HalError::NotSupported)
    }

    fn clear_interrupt(&mut self) -> HalResult<()> {
        // Write-1-to-clear on both pending registers, so this is already
        // correct for whoever fills in `on_edge`.
        reg::write(RISE_IP, self.bit());
        reg::write(FALL_IP, self.bit());
        reg::modify(RISE_IE, self.bit(), 0);
        reg::modify(FALL_IE, self.bit(), 0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_maps_to_its_own_bit() {
        assert_eq!(K210Pin::new(0).bit(), 1);
        assert_eq!(K210Pin::new(31).bit(), 0x8000_0000);
    }

    #[test]
    fn gpiohs_channels_have_consecutive_interrupt_sources() {
        assert_eq!(IRQ_GPIOHS0 + 31, 65);
    }
}
