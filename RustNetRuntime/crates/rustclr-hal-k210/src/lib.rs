//! Kendryte K210 (dual-core RV64GC) board implementation of the RustNet HAL.
//!
//! The K210 is an unusual target for this runtime, and in a helpful direction:
//! **6 MB of on-chip SRAM.** Where the STM32F401 port has to justify every
//! kilobyte of heap, here the IL interpreter, its heap, an inbound signed
//! container and a 320×240 framebuffer all fit at once with room to spare. What
//! it does *not* have is any internal flash — the mask ROM copies the firmware
//! out of an external SPI NOR part into SRAM and jumps there — so persistence
//! means driving that chip, which [`flash`] does.
//!
//! Bring-up status: FPIOA muxing, GPIOHS, UARTHS and UART1..3, an `mcycle`
//! clock, the three SPI masters and the boot flash as `extmem(0)` all run at
//! register level. Everything else returns `NotSupported` with its integration
//! point named in the source.
//!
//! Two things shape the whole crate:
//!
//! **There is no pinout.** Any of the 48 pads can carry any of 256 peripheral
//! functions, so `Board::gpio(pin)` takes an *FPIOA pad* number and has to
//! allocate one of GPIOHS's 32 channels and route it before there is anything to
//! drive. That allocation is the board's job and lives here. For the same
//! reason the UARTs and SPI buses have no default pins — see
//! [`uart::UartDef`].
//!
//! **The clock tree is read, not written.** The ROM has already brought PLL0 up
//! by the time this runs, and re-programming a PLL that feeds the executing core
//! is the kind of change that either works or hangs silently.
//! [`sysctl::Clocks::detect`] recovers what is in force and everything scales off
//! that, so the firmware's boot banner reports the real core frequency on the
//! first hardware run.
//!
//! ```text
//! cargo build -p rustclr-hal-k210 --target riscv64gc-unknown-none-elf
//! ```

#![no_std]

extern crate alloc;

use rustclr_hal::delay::Delay;
use rustclr_hal::extmem::ExtMemory;
use rustclr_hal::gpio::GpioPin;
use rustclr_hal::power::{BatteryStatus, PowerManager, SleepMode, WakeReason, WakeSource};
use rustclr_hal::rtc::{DateTime, Rtc};
use rustclr_hal::spi::SpiBus;
use rustclr_hal::uart::Uart;
use rustclr_hal::watchdog::Watchdog;
use rustclr_hal::{Board, HalError, HalResult};

pub mod camera;
pub mod delay;
pub mod flash;
pub mod fpioa;
pub mod gpio;
pub mod i2c;
pub mod lcd;
pub mod reg;
pub mod spi;
pub mod sysctl;
pub mod uart;

pub use delay::CycleDelay;
pub use flash::SpiFlash;
pub use camera::Dvp;
pub use gpio::K210Pin;
pub use i2c::K210I2c;
pub use lcd::{PanelPins, St7789, MAIX_PANEL};
pub use sysctl::Clocks;
pub use uart::{K210Uart, Uarths};

/// Marker for a pad that has not been bound to a GPIOHS channel.
const UNBOUND: u8 = u8::MAX;

// ---------------------------------------------------------------------------
// RTC
// ---------------------------------------------------------------------------

/// Software RTC: the epoch handed to `set`, plus elapsed time off the cycle
/// counter.
///
/// The K210 has a real RTC at `0x5046_0000` with its own 32.768 kHz domain, but
/// nothing on a Maix board keeps it powered across a cold boot, so it would only
/// hold time across a reset — not across the power cycle where a hardware RTC
/// actually earns its place. This is the honest equivalent until someone needs
/// the alarm as a deep-sleep wake source.
pub struct SoftRtc {
    epoch_base: u64,
    /// `now_us` at the moment `set` was called, so `now` can advance.
    set_at_us: u64,
    cpu_hz: u32,
    alarm: Option<u64>,
}

impl SoftRtc {
    fn elapsed_secs(&self) -> u64 {
        let per_us = (self.cpu_hz as u64 / 1_000_000).max(1);
        let now_us = delay::cycles() / per_us;
        now_us.saturating_sub(self.set_at_us) / 1_000_000
    }
}

impl Rtc for SoftRtc {
    fn now(&mut self) -> HalResult<DateTime> {
        Ok(DateTime::from_epoch(self.epoch_base + self.elapsed_secs()))
    }
    fn set(&mut self, dt: DateTime) -> HalResult<()> {
        let per_us = (self.cpu_hz as u64 / 1_000_000).max(1);
        self.epoch_base = dt.to_epoch();
        self.set_at_us = delay::cycles() / per_us;
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

// ---------------------------------------------------------------------------
// Power / watchdog
// ---------------------------------------------------------------------------

pub struct K210Power {
    cpu_hz: u32,
}

/// `soft_reset` — SYSCTL offset 0x30, bit 0. Resets the SoC, so the mask ROM
/// runs again and reloads the image from flash, which is what `rustnet reboot`
/// is asking for.
const SYSCTL_SOFT_RESET: usize = 0x5044_0000 + 0x30;

impl PowerManager for K210Power {
    fn sleep(&mut self, _mode: SleepMode, _duration_ms: Option<u64>) -> HalResult<()> {
        // Integration point: `wfi` with a CLINT `mtimecmp` deadline for a timed
        // wake, and `sysctl.power_sel` for the IO voltage domains on the way
        // down. Plain `wfi` alone would be a light sleep with no wake source
        // configured, which is a hang.
        Err(HalError::NotSupported)
    }
    fn battery(&mut self) -> HalResult<BatteryStatus> {
        // Integration point: no on-chip fuel gauge. A Maix Go's AXP173 PMIC
        // reports charge over I2C, which belongs in a board module rather than
        // here.
        Err(HalError::NotSupported)
    }
    fn cpu_frequency_hz(&self) -> u32 {
        self.cpu_hz
    }
    fn set_cpu_frequency_hz(&mut self, _hz: u32) -> HalResult<()> {
        // Integration point: PLL0's clkr/clkf/clkod with the reset-and-wait
        // lock sequence, plus `clk_sel0.aclk_divider_sel`. Deliberately absent:
        // see the crate docs on why this port reads the clock tree.
        Err(HalError::NotSupported)
    }
    fn reset(&mut self) -> ! {
        reg::write(SYSCTL_SOFT_RESET, 1);
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
        // Integration point: `sysctl.reset_status` at offset 0x60 distinguishes
        // a pin reset from a watchdog or software one.
        WakeReason::PowerOn
    }
}

pub struct K210Watchdog;

impl Watchdog for K210Watchdog {
    fn start(&mut self, _timeout_ms: u32) -> HalResult<()> {
        // Integration point: WDT0 at 0x5040_0000 (WDT1 at 0x5041_0000). The
        // timeout is a power-of-two range code rather than a millisecond count,
        // and the part needs its magic restart value (0x76) fed to `crr`.
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

// ---------------------------------------------------------------------------
// Board
// ---------------------------------------------------------------------------

/// The K210 board.
///
/// `gpio`, `uart`, `spi`, `delay`, `power` and — once the firmware attaches a
/// region — `extmem` are live. The rest name their integration points and fail
/// fast.
pub struct K210Board {
    name: &'static str,
    clocks: Clocks,
    pins: [K210Pin; gpio::CHANNEL_COUNT],
    /// FPIOA pad to GPIOHS channel, or [`UNBOUND`].
    pads: [u8; fpioa::PAD_COUNT as usize],
    /// Next channel a first-come allocation hands out.
    next_channel: u8,
    uarths: Uarths,
    uarts: [K210Uart; uart::UARTS.len()],
    spis: [spi::K210Spi; spi::BUSES.len()],
    i2cs: [i2c::K210I2c; i2c::BUSES.len()],
    delay: CycleDelay,
    rtc: SoftRtc,
    power: K210Power,
    watchdog: K210Watchdog,
    storage: Option<SpiFlash>,
    files: Option<SpiFlash>,
    panel: Option<lcd::St7789>,
}

impl K210Board {
    /// Build a board around clock frequencies the firmware has established.
    ///
    /// Side-effect free: nothing here touches a register, so a board can be
    /// constructed off-chip. [`K210Board::init`] is where hardware gets poked.
    pub fn new(name: &'static str, clocks: Clocks, console_pins: Option<(u8, u8)>) -> Self {
        Self {
            name,
            clocks,
            pins: core::array::from_fn(|i| K210Pin::new(i as u8)),
            pads: [UNBOUND; fpioa::PAD_COUNT as usize],
            next_channel: 0,
            uarths: Uarths::new(clocks.cpu_hz, console_pins),
            uarts: core::array::from_fn(|i| K210Uart::new(uart::UARTS[i], clocks.apb0_hz)),
            spis: core::array::from_fn(|i| {
                let def = spi::BUSES[i];
                spi::K210Spi::new(def, clocks.spi_hz(def.bus))
            }),
            // The I²C controllers hang off APB0.
            i2cs: core::array::from_fn(|i| i2c::K210I2c::new(i2c::BUSES[i], clocks.apb0_hz)),
            delay: CycleDelay::new(clocks.cpu_hz),
            rtc: SoftRtc { epoch_base: 0, set_at_us: 0, cpu_hz: clocks.cpu_hz, alarm: None },
            power: K210Power { cpu_hz: clocks.cpu_hz },
            watchdog: K210Watchdog,
            storage: None,
            files: None,
            panel: None,
        }
    }

    /// Ungate the clocks the live peripherals need. Assumes it is running on
    /// the chip.
    pub fn init(&mut self) {
        sysctl::enable_central_clocks();
        // FPIOA has to be clocked before a single pad can be muxed, which makes
        // it the one gate whose absence looks like every driver being broken at
        // once. GPIO is the conventional 8-channel block; GPIOHS and UARTHS sit
        // on the TileLink bus and have no gate to open.
        sysctl::clock_enable(sysctl::Peripheral::Fpioa);
        sysctl::clock_enable(sysctl::Peripheral::Gpio);
    }

    pub fn clocks(&self) -> Clocks {
        self.clocks
    }

    /// Hand the board a window of the boot flash to expose as `extmem(0)`.
    /// The firmware owns the decision, because only its flashing recipe knows
    /// how much of the device the image occupies.
    pub fn attach_storage(&mut self, flash: SpiFlash) {
        self.storage = Some(flash);
    }

    /// Hand the board a second, larger window to expose as `extmem(1)` — where
    /// the filesystem lives. Must not overlap the record window; the firmware
    /// picks both, and [`SpiFlash`] confines each to its own range.
    pub fn attach_files(&mut self, flash: SpiFlash) {
        self.files = Some(flash);
    }

    /// Give the board a wired panel, so `Display.Present()` reaches glass.
    ///
    /// Takes SPI0 out of general circulation: the panel needs the controller in
    /// octal mode with its data lines switched away from FPIOA, and a driver
    /// reconfiguring it between frames would be fighting for the same
    /// registers. [`Board::spi`] refuses bus 0 from here on, the way it already
    /// refuses bus 3 to protect the flash.
    pub fn attach_panel(&mut self, panel: lcd::St7789) {
        self.panel = Some(panel);
    }

    pub fn panel_mut(&mut self) -> Option<&mut lcd::St7789> {
        self.panel.as_mut()
    }

    /// Route `pad` to a specific GPIOHS `channel`.
    ///
    /// Worth doing for anything the firmware needs to reach without a board in
    /// scope — a panic handler blinking an LED knows a channel number, not a
    /// pad. Also the only way to be sure two pads do not end up on the same
    /// channel when some are bound eagerly and others on first use.
    pub fn bind_gpio(&mut self, pad: u8, channel: u8) -> HalResult<()> {
        if pad >= fpioa::PAD_COUNT {
            return Err(HalError::InvalidArgument("K210 has 48 FPIOA pads, IO0..IO47"));
        }
        if channel as usize >= gpio::CHANNEL_COUNT {
            return Err(HalError::InvalidArgument("GPIOHS has 32 channels"));
        }
        fpioa::set_function(pad, fpioa::gpiohs(channel));
        self.pins[channel as usize].pad = pad;
        self.pads[pad as usize] = channel;
        // Keep first-come allocation past anything bound by hand.
        self.next_channel = self.next_channel.max(channel + 1);
        Ok(())
    }

    /// The GPIOHS channel a pad is on, allocating one on first use.
    fn channel_for(&mut self, pad: u32) -> HalResult<u8> {
        if pad >= fpioa::PAD_COUNT as u32 {
            return Err(HalError::InvalidArgument("K210 has 48 FPIOA pads, IO0..IO47"));
        }
        let existing = self.pads[pad as usize];
        if existing != UNBOUND {
            return Ok(existing);
        }
        if self.next_channel as usize >= gpio::CHANNEL_COUNT {
            // Worth being explicit: this is not an invalid pin, it is 33 pins
            // asked of a block with 32 channels, and the fix is to free one
            // rather than to pick a different pad.
            return Err(HalError::Busy);
        }
        let channel = self.next_channel;
        self.bind_gpio(pad as u8, channel)?;
        Ok(channel)
    }

    /// Route one of the 16550 ports to a pair of pads. Nothing has a default
    /// pinout on this chip, so a port is unusable until this is called.
    pub fn set_uart_pins(&mut self, port: u8, tx: u8, rx: u8) -> HalResult<()> {
        let slot = (port as usize)
            .checked_sub(1)
            .filter(|i| *i < self.uarts.len())
            .ok_or(HalError::InvalidArgument("UART ports are 1..=3 (0 is UARTHS)"))?;
        self.uarts[slot].set_pins(tx, rx);
        Ok(())
    }

    /// The console, for callers that need it without the `Board` trait — the
    /// panic path, and the firmware's log ring.
    pub fn uarths(&mut self) -> &mut Uarths {
        &mut self.uarths
    }

    /// The attached flash as its concrete type, for the few operations that are
    /// not part of the `ExtMemory` surface — reading the JEDEC id, which is what
    /// a boot banner wants to report on hardware nobody has run this on yet.
    pub fn flash_mut(&mut self) -> Option<&mut SpiFlash> {
        self.storage.as_mut()
    }
}

impl Board for K210Board {
    fn name(&self) -> &str {
        self.name
    }

    fn gpio(&mut self, pin: u32) -> HalResult<&mut dyn GpioPin> {
        let channel = self.channel_for(pin)?;
        Ok(&mut self.pins[channel as usize] as &mut dyn GpioPin)
    }

    /// Port 0 is UARTHS — the console, and where RNDP lives. Ports 1..3 are the
    /// conventional 16550s.
    fn uart(&mut self, port: u8) -> HalResult<&mut dyn Uart> {
        match port {
            0 => Ok(&mut self.uarths as &mut dyn Uart),
            1..=3 => Ok(&mut self.uarts[port as usize - 1] as &mut dyn Uart),
            _ => Err(HalError::InvalidArgument("UART ports are 0 (UARTHS) and 1..=3")),
        }
    }

    /// Bus 0 is SPI0 (the LCD header), bus 1 is SPI1 (the microSD slot).
    /// SPI2 is slave-only silicon and SPI3 belongs to the boot flash, which is
    /// reached as `extmem(0)` instead — handing out a second owner of that
    /// controller would let an application corrupt its own storage.
    fn spi(&mut self, bus: u8) -> HalResult<&mut dyn SpiBus> {
        match bus {
            // A wired panel owns SPI0 outright — octal frames with the data
            // lines switched off FPIOA — so it is `Busy` for the same reason
            // bus 3 is: something else is holding the controller in a
            // configuration a general-purpose driver would undo.
            0 if self.panel.is_some() => Err(HalError::Busy),
            0 | 1 => Ok(&mut self.spis[bus as usize] as &mut dyn SpiBus),
            2 => Err(HalError::NotSupported),
            3 => Err(HalError::Busy),
            _ => Err(HalError::InvalidArgument("SPI buses are 0 and 1")),
        }
    }

    fn present_frame(&mut self, rgb565: &[u16], width: u32, height: u32) -> HalResult<()> {
        match self.panel.as_mut() {
            Some(panel) => panel.present(rgb565, width, height),
            // No panel wired: the framebuffer is still whole and still
            // readable over the wire, so this is not an error.
            None => Ok(()),
        }
    }

    fn i2c(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::i2c::I2cBus> {
        // Note what is *not* here: the camera. Its SCCB channel looks like I²C
        // and is not on any of these — see `crate::camera`.
        match bus {
            0..=2 => Ok(&mut self.i2cs[bus as usize] as &mut dyn rustclr_hal::i2c::I2cBus),
            _ => Err(HalError::InvalidArgument("I2C buses are 0, 1 and 2")),
        }
    }
    fn i2s(&mut self, _port: u8) -> HalResult<&mut dyn rustclr_hal::i2s::I2sBus> {
        // Integration point: I2S0..2 at 0x5025_0000 upwards. A Maix Go's
        // microphone array is on I2S0 (pads IO18/IO19/IO20).
        Err(HalError::NotSupported)
    }
    fn pwm(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::pwm::PwmChannel> {
        // Integration point: TIMER0..2 at 0x502D_0000 upwards, whose toggle
        // outputs FPIOA can route to any pad (functions 190..201).
        Err(HalError::NotSupported)
    }
    fn adc(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::adc::AdcChannel> {
        // No ADC on this part at all — the K210 is a digital-only SoC. A board
        // that needs one has an external converter on I2C or SPI.
        Err(HalError::NotSupported)
    }
    fn power(&mut self) -> &mut dyn PowerManager {
        &mut self.power
    }
    fn delay(&mut self) -> &mut dyn Delay {
        &mut self.delay
    }
    fn can(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::can::CanBus> {
        // No CAN controller on this part.
        Err(HalError::NotSupported)
    }
    fn onewire(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::onewire::OneWireBus> {
        // Integration point: bit-banged on a GPIOHS pad in open-drain mode,
        // timed off the `mcycle` delay. Both halves already exist.
        Err(HalError::NotSupported)
    }
    fn rtc(&mut self) -> &mut dyn Rtc {
        &mut self.rtc
    }
    fn watchdog(&mut self) -> &mut dyn Watchdog {
        &mut self.watchdog
    }
    fn extmem(&mut self, index: u8) -> HalResult<&mut dyn ExtMemory> {
        // 0 is the firmware's own record window; 1 is the filesystem's. Two
        // windows rather than one because they have different lifetimes: the
        // records must survive a filesystem that fills up and compacts, and a
        // single region would let either one erase the other's sectors.
        let region = match index {
            0 => self.storage.as_mut(),
            1 => self.files.as_mut(),
            _ => return Err(HalError::NotSupported),
        };
        region.map(|f| f as &mut dyn ExtMemory).ok_or(HalError::NotSupported)
    }
    fn netif(
        &mut self,
        _kind: rustclr_hal::netif::NetIfKind,
    ) -> HalResult<&mut dyn rustclr_hal::netif::NetInterface> {
        // Integration point: a Maix Go carries an ESP8285 on UART1 (pads IO6/
        // IO7, enable on IO8), so WiFi here is an AT-command companion rather
        // than an on-chip radio.
        Err(HalError::NotSupported)
    }
    fn signal(&mut self, _pin: u32) -> HalResult<&mut dyn rustclr_hal::signal::SignalControl> {
        // Integration point: TIMER0..2 capture/compare, same peripheral as PWM.
        Err(HalError::NotSupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_hal::uart::UartConfig;

    fn board() -> K210Board {
        K210Board::new("test", Clocks::MAIX_DEFAULT, None)
    }

    /// First-come allocation, and a pad asked for twice keeps its channel —
    /// otherwise every `Board::gpio` call would re-mux and eventually run out.
    #[test]
    fn pads_get_a_channel_once_and_keep_it() {
        let mut b = board();
        assert_eq!(b.channel_for(24).unwrap(), 0);
        assert_eq!(b.channel_for(25).unwrap(), 1);
        assert_eq!(b.channel_for(24).unwrap(), 0);
        assert_eq!(b.channel_for(25).unwrap(), 1);
    }

    #[test]
    fn a_hand_bound_channel_is_not_handed_out_again() {
        let mut b = board();
        b.bind_gpio(14, 3).unwrap();
        assert_eq!(b.channel_for(14).unwrap(), 3);
        // Allocation resumes past the bound one rather than colliding with it.
        assert_eq!(b.channel_for(20).unwrap(), 4);
    }

    #[test]
    fn pads_beyond_the_chip_are_rejected() {
        let mut b = board();
        assert!(b.channel_for(48).is_err());
        assert!(b.channel_for(1_000).is_err());
        assert!(b.bind_gpio(0, 32).is_err());
    }

    /// Thirty-three pins asked of a 32-channel block is a resource exhaustion,
    /// not a bad argument, and the error says so.
    #[test]
    fn running_out_of_channels_reports_busy_not_invalid() {
        let mut b = board();
        for pad in 0..gpio::CHANNEL_COUNT as u32 {
            assert!(b.channel_for(pad).is_ok(), "pad {pad} should get a channel");
        }
        assert_eq!(b.channel_for(32), Err(HalError::Busy));
    }

    #[test]
    fn port_zero_is_the_console_and_four_is_nothing() {
        let mut b = board();
        assert!(b.uart(0).is_ok());
        assert!(b.uart(3).is_ok());
        assert!(b.uart(4).is_err());
    }

    /// SPI3 exists but is refused, because storage owns it. `Busy` rather than
    /// `NotSupported` is the distinction: the controller is there and working,
    /// just not yours.
    #[test]
    fn the_boot_flash_controller_is_not_handed_out() {
        let mut b = board();
        assert!(b.spi(0).is_ok());
        assert!(b.spi(1).is_ok());
        assert_eq!(b.spi(2).err(), Some(HalError::NotSupported));
        assert_eq!(b.spi(3).err(), Some(HalError::Busy));
    }

    #[test]
    fn extmem_is_absent_until_the_firmware_attaches_a_region() {
        let mut b = board();
        assert!(b.extmem(0).is_err());
        b.attach_storage(SpiFlash::new(0x00FC_0000, 0x0004_0000, Clocks::MAIX_DEFAULT.spi_hz(3)));
        assert_eq!(b.extmem(0).unwrap().size(), 0x0004_0000);
        assert!(b.extmem(1).is_err(), "the filesystem window is separately attached");

        b.attach_files(SpiFlash::new(0x0010_0000, 0x00EC_0000, Clocks::MAIX_DEFAULT.spi_hz(3)));
        assert_eq!(b.extmem(1).unwrap().size(), 0x00EC_0000);
        assert!(b.extmem(2).is_err());
    }

    #[test]
    fn uart_pins_are_addressed_by_hal_port_number() {
        let mut b = board();
        assert!(b.set_uart_pins(1, 7, 6).is_ok());
        assert!(b.set_uart_pins(3, 11, 10).is_ok());
        // Port 0 is UARTHS, whose pins come from the constructor.
        assert!(b.set_uart_pins(0, 5, 4).is_err());
        assert!(b.set_uart_pins(4, 1, 2).is_err());
    }

    /// UARTHS cannot do parity in hardware, and saying so beats configuring a
    /// port that then garbles every frame.
    #[test]
    fn the_console_refuses_a_frame_format_it_cannot_produce() {
        let mut b = board();
        let with_parity = UartConfig { parity: rustclr_hal::uart::Parity::Even, ..Default::default() };
        assert!(b.uart(0).unwrap().configure(with_parity).is_err());
    }

    #[test]
    fn power_reports_the_clock_the_board_was_built_with() {
        let mut b = board();
        assert_eq!(b.power().cpu_frequency_hz(), Clocks::MAIX_DEFAULT.cpu_hz);
    }

    /// A software RTC still has to round-trip a calendar date, or `DateTime.Now`
    /// in managed code reports nonsense.
    #[test]
    fn the_soft_rtc_round_trips_a_date() {
        let mut b = board();
        let set = DateTime { year: 2026, month: 7, day: 30, hour: 12, minute: 34, second: 56 };
        b.rtc().set(set).unwrap();
        let read = b.rtc().now().unwrap();
        assert_eq!((read.year, read.month, read.day), (2026, 7, 30));
    }
}
