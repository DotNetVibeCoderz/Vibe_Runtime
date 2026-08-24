//! The seam between RustCLR and the hardware under it.
//!
//! Traits for GPIO, I2C, SPI, UART, I2S, PWM, ADC, CAN, 1-Wire, RTC, watchdog,
//! external memory, network interfaces, timed signals and cameras. A chip crate
//! (ESP32, STM32, RP2040, K210) or the host simulator implements them; the
//! runtime's native bindings, the drivers and the firmware services above talk
//! to nothing else.
//!
//! # Why traits and not a chip
//!
//! `requirements.md` says the collector and the scheduler must be swappable
//! modules rather than wired into the runtime. A board is the third such
//! module: [`Board`] hangs off the interpreter the way the host and the
//! collector do, so the same `RustNet.Hal.Gpio.Write(2, true)` reaches a real
//! pin on an ESP32, a simulated one on a desktop, and an error that says
//! `not supported` on a board without that pin — decided at firmware build
//! time, not by anything the program says.
//!
//! # Refusing rather than guessing
//!
//! Every method returns [`HalResult`]. A pin a chip does not have, a bus that
//! is busy, a frequency out of range: each is a distinct error a program can
//! act on. Nothing here silently substitutes a nearby pin or clamps a value
//! into range, because a peripheral that quietly did something else is the
//! kind of plausible wrong answer this runtime refuses everywhere.
//!
//! Adapted from the Gravicode RustNet reference HAL.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;


pub mod gpio;
pub mod i2c;
pub mod spi;
pub mod uart;
pub mod i2s;
pub mod pwm;
pub mod adc;
pub mod power;
pub mod delay;
pub mod can;
pub mod onewire;
pub mod rtc;
pub mod watchdog;
pub mod extmem;
pub mod netif;
pub mod signal;
pub mod camera;

/// Unified HAL error type shared by all buses/peripherals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HalError {
    /// Peripheral or pin does not exist on this chip variant.
    NotSupported,
    /// Peripheral is busy or locked by another owner.
    Busy,
    /// Communication timed out.
    Timeout,
    /// Bus-level failure (NACK, framing error, ...).
    Bus(&'static str),
    /// Argument out of range (bad pin number, bad frequency, ...).
    InvalidArgument(&'static str),
}

impl core::fmt::Display for HalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HalError::NotSupported => write!(f, "not supported"),
            HalError::Busy => write!(f, "busy"),
            HalError::Timeout => write!(f, "timeout"),
            HalError::Bus(m) => write!(f, "bus error: {m}"),
            HalError::InvalidArgument(m) => write!(f, "invalid argument: {m}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for HalError {}

/// Bytes per second, for buses whose speed a program may set.
pub type Hertz = u32;

pub type HalResult<T> = Result<T, HalError>;

/// Everything a board/chip exposes to the RustNet runtime, bundled.
/// The firmware selects the concrete implementation at build time.
pub trait Board: Send {
    fn name(&self) -> &str;
    fn gpio(&mut self, pin: u32) -> HalResult<&mut dyn gpio::GpioPin>;
    fn i2c(&mut self, bus: u8) -> HalResult<&mut dyn i2c::I2cBus>;
    fn spi(&mut self, bus: u8) -> HalResult<&mut dyn spi::SpiBus>;
    fn uart(&mut self, port: u8) -> HalResult<&mut dyn uart::Uart>;
    fn i2s(&mut self, port: u8) -> HalResult<&mut dyn i2s::I2sBus>;
    fn pwm(&mut self, channel: u8) -> HalResult<&mut dyn pwm::PwmChannel>;
    fn adc(&mut self, channel: u8) -> HalResult<&mut dyn adc::AdcChannel>;
    fn power(&mut self) -> &mut dyn power::PowerManager;
    fn delay(&mut self) -> &mut dyn delay::Delay;
    fn can(&mut self, bus: u8) -> HalResult<&mut dyn can::CanBus>;
    fn onewire(&mut self, bus: u8) -> HalResult<&mut dyn onewire::OneWireBus>;
    fn rtc(&mut self) -> &mut dyn rtc::Rtc;
    fn watchdog(&mut self) -> &mut dyn watchdog::Watchdog;
    fn extmem(&mut self, index: u8) -> HalResult<&mut dyn extmem::ExtMemory>;
    fn netif(&mut self, kind: netif::NetIfKind) -> HalResult<&mut dyn netif::NetInterface>;
    fn signal(&mut self, pin: u32) -> HalResult<&mut dyn signal::SignalControl>;

    /// The board's image sensor, if it has one.
    ///
    /// Defaulted rather than required, because a camera is the one peripheral
    /// here that most boards genuinely do not have: an STM32 Nucleo has no
    /// sensor interface at all, and making every board write a stub that
    /// returns `NotSupported` would be four copies of the same refusal. The
    /// default *is* that refusal, so a board with a camera overrides it and a
    /// board without one says `not supported` — which is what a program
    /// calling `Camera.Capture()` needs to hear, rather than a made-up frame.
    fn camera(&mut self) -> HalResult<&mut dyn camera::Camera> {
        Err(HalError::NotSupported)
    }

    /// Push a fully-rendered RGB565 framebuffer (row-major, `width`×`height`,
    /// native `u16` per pixel) to the board's physical panel, if it has one.
    /// Called on `Display.Present()`. Boards without a wired panel (the virtual
    /// device, headless boards) keep the default no-op — the framebuffer is
    /// still readable via the display-capture command.
    fn present_frame(&mut self, _rgb565: &[u16], _width: u32, _height: u32) -> HalResult<()> {
        Ok(())
    }
}
