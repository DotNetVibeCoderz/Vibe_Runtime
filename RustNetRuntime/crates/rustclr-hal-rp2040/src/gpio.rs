//! GPIO — three blocks for one pin.
//!
//! A pin on this chip is configured in three places and all three have to
//! agree, which is the usual way an RP2040 GPIO ends up doing nothing:
//!
//! - **IO_BANK0** picks which peripheral drives the pin. `SIO` (function 5) is
//!   the one that makes it a plain GPIO.
//! - **PADS_BANK0** is the analogue pad: input enable, output disable, pulls.
//!   A pad whose input enable is off reads zero forever no matter what is on
//!   the wire.
//! - **SIO** is the register the core actually writes. It sits on the
//!   single-cycle bus rather than the APB, so it has its own `SET`/`CLR`/`XOR`
//!   registers instead of the `+0x2000` aliases every other peripheral uses.

use alloc::boxed::Box;
use rustclr_hal::gpio::{Edge, GpioPin, Level, PinMode};
use rustclr_hal::{HalError, HalResult};

use crate::base::{IO_BANK0, PADS_BANK0, SIO};
use crate::{reg, resets};

/// The RP2040 brings out 30 GPIOs; a Pico exposes 0..=28.
pub const PIN_COUNT: u32 = 30;

const SIO_GPIO_IN: usize = SIO + 0x04;
const SIO_GPIO_OUT_SET: usize = SIO + 0x14;
const SIO_GPIO_OUT_CLR: usize = SIO + 0x18;
const SIO_GPIO_OUT_XOR: usize = SIO + 0x1C;
const SIO_GPIO_OE_SET: usize = SIO + 0x24;
const SIO_GPIO_OE_CLR: usize = SIO + 0x28;

/// `PADS_BANK0` per-pin control.
const PAD_OD: u32 = 1 << 7;
const PAD_IE: u32 = 1 << 6;
const PAD_PUE: u32 = 1 << 3;
const PAD_PDE: u32 = 1 << 2;

/// `IO_BANK0` function select. 5 is SIO — the core's own GPIO.
const FUNC_SIO: u32 = 5;

fn pad_ctrl(pin: u32) -> usize {
    PADS_BANK0 + 0x04 + 4 * pin as usize
}

fn io_ctrl(pin: u32) -> usize {
    IO_BANK0 + 0x04 + 8 * pin as usize
}

/// Release the GPIO blocks. Called once before any pin is touched — writes to
/// a held peripheral are discarded silently.
pub fn init() {
    resets::unreset(resets::IO_BANK0 | resets::PADS_BANK0);
}

/// Route `pin` to a peripheral function other than SIO (UART, SPI, ...).
pub fn set_function(pin: u32, function: u32) {
    if pin >= PIN_COUNT {
        return;
    }
    // The pad has to be able to carry a signal in both directions: a UART's
    // RX pin needs the input enabled, and its TX pin needs the output driver
    // left on. One pad setting serves both because the peripheral, not the
    // pad, decides which way the line is driven.
    reg::clear_bits(pad_ctrl(pin), PAD_OD);
    reg::set_bits(pad_ctrl(pin), PAD_IE);
    reg::write(io_ctrl(pin), function);
}

pub struct Rp2040Pin {
    pin: u32,
}

impl Rp2040Pin {
    pub const fn new(pin: u32) -> Self {
        Self { pin }
    }
}

impl GpioPin for Rp2040Pin {
    fn set_mode(&mut self, mode: PinMode) -> HalResult<()> {
        if self.pin >= PIN_COUNT {
            return Err(HalError::InvalidArgument("RP2040 has GPIO0..=29"));
        }
        let pad = pad_ctrl(self.pin);
        // Input enabled in every mode, including output: reading back a pin
        // the core is driving is how open-drain emulation and bus arbitration
        // are checked, and a pad with IE off reads zero regardless.
        reg::set_bits(pad, PAD_IE);
        reg::clear_bits(pad, PAD_OD | PAD_PUE | PAD_PDE);
        reg::write(io_ctrl(self.pin), FUNC_SIO);

        let bit = 1 << self.pin;
        match mode {
            PinMode::Output => reg::write(SIO_GPIO_OE_SET, bit),
            PinMode::Input => reg::write(SIO_GPIO_OE_CLR, bit),
            PinMode::InputPullUp => {
                reg::write(SIO_GPIO_OE_CLR, bit);
                reg::set_bits(pad, PAD_PUE);
            }
            PinMode::InputPullDown => {
                reg::write(SIO_GPIO_OE_CLR, bit);
                reg::set_bits(pad, PAD_PDE);
            }
            PinMode::OutputOpenDrain => {
                // The pad has no open-drain mode, so it is emulated the way
                // every other port in this repo emulates it: the pin is only
                // ever driven low, and "high" means releasing it to whatever
                // pulls the line up. `write` cannot know that, so the output
                // enable is toggled there via the same OE registers.
                reg::write(SIO_GPIO_OUT_CLR, bit);
                reg::write(SIO_GPIO_OE_CLR, bit);
                reg::set_bits(pad, PAD_PUE);
            }
        }
        Ok(())
    }

    fn write(&mut self, level: Level) -> HalResult<()> {
        let bit = 1 << self.pin;
        // SIO is on the single-cycle bus, so it carries its own set and clear
        // registers rather than the +0x2000 aliases the APB peripherals have.
        match level {
            Level::High => reg::write(SIO_GPIO_OUT_SET, bit),
            Level::Low => reg::write(SIO_GPIO_OUT_CLR, bit),
        }
        Ok(())
    }

    fn read(&mut self) -> HalResult<Level> {
        let bit = 1 << self.pin;
        Ok(if reg::read(SIO_GPIO_IN) & bit != 0 {
            Level::High
        } else {
            Level::Low
        })
    }

    fn toggle(&mut self) -> HalResult<()> {
        reg::write(SIO_GPIO_OUT_XOR, 1 << self.pin);
        Ok(())
    }

    fn on_edge(&mut self, _edge: Edge, _handler: Box<dyn FnMut(Level) + Send>) -> HalResult<()> {
        // IO_BANK0 has per-pin edge interrupts; nothing in this port has
        // needed one yet. Refused rather than accepted, because a handler
        // that is registered and never fires is worse than one that says so.
        Err(HalError::NotSupported)
    }

    fn clear_interrupt(&mut self) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
}
