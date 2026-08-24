//! FPIOA — the Field Programmable IO Array.
//!
//! **Nothing on this chip has a pinout.** Any of the 48 pads can carry any of
//! 256 peripheral functions, so a peripheral is not "on" a pin until FPIOA has
//! been told to route it there — unlike the STM32, where each pin offers a
//! short fixed menu of alternate functions. Every driver in this crate
//! therefore starts by muxing its pads, and every board pin number in the
//! firmware is an *FPIOA pad* number (IO0..IO47), not a port/index pair.
//!
//! Each pad has one 32-bit configuration word: the function number in the low
//! byte, and above it the pad's electrical setup — output enable, input enable,
//! drive strength, pulls, Schmitt trigger, inversions. Getting those right per
//! function matters and is not guessable: a `UARTHS_TX` pad wants its output
//! driver on and its input off, a `GPIOHS` pad wants *both* on so the GPIOHS
//! peripheral's own `output_en` can pick the direction, and an I²C pad wants
//! open-drain with a pull-up. So rather than derive them, [`FUNCTION_DEFAULTS`]
//! carries Kendryte's own table verbatim — 256 words, one per function,
//! straight out of `fpioa.c` in the kendryte-standalone-sdk (BSD-3-Clause),
//! by way of the `k210-hal` crate which transcribed it first.

use crate::reg;

const FPIOA_BASE: usize = 0x502B_0000;

/// Pads IO0..IO47.
pub const PAD_COUNT: u8 = 48;

// Function numbers. Only the ones this crate routes are named; the table below
// covers all 256. The K210 datasheet numbers them in this order, which is why
// `gpiohs(n)` and `uart(port, ..)` can be arithmetic rather than lookups.

/// SPI0's first data line — the transmit line in ordinary one-bit SPI.
pub const SPI0_D0: u8 = 4;
pub const SPI0_SS0: u8 = 12;
/// The `ss` line the Maix boards route their panel to.
pub const SPI0_SS3: u8 = 15;
pub const SPI0_SCLK: u8 = 17;
pub const UARTHS_RX: u8 = 18;
pub const UARTHS_TX: u8 = 19;
pub const GPIOHS0: u8 = 24;
pub const GPIO0: u8 = 56;
pub const UART1_RX: u8 = 64;
pub const UART1_TX: u8 = 65;
pub const SPI1_D0: u8 = 70;
pub const SPI1_D1: u8 = 71;
pub const SPI1_SS0: u8 = 78;
pub const SPI1_SCLK: u8 = 83;
/// Reserved: what a pad is set to when it is released.
/// The camera interface. `SCCB_*` is the sensor's control channel — an I²C
/// look-alike driven by a master inside the DVP block, not by any of the three
/// I²C controllers.
/// The sensor's control bus. A Maix Go wires it to I²C2 — *not* to the
/// `SCCB_*` functions, despite the pads being labelled `DVP_SDA`/`DVP_SCL`.
pub const I2C2_SCLK: u8 = 130;
pub const I2C2_SDA: u8 = 131;
pub const CMOS_XCLK: u8 = 132;
pub const CMOS_RST: u8 = 133;
pub const CMOS_PWDN: u8 = 134;
pub const CMOS_VSYNC: u8 = 135;
pub const CMOS_HREF: u8 = 136;
pub const CMOS_PCLK: u8 = 137;
pub const SCCB_SCLK: u8 = 146;
pub const SCCB_SDA: u8 = 147;

pub const RESV0: u8 = 120;

/// FPIOA function for high-speed GPIO channel `channel` (0..=31).
pub const fn gpiohs(channel: u8) -> u8 {
    GPIOHS0 + channel
}

/// FPIOA function for conventional GPIO channel `channel` (0..=7).
pub const fn gpio(channel: u8) -> u8 {
    GPIO0 + channel
}

/// `(rx, tx)` functions for UART1..UART3. The three ports' RX/TX pairs sit two
/// apart in the function list.
pub const fn uart(port: u8) -> (u8, u8) {
    let offset = (port - 1) * 2;
    (UART1_RX + offset, UART1_TX + offset)
}

/// `(d0, d1, sclk, ss0)` functions for SPI1. SPI0's are named individually
/// because only its clock and chip-selects get muxed here — its data lines
/// belong to whatever panel the board wires up.
pub const fn spi1() -> (u8, u8, u8, u8) {
    (SPI1_D0, SPI1_D1, SPI1_SCLK, SPI1_SS0)
}

/// Route `function` to `pad`, with Kendryte's electrical setup for it.
///
/// Silently ignores an out-of-range pad rather than panicking: this runs during
/// board bring-up, where a panic is far harder to diagnose than a dead pin, and
/// the callers all pass compile-time constants.
pub fn set_function(pad: u8, function: u8) {
    if pad >= PAD_COUNT {
        return;
    }
    reg::write(
        FPIOA_BASE + 4 * pad as usize,
        FUNCTION_DEFAULTS[function as usize],
    );
}

/// Release a pad, so a later `set_function` starts from a known state.
pub fn clear_function(pad: u8) {
    set_function(pad, RESV0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pull {
    None,
    Down,
    Up,
}

const PU_BIT: u32 = 1 << 16;
const PD_BIT: u32 = 1 << 17;

/// Override the pad's pull, leaving the rest of the function's setup alone.
/// Call *after* [`set_function`] — that writes the whole word.
pub fn set_pull(pad: u8, pull: Pull) {
    if pad >= PAD_COUNT {
        return;
    }
    let addr = FPIOA_BASE + 4 * pad as usize;
    match pull {
        Pull::None => reg::modify(addr, PU_BIT | PD_BIT, 0),
        Pull::Down => reg::modify(addr, PU_BIT, PD_BIT),
        Pull::Up => reg::modify(addr, PD_BIT, PU_BIT),
    }
}

/// The function currently routed to `pad`, or `None` if the pad is out of
/// range. Reads back the low byte of the configuration word, which is the one
/// piece of FPIOA state worth reporting: it says what a pin is actually doing.
pub fn function_of(pad: u8) -> Option<u8> {
    if pad >= PAD_COUNT {
        return None;
    }
    Some((reg::read(FPIOA_BASE + 4 * pad as usize) & 0xFF) as u8)
}

/// Per-function pad configuration, indexed by function number.
///
/// Transcribed from `fpioa_function_config` in Kendryte's
/// kendryte-standalone-sdk (`lib/drivers/fpioa.c`, BSD-3-Clause). The low byte
/// of each word is the function number itself, so the table doubles as a
/// consistency check — `FUNCTION_DEFAULTS[n] & 0xFF == n` for every entry, and
/// a test below asserts exactly that.
#[rustfmt::skip]
pub static FUNCTION_DEFAULTS: [u32; 256] = [
    0x00900000, 0x00900001, 0x00900002, 0x00001f03, 0x00b03f04, 0x00b03f05, 0x00b03f06, 0x00b03f07,
    0x00b03f08, 0x00b03f09, 0x00b03f0a, 0x00b03f0b, 0x00001f0c, 0x00001f0d, 0x00001f0e, 0x00001f0f,
    0x03900010, 0x00001f11, 0x00900012, 0x00001f13, 0x00900014, 0x00900015, 0x00001f16, 0x00001f17,
    0x00901f18, 0x00901f19, 0x00901f1a, 0x00901f1b, 0x00901f1c, 0x00901f1d, 0x00901f1e, 0x00901f1f,
    0x00901f20, 0x00901f21, 0x00901f22, 0x00901f23, 0x00901f24, 0x00901f25, 0x00901f26, 0x00901f27,
    0x00901f28, 0x00901f29, 0x00901f2a, 0x00901f2b, 0x00901f2c, 0x00901f2d, 0x00901f2e, 0x00901f2f,
    0x00901f30, 0x00901f31, 0x00901f32, 0x00901f33, 0x00901f34, 0x00901f35, 0x00901f36, 0x00901f37,
    0x00901f38, 0x00901f39, 0x00901f3a, 0x00901f3b, 0x00901f3c, 0x00901f3d, 0x00901f3e, 0x00901f3f,
    0x00900040, 0x00001f41, 0x00900042, 0x00001f43, 0x00900044, 0x00001f45, 0x00b03f46, 0x00b03f47,
    0x00b03f48, 0x00b03f49, 0x00b03f4a, 0x00b03f4b, 0x00b03f4c, 0x00b03f4d, 0x00001f4e, 0x00001f4f,
    0x00001f50, 0x00001f51, 0x03900052, 0x00001f53, 0x00b03f54, 0x00900055, 0x00900056, 0x00001f57,
    0x00001f58, 0x00001f59, 0x0090005a, 0x0090005b, 0x0090005c, 0x0090005d, 0x00001f5e, 0x00001f5f,
    0x00001f60, 0x00001f61, 0x00001f62, 0x00001f63, 0x00001f64, 0x00900065, 0x00900066, 0x00900067,
    0x00900068, 0x00001f69, 0x00001f6a, 0x00001f6b, 0x00001f6c, 0x00001f6d, 0x00001f6e, 0x00001f6f,
    0x00900070, 0x00900071, 0x00900072, 0x00900073, 0x00001f74, 0x00001f75, 0x00001f76, 0x00001f77,
    0x00000078, 0x00000079, 0x0000007a, 0x0000007b, 0x0000007c, 0x0000007d, 0x0099107e, 0x0099107f,
    0x00991080, 0x00991081, 0x00991082, 0x00991083, 0x00001f84, 0x00001f85, 0x00001f86, 0x00900087,
    0x00900088, 0x00900089, 0x0090008a, 0x0090008b, 0x0090008c, 0x0090008d, 0x0090008e, 0x0090008f,
    0x00900090, 0x00900091, 0x00993092, 0x00993093, 0x00900094, 0x00900095, 0x00900096, 0x00900097,
    0x00900098, 0x00001f99, 0x00001f9a, 0x00001f9b, 0x00001f9c, 0x00001f9d, 0x00001f9e, 0x00001f9f,
    0x00001fa0, 0x00001fa1, 0x009000a2, 0x009000a3, 0x009000a4, 0x009000a5, 0x009000a6, 0x00001fa7,
    0x00001fa8, 0x00001fa9, 0x00001faa, 0x00001fab, 0x00001fac, 0x00001fad, 0x00001fae, 0x00001faf,
    0x009000b0, 0x009000b1, 0x009000b2, 0x009000b3, 0x009000b4, 0x00001fb5, 0x00001fb6, 0x00001fb7,
    0x00001fb8, 0x00001fb9, 0x00001fba, 0x00001fbb, 0x00001fbc, 0x00001fbd, 0x00001fbe, 0x00001fbf,
    0x00001fc0, 0x00001fc1, 0x00001fc2, 0x00001fc3, 0x00001fc4, 0x00001fc5, 0x00001fc6, 0x00001fc7,
    0x00001fc8, 0x00001fc9, 0x00001fca, 0x00001fcb, 0x00001fcc, 0x00001fcd, 0x00001fce, 0x00001fcf,
    0x00001fd0, 0x00001fd1, 0x00001fd2, 0x00001fd3, 0x00001fd4, 0x009000d5, 0x009000d6, 0x009000d7,
    0x009000d8, 0x009100d9, 0x00991fda, 0x009000db, 0x009000dc, 0x009000dd, 0x000000de, 0x009000df,
    0x00001fe0, 0x00001fe1, 0x00001fe2, 0x00001fe3, 0x00001fe4, 0x00001fe5, 0x00001fe6, 0x00001fe7,
    0x00001fe8, 0x00001fe9, 0x00001fea, 0x00001feb, 0x00001fec, 0x00001fed, 0x00001fee, 0x00001fef,
    0x00001ff0, 0x00001ff1, 0x00001ff2, 0x00001ff3, 0x00001ff4, 0x00001ff5, 0x00001ff6, 0x00001ff7,
    0x00001ff8, 0x00001ff9, 0x00001ffa, 0x00001ffb, 0x00001ffc, 0x00001ffd, 0x00001ffe, 0x00001fff,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every word's low byte is its own function number. A transcription slip —
    /// a duplicated or dropped line in a 32-row table — shows up here rather
    /// than as a pad that quietly does the wrong thing on hardware.
    #[test]
    fn every_entry_carries_its_own_function_number() {
        for (n, word) in FUNCTION_DEFAULTS.iter().enumerate() {
            assert_eq!(word & 0xFF, n as u32, "entry {n} has the wrong ch_sel");
        }
    }

    /// The pad setup for the functions this crate routes, decoded. These are
    /// the bits that decide whether a pin drives, listens, or does neither, so
    /// spelling out the expectation documents the electrical intent as well as
    /// guarding the table.
    #[test]
    fn output_and_input_enables_match_the_direction_each_function_needs() {
        const OE: u32 = 1 << 12;
        const IE: u32 = 1 << 20;

        // Transmit-only: drives, never listens.
        for f in [UARTHS_TX, UART1_TX, SPI0_SCLK, SPI1_SCLK, SPI0_SS0, SPI1_SS0] {
            let w = FUNCTION_DEFAULTS[f as usize];
            assert_eq!(w & OE, OE, "function {f} should drive");
            assert_eq!(w & IE, 0, "function {f} should not listen");
        }

        // Receive-only: listens, never drives.
        for f in [UARTHS_RX, UART1_RX] {
            let w = FUNCTION_DEFAULTS[f as usize];
            assert_eq!(w & OE, 0, "function {f} should not drive");
            assert_eq!(w & IE, IE, "function {f} should listen");
        }

        // GPIOHS and the SPI data lines are bidirectional at the pad, because
        // the direction is chosen downstream — by GPIOHS's own `output_en` for
        // a pin, and by the transfer mode for SPI.
        for f in [gpiohs(0), gpiohs(31), gpio(0), SPI1_D0, SPI1_D1] {
            let w = FUNCTION_DEFAULTS[f as usize];
            assert_eq!(w & OE, OE, "function {f} should be able to drive");
            assert_eq!(w & IE, IE, "function {f} should be able to listen");
        }
    }

    #[test]
    fn function_arithmetic_lands_on_the_right_numbers() {
        assert_eq!(gpiohs(0), 24);
        assert_eq!(gpiohs(31), 55);
        assert_eq!(gpio(7), 63);
        assert_eq!(uart(1), (64, 65));
        assert_eq!(uart(2), (66, 67));
        assert_eq!(uart(3), (68, 69));
    }
}
