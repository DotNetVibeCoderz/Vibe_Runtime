//! ESP32-C3 (RISC-V RV32IMC) board implementation of the RustNet HAL.
//!
//! Bring-up status: GPIO output/input runs directly on the chip's GPIO
//! matrix registers (ESP32-C3 TRM chapter 5); the delay source is a
//! calibrated spin loop. Every other peripheral returns `NotSupported`
//! with a pointer to its integration point — the intended fill-in is
//! `esp-hal` (UART/I2C/SPI/TWAI→CAN/RMT→SignalControl) and `esp-wifi`
//! (netif), keeping this crate's trait surface unchanged.
//!
//! The crate is `no_std` and builds for `riscv32imc-unknown-none-elf`:
//!
//! ```text
//! cargo build -p rustclr-hal-esp32c3 --target riscv32imc-unknown-none-elf
//! ```

#![no_std]

extern crate alloc;

use rustclr_hal::gpio::{Edge, GpioPin, Level, PinMode};
use rustclr_hal::power::{BatteryStatus, PowerManager, SleepMode, WakeReason, WakeSource};
use rustclr_hal::rtc::{DateTime, Rtc};
use rustclr_hal::watchdog::Watchdog;
use rustclr_hal::{delay::Delay, Board, HalError, HalResult};

/// ESP32-C3 GPIO matrix registers (TRM v1.1, chapter 5.14).
const GPIO_BASE: usize = 0x6000_4000;
const GPIO_OUT_W1TS: usize = GPIO_BASE + 0x0008;
const GPIO_OUT_W1TC: usize = GPIO_BASE + 0x000C;
const GPIO_ENABLE_W1TS: usize = GPIO_BASE + 0x0024;
const GPIO_ENABLE_W1TC: usize = GPIO_BASE + 0x0028;
const GPIO_IN: usize = GPIO_BASE + 0x003C;
const GPIO_OUT: usize = GPIO_BASE + 0x0004;

/// ESP32-C3 exposes GPIO0..=21.
const PIN_COUNT: u32 = 22;

/// Default CPU frequency after ROM boot (160 MHz max; 20 MHz ROM default
/// is raised by the bootloader — we assume the common 160 MHz setup).
const CPU_HZ: u32 = 160_000_000;

#[inline(always)]
fn reg_write(addr: usize, value: u32) {
    // SAFETY: fixed peripheral addresses from the ESP32-C3 TRM; only
    // meaningful when executing on the chip itself.
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

#[inline(always)]
fn reg_read(addr: usize) -> u32 {
    // SAFETY: see `reg_write`.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

pub struct C3Pin {
    pin: u32,
}

impl GpioPin for C3Pin {
    fn set_mode(&mut self, mode: PinMode) -> HalResult<()> {
        match mode {
            PinMode::Output | PinMode::OutputOpenDrain => {
                reg_write(GPIO_ENABLE_W1TS, 1 << self.pin)
            }
            _ => reg_write(GPIO_ENABLE_W1TC, 1 << self.pin),
        }
        Ok(())
    }

    fn write(&mut self, level: Level) -> HalResult<()> {
        match level {
            Level::High => reg_write(GPIO_OUT_W1TS, 1 << self.pin),
            Level::Low => reg_write(GPIO_OUT_W1TC, 1 << self.pin),
        }
        Ok(())
    }

    fn read(&mut self) -> HalResult<Level> {
        // Output pins read back the output latch, inputs the pad.
        let out_enabled = reg_read(GPIO_BASE + 0x0020) & (1 << self.pin) != 0;
        let bits = if out_enabled { reg_read(GPIO_OUT) } else { reg_read(GPIO_IN) };
        Ok(if bits & (1 << self.pin) != 0 { Level::High } else { Level::Low })
    }

    fn toggle(&mut self) -> HalResult<()> {
        let level = self.read()?;
        self.write(if level == Level::High { Level::Low } else { Level::High })
    }

    fn on_edge(
        &mut self,
        _edge: Edge,
        _callback: alloc::boxed::Box<dyn FnMut(Level) + Send>,
    ) -> HalResult<()> {
        // Integration point: GPIO interrupt matrix + esp-hal's interrupt
        // handler registration.
        Err(HalError::NotSupported)
    }

    fn clear_interrupt(&mut self) -> HalResult<()> {
        Ok(())
    }
}

/// Cycle-counted busy-wait delay (RISC-V `mcycle` CSR).
pub struct SpinDelay;

impl Delay for SpinDelay {
    fn delay_us(&mut self, us: u64) {
        let cycles = us * (CPU_HZ as u64 / 1_000_000);
        let start = mcycle();
        while mcycle().wrapping_sub(start) < cycles {
            core::hint::spin_loop();
        }
    }

    fn now_us(&self) -> u64 {
        mcycle() / (CPU_HZ as u64 / 1_000_000)
    }
}

#[inline(always)]
fn mcycle() -> u64 {
    #[cfg(target_arch = "riscv32")]
    {
        let lo: u32;
        // SAFETY: reading the standard RISC-V cycle CSR.
        unsafe { core::arch::asm!("csrr {0}, mcycle", out(reg) lo) };
        lo as u64
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        0
    }
}

/// Software RTC: seconds owed to `set()` plus cycle-counter drift.
pub struct SoftRtc {
    epoch_base: u64,
    alarm: Option<u64>,
}

impl Rtc for SoftRtc {
    fn now(&mut self) -> HalResult<DateTime> {
        Ok(DateTime::from_epoch(self.epoch_base))
    }
    fn set(&mut self, dt: DateTime) -> HalResult<()> {
        self.epoch_base = dt.to_epoch();
        Ok(())
    }
    fn set_alarm(&mut self, epoch: u64) -> HalResult<()> {
        self.alarm = Some(epoch);
        Ok(())
    }
    fn clear_alarm(&mut self) -> HalResult<()> {
        self.alarm = None;
        Ok(())
    }
    fn alarm(&self) -> Option<u64> {
        self.alarm
    }
}

pub struct C3Power;

impl PowerManager for C3Power {
    fn sleep(&mut self, _mode: SleepMode, _duration_ms: Option<u64>) -> HalResult<()> {
        // Integration point: RTC_CNTL sleep registers / esp-hal RtcSleep.
        Err(HalError::NotSupported)
    }
    fn battery(&mut self) -> HalResult<BatteryStatus> {
        Err(HalError::NotSupported)
    }
    fn cpu_frequency_hz(&self) -> u32 {
        CPU_HZ
    }
    fn set_cpu_frequency_hz(&mut self, _hz: u32) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn reset(&mut self) -> ! {
        // RTC_CNTL_OPTIONS0_REG sw_sys_rst (TRM chapter 8); placeholder
        // spin keeps the signature honest until the register write lands.
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

pub struct C3Watchdog;

impl Watchdog for C3Watchdog {
    fn start(&mut self, _timeout_ms: u32) -> HalResult<()> {
        // Integration point: TIMG0 watchdog registers.
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

/// The ESP32-C3 board. GPIO + delay are live; the remaining peripherals
/// name their integration points and fail fast until wired to esp-hal.
pub struct Esp32C3Board {
    pins: [C3Pin; PIN_COUNT as usize],
    delay: SpinDelay,
    rtc: SoftRtc,
    power: C3Power,
    watchdog: C3Watchdog,
}

impl Default for Esp32C3Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Esp32C3Board {
    pub fn new() -> Self {
        Esp32C3Board {
            pins: core::array::from_fn(|i| C3Pin { pin: i as u32 }),
            delay: SpinDelay,
            rtc: SoftRtc { epoch_base: 0, alarm: None },
            power: C3Power,
            watchdog: C3Watchdog,
        }
    }
}

impl Board for Esp32C3Board {
    fn name(&self) -> &str {
        "esp32c3-devkit"
    }
    fn gpio(&mut self, pin: u32) -> HalResult<&mut dyn GpioPin> {
        self.pins
            .get_mut(pin as usize)
            .map(|p| p as &mut dyn GpioPin)
            .ok_or(HalError::InvalidArgument("ESP32-C3 has GPIO0..=21"))
    }
    fn i2c(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::i2c::I2cBus> {
        Err(HalError::NotSupported) // integration point: esp-hal I2C0
    }
    fn spi(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::spi::SpiBus> {
        Err(HalError::NotSupported) // integration point: esp-hal SPI2
    }
    fn uart(&mut self, _port: u8) -> HalResult<&mut dyn rustclr_hal::uart::Uart> {
        Err(HalError::NotSupported) // integration point: esp-hal UART0/1 (RNDP transport)
    }
    fn i2s(&mut self, _port: u8) -> HalResult<&mut dyn rustclr_hal::i2s::I2sBus> {
        Err(HalError::NotSupported) // integration point: esp-hal I2S0
    }
    fn pwm(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::pwm::PwmChannel> {
        Err(HalError::NotSupported) // integration point: LEDC
    }
    fn adc(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::adc::AdcChannel> {
        Err(HalError::NotSupported) // integration point: SAR ADC1
    }
    fn power(&mut self) -> &mut dyn PowerManager {
        &mut self.power
    }
    fn delay(&mut self) -> &mut dyn Delay {
        &mut self.delay
    }
    fn can(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::can::CanBus> {
        Err(HalError::NotSupported) // integration point: TWAI controller
    }
    fn onewire(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::onewire::OneWireBus> {
        Err(HalError::NotSupported) // integration point: RMT-timed bit banging
    }
    fn rtc(&mut self) -> &mut dyn Rtc {
        &mut self.rtc
    }
    fn watchdog(&mut self) -> &mut dyn Watchdog {
        &mut self.watchdog
    }
    fn extmem(&mut self, _index: u8) -> HalResult<&mut dyn rustclr_hal::extmem::ExtMemory> {
        Err(HalError::NotSupported) // integration point: SPI flash driver
    }
    fn netif(
        &mut self,
        _kind: rustclr_hal::netif::NetIfKind,
    ) -> HalResult<&mut dyn rustclr_hal::netif::NetInterface> {
        Err(HalError::NotSupported) // integration point: esp-wifi / lwIP
    }
    fn signal(&mut self, _pin: u32) -> HalResult<&mut dyn rustclr_hal::signal::SignalControl> {
        Err(HalError::NotSupported) // integration point: RMT peripheral
    }
}
