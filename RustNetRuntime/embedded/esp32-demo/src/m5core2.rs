//! The M5Stack Core2's panel: an AXP192 that gates its power, and an ILI9342C
//! that draws.
//!
//! Ported from the Gravicode RustNet ESP-IDF firmware, which drives this board
//! through `esp-idf-svc`. The *register sequences* are the valuable part and
//! they carry over unchanged; the plumbing is rewritten against `esp-hal`,
//! because this firmware is bare-metal `no_std` and there is no IDF underneath
//! it.
//!
//! # The screen is dark until the PMIC says otherwise
//!
//! Nothing about this panel works until the **AXP192** has been told to. It
//! gates the LCD's 3.3 V rail (LDO2), the backlight rail (DCDC3) and the
//! panel's reset line (its own GPIO4). A board where the SPI writes are
//! perfect and the PMIC was never configured looks exactly like a board where
//! the SPI is broken: the screen keeps whatever was in the panel's RAM from
//! whatever ran before. That is worth knowing before debugging the wrong half.
//!
//! # What is wired where
//!
//! ```text
//! AXP192   I2C0, address 0x34, SDA 21 / SCL 22
//! ILI9342C SPI2, SCLK 18 / MOSI 23 / CS 5 / DC 15, 320x240 RGB565
//! ```
//!
//! # Differences from the reference worth stating
//!
//! The reference streams frames by DMA out of a bounce buffer, because its
//! framebuffer lives in PSRAM and SPI DMA cannot read PSRAM directly. Here the
//! framebuffer is in internal RAM already, so frames go out as ordinary
//! blocking writes in bands. Slower, and simple enough to be obviously right —
//! which is the correct trade for a driver whose first job is to prove the
//! panel can be driven at all.

use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output};
use esp_hal::i2c::master::I2c;
use esp_hal::spi::master::Spi;
use rustclr_hal::{HalError, HalResult};

/// The PMIC's address on I2C0.
const AXP192_ADDR: u8 = 0x34;

pub const PANEL_WIDTH: u32 = 320;
pub const PANEL_HEIGHT: u32 = 240;

/// Rows per SPI write. 40 rows of 320 RGB565 pixels is 25,600 bytes — big
/// enough that the per-transfer overhead disappears, small enough that the
/// staging buffer is not a meaningful part of the heap on a board that has
/// 278 KB of it.
const BAND_ROWS: usize = 40;

/// Brings the AXP192 up for an M5 LCD, following M5Stack's own Core2 sequence.
///
/// Every write here is read-modify-write where the datasheet says other bits in
/// the register matter — `DCDC1` in particular carries the ESP32's own supply,
/// and clearing it turns the board off mid-configuration.
fn axp192_init<'a>(i2c: &mut I2c<'a, esp_hal::Blocking>, delay: &Delay) -> HalResult<()> {
    let read = |i2c: &mut I2c<'a, esp_hal::Blocking>, reg: u8| -> HalResult<u8> {
        let mut byte = [0u8; 1];
        i2c.write_read(AXP192_ADDR, &[reg], &mut byte)
            .map_err(|_| HalError::Bus("AXP192 did not answer; is this an M5 board?"))?;
        Ok(byte[0])
    };
    let write = |i2c: &mut I2c<'a, esp_hal::Blocking>, reg: u8, value: u8| -> HalResult<()> {
        i2c.write(AXP192_ADDR, &[reg, value])
            .map_err(|_| HalError::Bus("AXP192 write failed"))
    };

    // VBUS current-limit path: keep bit 2, set bit 1.
    let v = read(i2c, 0x30)?;
    write(i2c, 0x30, (v & 0x04) | 0x02)?;
    // Battery-charge control at M5's default, and every ADC channel on.
    let v = read(i2c, 0x35)?;
    write(i2c, 0x35, (v & 0x1C) | 0xA2)?;
    write(i2c, 0x82, 0xFF)?;
    // LDO2 = 3.3 V (the LCD rail) in the upper nibble. LDO3 is the Core2's
    // vibration motor, so it gets the lowest setting and is never enabled
    // below — a Core2 that powers LDO3 the way a Tough does sits there buzzing.
    write(i2c, 0x28, 0xF0)?;
    // DCDC3 = 3.0 V, which is where this unit routes the backlight.
    let v = read(i2c, 0x27)?;
    write(i2c, 0x27, (v & 0x80) | 0x5C)?;
    // GPIO1 and GPIO2 to open-drain output.
    let v = read(i2c, 0x92)?;
    write(i2c, 0x92, v & 0xF8)?;
    let v = read(i2c, 0x93)?;
    write(i2c, 0x93, v & 0xF8)?;
    // GPIO4 to output — it drives the panel's reset line.
    let v = read(i2c, 0x95)?;
    write(i2c, 0x95, (v & 0x72) | 0x84)?;
    // Power-output enable: DCDC3 (bit 1, backlight), LDO2 (bit 2, LCD),
    // EXTEN (bit 6). `DCDC1` (bit 0) is preserved because it is the ESP32's
    // own supply.
    let v = read(i2c, 0x12)?;
    write(i2c, 0x12, v | 0x46)?;
    // Reset pulse on the PMIC's GPIO3/GPIO4: low, wait, high. Both bits,
    // because which one gates reset differs between M5 revisions.
    write(i2c, 0x96, 0x00)?;
    delay.delay_millis(120);
    write(i2c, 0x96, 0x03)?;
    delay.delay_millis(120);
    Ok(())
}

/// The panel.
pub struct M5Core2Panel<'a> {
    spi: Spi<'a, esp_hal::Blocking>,
    dc: Output<'a>,
    cs: Output<'a>,
    delay: Delay,
    /// One band of pixels, big-endian, staged for the SPI write.
    band: alloc::vec::Vec<u8>,
}

impl<'a> M5Core2Panel<'a> {
    /// Powers the panel through the PMIC and initialises the controller.
    ///
    /// The I2C bus is consumed: the AXP192 is the only thing on it that this
    /// firmware talks to, and handing it back would invite a second owner.
    pub fn new(
        mut i2c: I2c<'a, esp_hal::Blocking>,
        spi: Spi<'a, esp_hal::Blocking>,
        dc: Output<'a>,
        cs: Output<'a>,
    ) -> HalResult<Self> {
        let delay = Delay::new();
        axp192_init(&mut i2c, &delay)?;

        let mut panel = Self {
            spi,
            dc,
            cs,
            delay,
            band: alloc::vec![0u8; PANEL_WIDTH as usize * BAND_ROWS * 2],
        };
        panel.init_controller()?;
        Ok(panel)
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> HalResult<()> {
        self.spi
            .write(bytes)
            .map_err(|_| HalError::Bus("ILI9342C SPI write failed"))
    }

    fn command(&mut self, code: u8) -> HalResult<()> {
        self.dc.set_level(Level::Low);
        self.write_bytes(&[code])
    }

    fn data(&mut self, bytes: &[u8]) -> HalResult<()> {
        self.dc.set_level(Level::High);
        self.write_bytes(bytes)
    }

    fn init_controller(&mut self) -> HalResult<()> {
        self.cs.set_level(Level::Low);
        self.command(0x01)?; // SWRESET
        self.delay.delay_millis(150);
        self.command(0x11)?; // SLPOUT
        self.delay.delay_millis(120);
        self.command(0x3A)?;
        self.data(&[0x55])?; // COLMOD: 16 bits per pixel, RGB565
        self.command(0x36)?;
        self.data(&[0x08])?; // MADCTL: BGR order, native 320x240 landscape
        self.command(0x21)?; // INVON — the M5 panels are inverted
        self.command(0x13)?; // NORON
        self.command(0x29)?; // DISPON
        self.delay.delay_millis(50);
        self.cs.set_level(Level::High);
        Ok(())
    }

    /// Fills the whole panel with one colour.
    ///
    /// Not decoration: it separates "the bus is dead" from "the colours or the
    /// orientation are wrong", and those look identical in a photograph of a
    /// running application.
    pub fn fill(&mut self, colour: u16) -> HalResult<()> {
        // Band at a time out of the buffer that already exists. Allocating a
        // whole frame here — 153,600 bytes — was an out-of-memory on a board
        // with 278 KB whose BCL already holds 252 KB of it, and it killed the
        // firmware right after the panel came up.
        self.cs.set_level(Level::Low);
        self.command(0x2A)?;
        let last_column = (PANEL_WIDTH - 1) as u16;
        self.data(&[0, 0, (last_column >> 8) as u8, last_column as u8])?;
        self.command(0x2B)?;
        let last_row = (PANEL_HEIGHT - 1) as u16;
        self.data(&[0, 0, (last_row >> 8) as u8, last_row as u8])?;
        self.command(0x2C)?;
        self.dc.set_level(Level::High);

        let per_band = PANEL_WIDTH as usize * BAND_ROWS;
        for index in 0..per_band {
            self.band[index * 2] = (colour >> 8) as u8;
            self.band[index * 2 + 1] = colour as u8;
        }
        let mut row = 0usize;
        while row < PANEL_HEIGHT as usize {
            let rows = BAND_ROWS.min(PANEL_HEIGHT as usize - row);
            let bytes = rows * PANEL_WIDTH as usize * 2;
            let staged = core::mem::take(&mut self.band);
            let result = self.write_bytes(&staged[..bytes]);
            self.band = staged;
            result?;
            row += rows;
        }
        self.cs.set_level(Level::High);
        Ok(())
    }

    /// Pushes a frame.
    pub fn present(&mut self, pixels: &[u16], width: u32, height: u32) -> HalResult<()> {
        let width = width.min(PANEL_WIDTH) as usize;
        let height = height.min(PANEL_HEIGHT) as usize;
        if width == 0 || height == 0 {
            return Ok(());
        }

        self.cs.set_level(Level::Low);
        self.command(0x2A)?; // CASET — column address
        let last_column = (width - 1) as u16;
        self.data(&[0, 0, (last_column >> 8) as u8, last_column as u8])?;
        self.command(0x2B)?; // PASET — page address
        let last_row = (height - 1) as u16;
        self.data(&[0, 0, (last_row >> 8) as u8, last_row as u8])?;
        self.command(0x2C)?; // RAMWR
        self.dc.set_level(Level::High);

        // Big-endian per pixel, which is what RAMWR takes, staged a band at a
        // time so the copy never needs a second framebuffer.
        let mut row = 0usize;
        while row < height {
            let rows = BAND_ROWS.min(height - row);
            let count = rows * width;
            for index in 0..count {
                let pixel = pixels.get(row * width + index).copied().unwrap_or(0);
                self.band[index * 2] = (pixel >> 8) as u8;
                self.band[index * 2 + 1] = pixel as u8;
            }
            let bytes = count * 2;
            // Split so the borrow of `band` ends before `write_bytes` takes
            // `&mut self`.
            let staged = core::mem::take(&mut self.band);
            let result = self.write_bytes(&staged[..bytes]);
            self.band = staged;
            result?;
            row += rows;
        }
        self.cs.set_level(Level::High);
        Ok(())
    }
}

/// A delay source over the chip's own.
struct CoreDelay(Delay);

impl rustclr_hal::delay::Delay for CoreDelay {
    fn delay_us(&mut self, us: u64) {
        self.0.delay_micros(us as u32);
    }

    fn now_us(&self) -> u64 {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros()
    }
}

/// Stand-ins for the peripherals this board has and this firmware does not
/// drive yet.
///
/// The trait returns a reference rather than a `HalResult`, so there is no way
/// to refuse — which makes a panic the wrong answer twice over: it turns "not
/// driven yet" into a dead board, and it fires from the HAL report that every
/// firmware prints at boot. These say plainly that nothing is there.
struct NoWatchdog;

impl rustclr_hal::watchdog::Watchdog for NoWatchdog {
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


/// A power manager for a board whose PMIC this firmware only configures, never
/// drives afterwards.
struct NoPower;

impl rustclr_hal::power::PowerManager for NoPower {
    fn sleep(
        &mut self,
        _mode: rustclr_hal::power::SleepMode,
        _duration_ms: Option<u64>,
    ) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn battery(&mut self) -> HalResult<rustclr_hal::power::BatteryStatus> {
        // The AXP192 knows, and reading it back is not written yet. Refusing
        // beats a plausible percentage nobody measured.
        Err(HalError::NotSupported)
    }
    fn cpu_frequency_hz(&self) -> u32 {
        240_000_000
    }
    fn set_cpu_frequency_hz(&mut self, _hz: u32) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn reset(&mut self) -> ! {
        esp_hal::system::software_reset()
    }
    fn shutdown(&mut self) -> ! {
        // The PMIC could cut its own rails; until that is written, a reset is
        // the honest nearest thing and it is not silently different.
        esp_hal::system::software_reset()
    }
    fn arm_wake(&mut self, _source: rustclr_hal::power::WakeSource) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn clear_wake_sources(&mut self) {}
    fn wake_reason(&self) -> rustclr_hal::power::WakeReason {
        rustclr_hal::power::WakeReason::PowerOn
    }
}

/// The Core2's real-time clock lives on the PMIC and is not driven yet.
struct NoRtc;

impl rustclr_hal::rtc::Rtc for NoRtc {
    fn now(&mut self) -> HalResult<rustclr_hal::rtc::DateTime> {
        Err(HalError::NotSupported)
    }
    fn set(&mut self, _dt: rustclr_hal::rtc::DateTime) -> HalResult<()> {
        Err(HalError::NotSupported)
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

/// The Core2 as a [`rustclr_hal::Board`].
///
/// Only what this firmware can actually reach. Every peripheral the board has
/// but this driver does not drive answers `NotSupported`, which is what the
/// HAL report prints — a board that claimed a bus it cannot use would be worse
/// than one that says no.
pub struct M5Core2Board<'a> {
    panel: Option<M5Core2Panel<'a>>,
    delay: CoreDelay,
    watchdog: NoWatchdog,
    power: NoPower,
    rtc: NoRtc,
}

impl<'a> M5Core2Board<'a> {
    pub fn new(panel: Option<M5Core2Panel<'a>>) -> Self {
        Self {
            panel,
            delay: CoreDelay(Delay::new()),
            watchdog: NoWatchdog,
            power: NoPower,
            rtc: NoRtc,
        }
    }
}

macro_rules! unsupported {
    ($name:ident, $ret:ty) => {
        fn $name(&mut self, _n: u8) -> HalResult<&mut $ret> {
            Err(HalError::NotSupported)
        }
    };
}

impl rustclr_hal::Board for M5Core2Board<'_> {
    fn name(&self) -> &str {
        "M5Stack Core2"
    }

    fn gpio(&mut self, _pin: u32) -> HalResult<&mut dyn rustclr_hal::gpio::GpioPin> {
        // The pins this board exposes are behind the PMIC and the panel; a
        // general GPIO driver for the Xtensa ESP32 is not written yet.
        Err(HalError::NotSupported)
    }

    unsupported!(i2c, dyn rustclr_hal::i2c::I2cBus);
    unsupported!(spi, dyn rustclr_hal::spi::SpiBus);
    unsupported!(uart, dyn rustclr_hal::uart::Uart);
    unsupported!(i2s, dyn rustclr_hal::i2s::I2sBus);
    unsupported!(pwm, dyn rustclr_hal::pwm::PwmChannel);
    unsupported!(adc, dyn rustclr_hal::adc::AdcChannel);
    unsupported!(can, dyn rustclr_hal::can::CanBus);
    unsupported!(onewire, dyn rustclr_hal::onewire::OneWireBus);
    unsupported!(extmem, dyn rustclr_hal::extmem::ExtMemory);

    fn power(&mut self) -> &mut dyn rustclr_hal::power::PowerManager {
        &mut self.power
    }

    fn delay(&mut self) -> &mut dyn rustclr_hal::delay::Delay {
        &mut self.delay
    }

    fn rtc(&mut self) -> &mut dyn rustclr_hal::rtc::Rtc {
        &mut self.rtc
    }

    fn watchdog(&mut self) -> &mut dyn rustclr_hal::watchdog::Watchdog {
        &mut self.watchdog
    }

    fn netif(
        &mut self,
        _kind: rustclr_hal::netif::NetIfKind,
    ) -> HalResult<&mut dyn rustclr_hal::netif::NetInterface> {
        // The radio needs `esp-wifi`, which this firmware does not carry.
        // Saying so beats a `Wifi.Connect` that reports a join it never made.
        Err(HalError::NotSupported)
    }

    fn signal(&mut self, _pin: u32) -> HalResult<&mut dyn rustclr_hal::signal::SignalControl> {
        Err(HalError::NotSupported)
    }

    fn present_frame(&mut self, rgb565: &[u16], width: u32, height: u32) -> HalResult<()> {
        match self.panel.as_mut() {
            Some(panel) => panel.present(rgb565, width, height),
            // No panel brought up: the framebuffer is still whole and still
            // drawable, so this is not an error.
            None => Ok(()),
        }
    }
}
