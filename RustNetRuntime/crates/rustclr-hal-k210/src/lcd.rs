//! ST7789V panel over SPI0 in octal mode — the LCD on a Sipeed Maix board.
//!
//! This panel is not wired the way an ST7789V usually is. There is no MOSI: the
//! controller talks to it over an 8-bit parallel bus, which the K210 drives by
//! putting SPI0 in **octal** frame format and routing its eight data lines to
//! the DVP (camera-interface) pins with [`sysctl::set_spi0_dvp_data`]. Only two
//! signals go through FPIOA pads — chip select and the clock — plus two ordinary
//! GPIOHS outputs for reset and the data/command line.
//!
//! Consequences worth knowing before reading the code:
//!
//! * **A byte costs one clock.** Eight lines, eight bits. So the pixel rate is
//!   the SPI clock, not an eighth of it, and a 320×240 frame at 15 MHz takes
//!   about 10 ms of wire time.
//! * **The camera shares those pins.** A DVP driver and this cannot stream at
//!   the same time.
//! * **`dcx` is not part of the bus.** It is sampled by the panel per transfer,
//!   so it must be settled before chip select falls and left alone until the
//!   transfer ends. Every command here is therefore its own SPI transaction.
//!
//! **The controller is reconfigured before every transfer, and that is the
//! whole trick.** A command is an 8-bit *instruction*, its parameters are 8-bit
//! *address* units, and pixels are 32-bit units — and `spi_ctrlr0` has to say
//! which, every time, alongside the frame width. This driver spent a long while
//! setting one value for everything and getting a panel that accepted `SLPOUT`
//! and `DISPON` and then showed a single flat colour forever. The sequence and
//! the encodings here are MaixPy's `st7789.c` and `lcd_mcu.c`, mirrored,
//! because those demonstrably drive this screen.

use rustclr_hal::gpio::{GpioPin, Level, PinMode};
use rustclr_hal::{HalError, HalResult};

use crate::delay::CycleDelay;
use crate::gpio::K210Pin;
use crate::spi::{FrameFormat, K210Spi, SPI0};
use crate::{fpioa, sysctl};

use rustclr_hal::delay::Delay;

/// Panel commands used here. The ST7789V's set is much larger; these are the
/// ones an initialise-and-blit driver needs.
mod cmd {
    pub const SOFTWARE_RESET: u8 = 0x01;
    pub const READ_DISPLAY_ID: u8 = 0x04;
    pub const SLEEP_OFF: u8 = 0x11;
    pub const NORMAL_DISPLAY_ON: u8 = 0x13;
    pub const INVERSION_ON: u8 = 0x21;
    pub const DISPLAY_ON: u8 = 0x29;
    pub const COLUMN_ADDRESS_SET: u8 = 0x2A;
    pub const PAGE_ADDRESS_SET: u8 = 0x2B;
    pub const MEMORY_WRITE: u8 = 0x2C;
    pub const MEMORY_ACCESS_CONTROL: u8 = 0x36;
    pub const PIXEL_FORMAT_SET: u8 = 0x3A;
}

/// `MADCTL` for a landscape 320×240 with the origin top-left.
///
/// Bit 5 (`MV`) swaps the row and column axes, which is what turns the panel's
/// native 240×320 portrait into landscape; bit 6 (`MX`) mirrors columns so the
/// origin ends up at the corner nearest the board's edge rather than opposite
/// it. If the image arrives mirrored or upside down, this constant is the one
/// thing to change — the rest of the driver is orientation-agnostic.
const MADCTL_LANDSCAPE: u8 = 0x20 | 0x40;

/// 16 bits per pixel, RGB565, for both the MCU and RGB interfaces.
const PIXEL_FORMAT_16BIT: u8 = 0x55;

/// Clock for the panel. Fast enough that a full frame is ~10 ms of wire time,
/// conservative enough to leave margin on a bus whose data lines are shared
/// with the camera pins and were never intended to be pushed.
pub const DEFAULT_CLOCK_HZ: u32 = 15_000_000;

/// Pads and channels the panel occupies on a Maix board.
#[derive(Clone, Copy)]
pub struct PanelPins {
    /// Pad carrying `SPI0_SS3`.
    pub cs_pad: u8,
    /// Pad carrying `SPI0_SCLK`.
    pub sclk_pad: u8,
    /// Pad for the data/command line, driven as GPIOHS.
    pub dc_pad: u8,
    /// Pad for the panel's reset, driven as GPIOHS.
    pub rst_pad: u8,
    /// GPIOHS channel to bind to `dc_pad`.
    pub dc_channel: u8,
    /// GPIOHS channel to bind to `rst_pad`.
    pub rst_channel: u8,
    /// Whether `spi_dvp_data_enable` is set, putting SPI0's eight data lines on
    /// the camera interface's pins.
    ///
    /// Kendryte's examples set it unconditionally, and a running MaixPy has it
    /// set while driving this panel, so the Maix boards want it — the board
    /// schematic showing a separate "MCU 8-bit LCD" pin group notwithstanding.
    pub data_on_dvp_pins: bool,
    /// Whether this board's panel wants its output inverted (`INVON`).
    pub invert: bool,
}

/// The Maix wiring: CS 36, RST 37, DC 38, SCLK 39. Same on Maix Go, Maix Bit
/// and Maix Dock — the panel hangs off a fixed header rather than being routed
/// per board.
pub const MAIX_PANEL: PanelPins = PanelPins {
    cs_pad: 36,
    sclk_pad: 39,
    dc_pad: 38,
    rst_pad: 37,
    dc_channel: 8,
    rst_channel: 9,
    data_on_dvp_pins: true,
    // MaixPy's Maix board config leaves `invert` clear.
    invert: false,
};

/// Which `ss` line the panel sits on. `SPI0_SS3` is what the Maix boards route.
const PANEL_CHIP_SELECT: u8 = 3;

// `spi_ctrlr0` — and it is **not one value**. That was the mistake this driver
// spent a long time making.
//
// MaixPy's `st7789.c` reconfigures the controller before every transfer, and
// the enhanced-format register goes with it. Kendryte's encoding is
// `(wait << 11) | (inst_code << 8) | ((addr_bits / 4) << 2) | trans`, where the
// instruction code is 0/1/2/3 for 0/4/8/16 bits and `trans = 2` is
// `SPI_AITM_AS_FRAME_FORMAT` — instruction and address go out in the same wide
// format as the data rather than in one-bit SPI.
//
// Reading `0x22` out of a *running* MaixPy and using it for everything is what
// went wrong: that is the value left behind by its last pixel write, and it is
// meaningless for a command. Each kind of transfer needs its own.

/// A command byte: an 8-bit instruction, no address.
const CTRLR0_COMMAND: u32 = (2 << 8) | 2;
/// Command parameters: no instruction, 8-bit "address" carrying the bytes.
const CTRLR0_BYTES: u32 = ((8 / 4) << 2) | 2;
/// Pixels: no instruction, 32-bit units, two RGB565 pixels to a frame.
const CTRLR0_WORDS: u32 = ((32 / 4) << 2) | 2;

pub struct St7789 {
    spi: K210Spi,
    dc: K210Pin,
    rst: K210Pin,
    delay: CycleDelay,
    width: u16,
    height: u16,
    /// Whether the eight data lines go to the camera interface's pins instead
    /// of the dedicated LCD group. Board-specific: see [`St7789::init`].
    route_data_to_dvp: bool,
    /// Whether the panel wants `INVON`. A board property, not a panel one.
    invert: bool,
}

impl St7789 {
    /// Mux the pads and take ownership of SPI0. Does not touch the panel — call
    /// [`St7789::init`] for that.
    pub fn new(pins: PanelPins, spi_source_hz: u32, cpu_hz: u32) -> Self {
        sysctl::clock_enable(sysctl::Peripheral::Fpioa);
        fpioa::set_function(pins.cs_pad, fpioa::SPI0_SS3);
        fpioa::set_function(pins.sclk_pad, fpioa::SPI0_SCLK);
        fpioa::set_function(pins.dc_pad, fpioa::gpiohs(pins.dc_channel));
        fpioa::set_function(pins.rst_pad, fpioa::gpiohs(pins.rst_channel));

        // The panel's pins do not live on the 3.3 V default.
        //
        // Sipeed's datasheet marks IO36..IO39 — chip select, reset, `dcx` and
        // the write strobe — as **1.8 V**, and the board schematic puts them in
        // bank 6 (the FPIOA banks are six pads each, not eight). Banks 6 and 7
        // are also the pair Kendryte's LCD and camera examples set, with the
        // comment "Set dvp and spi pin to 1.8V", and a running MaixPy has
        // exactly `power_sel = 0xc0`.
        //
        // Getting this wrong does not fail loudly: every SPI transfer still
        // completes, and the screen sits white with its backlight on.
        sysctl::set_power_18v(sysctl::PowerBank::Dvp0);
        sysctl::set_power_18v(sysctl::PowerBank::Dvp1);

        let mut spi = K210Spi::new(SPI0, spi_source_hz);
        spi.set_chip_select(PANEL_CHIP_SELECT);
        spi.set_frame_format(FrameFormat::Octal);

        let mut dc = K210Pin::new(pins.dc_channel);
        dc.pad = pins.dc_pad;
        let mut rst = K210Pin::new(pins.rst_channel);
        rst.pad = pins.rst_pad;

        Self {
            spi,
            dc,
            rst,
            delay: CycleDelay::new(cpu_hz),
            width: 320,
            height: 240,
            route_data_to_dvp: pins.data_on_dvp_pins,
            invert: pins.invert,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Bring the panel up. Safe to call twice.
    pub fn init(&mut self, clock_hz: u32) -> HalResult<()> {
        sysctl::clock_enable(sysctl::Peripheral::Spi0);
        sysctl::reset(sysctl::Peripheral::Spi0);
        // The eight data lines are switched over here, after the controller
        // has been reset — resetting SPI0 while it owns those pins is an
        // ordering that either does not matter or matters completely, and the
        // safe order costs nothing.
        sysctl::set_spi0_dvp_data(self.route_data_to_dvp);
        self.spi.set_target_hz(clock_hz);

        self.dc.set_mode(PinMode::Output)?;
        self.rst.set_mode(PinMode::Output)?;

        // A hardware reset first: the panel may be mid-frame from whatever ran
        // on this board before us, and the software reset below is only
        // honoured once it is listening.
        self.rst.write(Level::Low)?;
        self.delay.delay_ms(20);
        self.rst.write(Level::High)?;
        self.delay.delay_ms(50);

        // The sequence and the delays are MaixPy's `mcu_lcd_init`, including
        // the 120 ms after leaving sleep — the ST7789V's datasheet minimum, and
        // longer than the 100 ms guessed here before — and `NORMAL_DISPLAY_ON`,
        // which was missing entirely.
        self.command(cmd::SOFTWARE_RESET, &[])?;
        self.delay.delay_ms(50);
        self.command(cmd::SLEEP_OFF, &[])?;
        self.delay.delay_ms(120);
        self.command(cmd::PIXEL_FORMAT_SET, &[PIXEL_FORMAT_16BIT])?;
        self.delay.delay_ms(10);
        self.command(cmd::MEMORY_ACCESS_CONTROL, &[MADCTL_LANDSCAPE])?;
        // Inversion is a *board* property, and MaixPy sends it only when its
        // board config asks for it. Sending it unconditionally, as this driver
        // used to, complements every colour on a panel that does not want it.
        if self.invert {
            self.command(cmd::INVERSION_ON, &[])?;
            self.delay.delay_ms(10);
        }
        self.command(cmd::NORMAL_DISPLAY_ON, &[])?;
        self.delay.delay_ms(10);
        self.command(cmd::DISPLAY_ON, &[])?;
        self.delay.delay_ms(20);
        Ok(())
    }

    /// One command byte with `dcx` low, then its parameters with `dcx` high.
    ///
    /// The controller is reconfigured for each half, which is the part that
    /// matters: a command is an 8-bit *instruction* and its parameters are
    /// 8-bit *address* units, and the enhanced-format register has to say so.
    /// MaixPy re-runs `spi_init` and `spi_init_non_standard` before every
    /// single transfer for exactly this reason.
    fn command(&mut self, command: u8, params: &[u8]) -> HalResult<()> {
        self.dc.write(Level::Low)?;
        self.spi.set_frame_bits(8)?;
        self.spi.set_non_standard(CTRLR0_COMMAND);
        // Sent as *data* with no preceding command, so chip select is asserted
        // before the byte is pushed rather than after — the order Kendryte's
        // driver uses.
        self.spi.write_after_command(&[], &[command])?;

        if !params.is_empty() {
            self.dc.write(Level::High)?;
            self.spi.set_frame_bits(8)?;
            self.spi.set_non_standard(CTRLR0_BYTES);
            self.spi.write_after_command(&[], params)?;
        }
        Ok(())
    }

    /// Point the panel's GRAM cursor at a rectangle and leave it expecting
    /// pixels.
    fn set_window(&mut self, x: u16, y: u16, w: u16, h: u16) -> HalResult<()> {
        if w == 0 || h == 0 {
            return Err(HalError::InvalidArgument("empty window"));
        }
        let x1 = x + w - 1;
        let y1 = y + h - 1;
        self.command(
            cmd::COLUMN_ADDRESS_SET,
            &[(x >> 8) as u8, x as u8, (x1 >> 8) as u8, x1 as u8],
        )?;
        self.command(cmd::PAGE_ADDRESS_SET, &[(y >> 8) as u8, y as u8, (y1 >> 8) as u8, y1 as u8])?;
        self.command(cmd::MEMORY_WRITE, &[])?;
        Ok(())
    }

    /// Blit a full-size RGB565 frame.
    pub fn present(&mut self, pixels: &[u16], width: u32, height: u32) -> HalResult<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        if width > self.width as u32 || height > self.height as u32 {
            return Err(HalError::InvalidArgument("frame is larger than the panel"));
        }
        let expected = (width * height) as usize;
        if pixels.len() < expected {
            return Err(HalError::InvalidArgument("frame is shorter than its dimensions"));
        }

        self.set_window(0, 0, width as u16, height as u16)?;
        // Pixels, so `dcx` stays high for the whole run — and the controller
        // switches to 32-bit units, which needs its own `spi_ctrlr0`.
        self.dc.write(Level::High)?;
        self.spi.set_non_standard(CTRLR0_WORDS);

        // An odd-width frame would leave one pixel over per row, and the pair
        // packing has nowhere to put it. Rows are sent whole when the width is
        // even — which every mode this panel has is — and one row at a time
        // otherwise, with the stray pixel repeated into the pair. Repeating
        // beats dropping: a one-pixel-wide seam is invisible, a shifted row is
        // not.
        if width % 2 == 0 {
            // The whole frame in one transfer, which is what MaixPy's
            // `lcd_draw_picture` does.
            //
            // Splitting it per row was a debugging artefact from when nothing
            // reached the panel at all, and it leaves a visible mark: each
            // transfer declares a 32-bit address unit, so whatever the
            // controller takes off the front of one costs *every* row rather
            // than the frame once — and the image comes out rotated
            // horizontally, with what should sit at the right edge reappearing
            // on the left.
            self.spi.write_rgb565(&pixels[..expected])
        } else {
            let mut pair = [0u16; 2];
            for row in 0..height as usize {
                let start = row * width as usize;
                let body = width as usize - 1;
                self.spi.write_rgb565(&pixels[start..start + body])?;
                pair[0] = pixels[start + body];
                pair[1] = pixels[start + body];
                self.spi.write_rgb565(&pair)?;
            }
            Ok(())
        }
    }

    /// Blink the reset line, which on a MaixLCD blinks the backlight with it.
    ///
    /// The board's schematic hangs the backlight rail off a PMOS switched by an
    /// S8050 whose base is driven from `LCD_RST`: hold reset high and the
    /// backlight comes on, pull it low and the panel goes dark. That makes this
    /// the one signal whose arrival can be *seen* without the panel having to
    /// understand anything.
    ///
    /// So it separates the last two possibilities. A backlight that blinks in
    /// step proves the control lines reach the board, which confines the fault
    /// to the eight data lines — and since a command byte travels on those same
    /// lines in an 8080 interface, dead data means the panel never received an
    /// instruction at all, which is exactly what a white screen looks like. A
    /// backlight that ignores it means nothing is getting through.
    pub fn blink_backlight(&mut self, times: u32, ms: u64) -> HalResult<()> {
        self.rst.set_mode(PinMode::Output)?;
        for _ in 0..times {
            self.rst.write(Level::Low)?;
            self.delay.delay_ms(ms);
            self.rst.write(Level::High)?;
            self.delay.delay_ms(ms);
        }
        Ok(())
    }

    /// Ask the panel who it is: `RDDID`, one dummy byte then three id bytes.
    ///
    /// The only reading this driver can take without someone looking at the
    /// screen, and the answer is worth having either way. An ST7789V replies
    /// with a manufacturer byte and two version/driver bytes; all-`0x00` or
    /// all-`0xFF` means the data lines are not connected in the direction this
    /// is asking, which — since they are the same eight lines writes go out on
    /// — is a strong hint that nothing is reaching the panel at all.
    ///
    /// Not proof on its own: a panel wired write-only would also answer with
    /// nothing while accepting commands perfectly well.
    pub fn read_id(&mut self) -> HalResult<[u8; 4]> {
        let mut id = [0u8; 4];
        self.dc.write(Level::Low)?;
        self.spi.set_frame_bits(8)?;
        self.spi.set_non_standard(CTRLR0_COMMAND);
        self.spi.read_after_command(&[cmd::READ_DISPLAY_ID], &mut id)?;
        Ok(id)
    }

    /// Walk the panel through solid colours, holding each one.
    ///
    /// The one diagnostic that separates the two ways a panel fails. A screen
    /// that goes red, then green, then blue has a working bus, and anything
    /// still wrong is a colour or orientation setting — one constant each. A
    /// screen that stays uniformly white through all of them is not being
    /// driven at all, and the answer is upstream of this driver: the power
    /// domain, the DVP routing, or the clock.
    ///
    /// Worth the two seconds it costs at boot on a port this young.
    pub fn test_pattern(&mut self, hold_ms: u64) -> HalResult<()> {
        for colour in [0xF800u16, 0x07E0, 0x001F, 0xFFFF, 0x0000] {
            self.fill(colour)?;
            self.delay.delay_ms(hold_ms);
        }
        Ok(())
    }

    /// Fill the whole panel with one colour, without needing a framebuffer.
    /// Used at bring-up: a panel that goes solid red on command has its clock,
    /// its chip select and its `dcx` all correct.
    ///
    /// Sent as a row-sized band repeated, which for anything but a flat colour
    /// would come out rotated — see [`St7789::present`], which is why that one
    /// sends the frame in a single transfer. A uniform colour is exactly the
    /// case where the difference cannot show, which is also why this hid the
    /// problem for as long as it did.
    pub fn fill(&mut self, colour: u16) -> HalResult<()> {
        self.set_window(0, 0, self.width, self.height)?;
        self.dc.write(Level::High)?;
        self.spi.set_non_standard(CTRLR0_WORDS);
        let mut band = [0u16; 320];
        band.fill(colour);
        for _ in 0..self.height {
            self.spi.write_rgb565(&band)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel's pads are fixed by the Maix header, and getting `cs` and
    /// `sclk` the wrong way round produces a bus that clocks into nothing.
    #[test]
    fn the_maix_panel_sits_on_the_documented_pads() {
        assert_eq!(MAIX_PANEL.cs_pad, 36);
        assert_eq!(MAIX_PANEL.rst_pad, 37);
        assert_eq!(MAIX_PANEL.dc_pad, 38);
        assert_eq!(MAIX_PANEL.sclk_pad, 39);
    }

    /// `SPI0_SS3` is a specific FPIOA function number, not "the SPI0 chip
    /// select"; SS0 is a different pad function and the panel does not answer
    /// on it.
    #[test]
    fn chip_select_names_the_fourth_ss_line() {
        assert_eq!(fpioa::SPI0_SS3, fpioa::SPI0_SS0 + 3);
        assert_eq!(PANEL_CHIP_SELECT, 3);
    }

    #[test]
    fn landscape_swaps_the_axes() {
        // MV, the axis exchange, is what makes 240x320 into 320x240.
        assert_ne!(MADCTL_LANDSCAPE & 0x20, 0, "MV must be set for landscape");
    }

    /// Off-chip the register writes are no-ops, so this exercises the argument
    /// checking rather than the panel: a frame that does not fit must be
    /// refused rather than blitted past the end of the buffer.
    #[test]
    fn oversized_and_short_frames_are_refused() {
        let mut panel = St7789::new(MAIX_PANEL, 100_000_000, 390_000_000);
        let pixels = [0u16; 16];
        assert!(panel.present(&pixels, 400, 240).is_err(), "wider than the panel");
        assert!(panel.present(&pixels, 320, 240).is_err(), "buffer too short for its size");
        assert!(panel.present(&pixels, 0, 0).is_ok(), "an empty frame is nothing to do");
    }
}
