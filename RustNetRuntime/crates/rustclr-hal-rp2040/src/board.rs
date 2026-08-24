//! The board: what this port implements, and honest refusals for the rest.
//!
//! A bring-up port. GPIO, UART, and time work; everything else in the
//! [`rustclr_hal::Board`] surface answers [`HalError::NotSupported`] rather
//! than pretending. A stub that silently succeeds is worse than one that
//! refuses — an application gets no reading and no reason.

use alloc::vec::Vec;
use rustclr_hal::delay::Delay;
use rustclr_hal::power::{BatteryStatus, PowerManager, SleepMode, WakeReason, WakeSource};
use rustclr_hal::rtc::{DateTime, Rtc};
use rustclr_hal::watchdog::Watchdog;
use rustclr_hal::{Board, HalError, HalResult};

use crate::clocks::Clocks;
use crate::gpio::{Rp2040Pin, PIN_COUNT};
use crate::timer::{self, TimerDelay};
use crate::uart::{Rp2040Uart, UART0, UART1};

/// Nothing here resets the chip yet; the watchdog is the way in when it does.
struct Rp2040Power {
    sys_hz: u32,
}

impl PowerManager for Rp2040Power {
    fn sleep(&mut self, _mode: SleepMode, _duration_ms: Option<u64>) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn battery(&mut self) -> HalResult<BatteryStatus> {
        // A Pico has no fuel gauge and no battery input; reporting a made-up
        // percentage would be worse than saying there is nothing to read.
        Err(HalError::NotSupported)
    }
    fn cpu_frequency_hz(&self) -> u32 {
        self.sys_hz
    }
    fn set_cpu_frequency_hz(&mut self, _hz: u32) -> HalResult<()> {
        // Retuning the PLL under a running system means every UART divisor
        // derived from it is now wrong. Refused until something recomputes
        // them.
        Err(HalError::NotSupported)
    }
    fn reset(&mut self) -> ! {
        // The documented software reset is a watchdog armed to fire at once.
        // Until that is wired up, spin: the signature promises never to
        // return, and returning would be a lie the caller cannot detect.
        loop {
            core::hint::spin_loop();
        }
    }
    fn shutdown(&mut self) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }
    fn arm_wake(&mut self, _source: WakeSource) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn clear_wake_sources(&mut self) {}
    fn wake_reason(&self) -> WakeReason {
        WakeReason::PowerOn
    }
}

/// The RP2040 has no battery-backed clock, so this is uptime with an offset —
/// enough to timestamp a log line, not enough to trust across a power cut.
struct Rp2040Rtc {
    offset_s: u64,
}

impl Rtc for Rp2040Rtc {
    fn now(&mut self) -> HalResult<DateTime> {
        Ok(DateTime::from_epoch(
            self.offset_s + timer::now_us() / 1_000_000,
        ))
    }
    fn set(&mut self, dt: DateTime) -> HalResult<()> {
        self.offset_s = dt
            .to_epoch()
            .saturating_sub(timer::now_us() / 1_000_000);
        Ok(())
    }
    fn set_alarm(&mut self, _epoch: u64) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn clear_alarm(&mut self) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn alarm(&self) -> Option<u64> {
        None
    }
}

struct Rp2040Watchdog;

impl Watchdog for Rp2040Watchdog {
    fn start(&mut self, _timeout_ms: u32) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn feed(&mut self) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn stop(&mut self) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn is_running(&self) -> bool {
        false
    }
    fn timeout_ms(&self) -> u32 {
        0
    }
}

pub struct Rp2040Board {
    name: &'static str,
    clocks: Clocks,
    pins: Vec<Rp2040Pin>,
    uarts: [Rp2040Uart; 2],
    delay: TimerDelay,
    power: Rp2040Power,
    rtc: Rp2040Rtc,
    watchdog: Rp2040Watchdog,
}

impl Rp2040Board {
    pub fn new(name: &'static str, clocks: Clocks) -> Self {
        Self {
            name,
            clocks,
            pins: (0..PIN_COUNT).map(Rp2040Pin::new).collect(),
            uarts: [
                Rp2040Uart::new(UART0, clocks.peri_hz),
                Rp2040Uart::new(UART1, clocks.peri_hz),
            ],
            delay: TimerDelay,
            power: Rp2040Power { sys_hz: clocks.sys_hz },
            rtc: Rp2040Rtc { offset_s: 0 },
            watchdog: Rp2040Watchdog,
        }
    }

    /// Release the pin blocks and start the timer's tick. Call once, before
    /// anything else touches a peripheral.
    ///
    /// Returns whether time is actually running. A caller that ignores this
    /// gets delays that do not delay, and every timeout above it fires at
    /// once — which presents as a hung peripheral somewhere else entirely.
    pub fn init(&mut self) -> bool {
        crate::gpio::init();
        timer::start_tick(self.clocks.xosc_hz)
    }

    pub fn clocks(&self) -> Clocks {
        self.clocks
    }
}

impl Board for Rp2040Board {
    fn name(&self) -> &str {
        self.name
    }

    fn gpio(&mut self, pin: u32) -> HalResult<&mut dyn rustclr_hal::gpio::GpioPin> {
        self.pins
            .get_mut(pin as usize)
            .map(|p| p as &mut dyn rustclr_hal::gpio::GpioPin)
            .ok_or(HalError::InvalidArgument("RP2040 has GPIO0..=29"))
    }

    fn uart(&mut self, port: u8) -> HalResult<&mut dyn rustclr_hal::uart::Uart> {
        self.uarts
            .get_mut(port as usize)
            .map(|u| u as &mut dyn rustclr_hal::uart::Uart)
            .ok_or(HalError::InvalidArgument("RP2040 has UART0 and UART1"))
    }

    fn delay(&mut self) -> &mut dyn Delay {
        &mut self.delay
    }

    fn power(&mut self) -> &mut dyn PowerManager {
        &mut self.power
    }

    fn rtc(&mut self) -> &mut dyn Rtc {
        &mut self.rtc
    }

    fn watchdog(&mut self) -> &mut dyn Watchdog {
        &mut self.watchdog
    }

    // --- not implemented on this port ------------------------------------
    //
    // Each of these is a real peripheral on the chip and an integration point
    // rather than a gap in the silicon: I2C0/1, SPI0/1, PWM, the ADC's five
    // channels, and PIO — which is where this part is actually unusual, and
    // where a one-wire or I2S implementation would come from.

    fn i2c(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::i2c::I2cBus> {
        Err(HalError::NotSupported)
    }
    fn spi(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::spi::SpiBus> {
        Err(HalError::NotSupported)
    }
    fn i2s(&mut self, _port: u8) -> HalResult<&mut dyn rustclr_hal::i2s::I2sBus> {
        Err(HalError::NotSupported)
    }
    fn pwm(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::pwm::PwmChannel> {
        Err(HalError::NotSupported)
    }
    fn adc(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::adc::AdcChannel> {
        Err(HalError::NotSupported)
    }
    fn can(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::can::CanBus> {
        Err(HalError::NotSupported)
    }
    fn onewire(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::onewire::OneWireBus> {
        Err(HalError::NotSupported)
    }
    fn extmem(&mut self, _index: u8) -> HalResult<&mut dyn rustclr_hal::extmem::ExtMemory> {
        Err(HalError::NotSupported)
    }
    fn netif(
        &mut self,
        _kind: rustclr_hal::netif::NetIfKind,
    ) -> HalResult<&mut dyn rustclr_hal::netif::NetInterface> {
        Err(HalError::NotSupported)
    }
    fn signal(&mut self, _pin: u32) -> HalResult<&mut dyn rustclr_hal::signal::SignalControl> {
        Err(HalError::NotSupported)
    }
}
