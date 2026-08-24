//! SYSCTL — clock tree and peripheral gating.
//!
//! This crate **reads** the clock tree rather than programming it. The K210's
//! mask ROM has already brought PLL0 up and pointed the core at it by the time
//! our image runs (the ROM's own ISP talks over UARTHS at speed, so it has to),
//! and re-programming a PLL that is currently feeding the executing core is the
//! kind of operation that either works or hangs with nothing on the console to
//! say which. So [`Clocks::detect`] recovers the numbers actually in force and
//! everything downstream — UART divisors, SPI divisors, the microsecond clock —
//! scales off them.
//!
//! That also means the firmware's boot banner reports the real core frequency
//! on the first hardware run, which is the measurement worth having before
//! deciding whether the PLL needs touching at all.
//!
//! Register offsets and field positions are from the K210 datasheet, checked
//! against the `k210-pac` register description.

use crate::reg;

const SYSCTL_BASE: usize = 0x5044_0000;

const PLL0: usize = SYSCTL_BASE + 0x08;
const CLK_SEL0: usize = SYSCTL_BASE + 0x20;
const CLK_EN_CENT: usize = SYSCTL_BASE + 0x28;
const CLK_EN_PERI: usize = SYSCTL_BASE + 0x2C;
const PERI_RESET: usize = SYSCTL_BASE + 0x34;
const CLK_TH1: usize = SYSCTL_BASE + 0x3C;
const MISC: usize = SYSCTL_BASE + 0x54;
const POWER_SEL: usize = SYSCTL_BASE + 0x6C;

/// `misc.spi_dvp_data_enable` — routes SPI0's eight data lines to the DVP pins.
const MISC_SPI_DVP_DATA: u32 = 1 << 10;

/// The external crystal. 26 MHz on every K210 board Kendryte ever specified,
/// including the Maix line — the part's PLLs are documented against it and
/// there is no register that reports it, so this is the one number that has to
/// be assumed rather than read.
pub const IN0_HZ: u32 = 26_000_000;

// PLL0 fields.
const PLL0_CLKR: (u32, u32) = (0, 4);
const PLL0_CLKF: (u32, u32) = (4, 6);
const PLL0_CLKOD: (u32, u32) = (10, 4);
const PLL0_BYPASS: u32 = 1 << 23;
const PLL0_OUT_EN: u32 = 1 << 25;

// CLK_SEL0 fields.
const ACLK_SEL: (u32, u32) = (0, 1);
const ACLK_DIVIDER_SEL: (u32, u32) = (1, 2);
const APB0_CLK_SEL: (u32, u32) = (3, 3);
const APB1_CLK_SEL: (u32, u32) = (6, 3);
const APB2_CLK_SEL: (u32, u32) = (9, 3);
const SPI3_CLK_SEL: (u32, u32) = (12, 1);

/// A gate in `clk_en_peri` (and, at the same bit, a line in `peri_reset`).
/// Only the peripherals this crate drives are listed; the two registers share
/// one bit assignment, which is why one enum serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Peripheral {
    /// The camera interface. Named here not for cameras but for its *pins*:
    /// SPI0's eight data lines are routed onto them in octal mode, so a panel
    /// on that bus needs this block clocked and out of reset even though
    /// nothing is going to capture an image.
    Dvp = 3,
    Gpio = 5,
    Spi0 = 6,
    Spi1 = 7,
    Spi3 = 9,
    I2c0 = 13,
    I2c1 = 14,
    I2c2 = 15,
    Uart1 = 16,
    Uart2 = 17,
    Uart3 = 18,
    Fpioa = 20,
}

/// Ungate a peripheral's clock. Read-modify-write, because the register gates
/// everything else too.
pub fn clock_enable(which: Peripheral) {
    reg::modify(CLK_EN_PERI, 0, 1 << which as u32);
}

/// I/O power domains. One bit per bank in `power_sel`: clear is 3.3 V, set is
/// 1.8 V. Banks 0..5 cover the 48 FPIOA pads, eight each; banks 6 and 7 are the
/// dedicated DVP pins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PowerBank {
    /// Pads IO0..IO7.
    Fpioa0 = 0,
    Fpioa1 = 1,
    Fpioa2 = 2,
    Fpioa3 = 3,
    /// Pads IO32..IO39 — where a Maix panel's chip select and clock live.
    Fpioa4 = 4,
    Fpioa5 = 5,
    /// The DVP pins, which are also SPI0's eight data lines in octal mode.
    Dvp0 = 6,
    Dvp1 = 7,
}

/// Put an I/O bank on the 1.8 V domain.
///
/// Easy to miss and unforgiving when missed. Kendryte's own LCD and camera
/// examples open with banks 6 and 7 — the comment in their source is literally
/// "Set dvp and spi pin to 1.8V" — because those pins carry the panel's 8-bit
/// data bus. Left on 3.3 V the pads do not drive the panel at a level it
/// recognises, so it never sees a command, never leaves its reset state, and
/// sits there with the backlight on showing a **uniform white screen**. That
/// looks like a dead driver rather than a supply-voltage setting, which is why
/// it is worth naming here.
pub fn set_power_18v(bank: PowerBank) {
    reg::modify(POWER_SEL, 0, 1 << bank as u32);
}

/// Route SPI0's eight data lines to the DVP pins.
///
/// This is the one thing that makes an octal-SPI panel possible on a Maix
/// board, and it is not an FPIOA muxing: `SPI0_D0..D7` exist as FPIOA functions
/// too, but the LCD's 8-bit bus is wired to the camera-interface pins, which
/// FPIOA does not reach. Only `ss` and `sclk` go through pads — 36 and 39 on a
/// Maix Go — and the data lines are switched here instead.
///
/// The camera shares those pins, so a future DVP driver and the panel cannot
/// both be streaming at once.
pub fn set_spi0_dvp_data(enable: bool) {
    if enable {
        reg::modify(MISC, 0, MISC_SPI_DVP_DATA);
    } else {
        reg::modify(MISC, MISC_SPI_DVP_DATA, 0);
    }
}

/// Pulse a peripheral's reset line.
///
/// Deliberately **not** used for [`Peripheral::Spi3`]: that controller is the
/// path to the boot flash the ROM just read this image out of, and resetting it
/// buys nothing that configuring it does not.
pub fn reset(which: Peripheral) {
    let bit = 1 << which as u32;
    reg::modify(PERI_RESET, 0, bit);
    // A handful of cycles is enough for the reset to be observed; the SDK
    // does the same with an empty loop rather than a timed delay, because no
    // clock source is guaranteed to be running yet at this point in bring-up.
    for _ in 0..64 {
        core::hint::spin_loop();
    }
    reg::modify(PERI_RESET, bit, 0);
}

/// Make sure the APB buses and both SRAM banks are clocked. The ROM leaves
/// these on — it executed from SRAM to get here — but a firmware that gates one
/// off by accident fails in a way that looks like bad hardware.
pub fn enable_central_clocks() {
    // cpu | sram0 | sram1 | apb0 | apb1 | apb2
    reg::modify(CLK_EN_CENT, 0, 0b11_1111);
}

/// PLL0's output frequency, as configured.
///
/// `in0 / (clkr + 1) * (clkf + 1) / (clkod + 1)`. Bypassed or powered down, the
/// PLL passes the crystal straight through.
pub fn pll0_hz() -> u32 {
    let word = reg::read(PLL0);
    if word & PLL0_BYPASS != 0 || word & PLL0_OUT_EN == 0 {
        return IN0_HZ;
    }
    let clkr = reg::field(PLL0, PLL0_CLKR.0, PLL0_CLKR.1) + 1;
    let clkf = reg::field(PLL0, PLL0_CLKF.0, PLL0_CLKF.1) + 1;
    let clkod = reg::field(PLL0, PLL0_CLKOD.0, PLL0_CLKOD.1) + 1;
    pll_freq(IN0_HZ, clkr, clkf, clkod)
}

/// Split out of [`pll0_hz`] so the arithmetic is testable off-chip. Done in 64
/// bits because `26 MHz * 64` overflows a `u32` on the way to the division.
pub fn pll_freq(in_hz: u32, clkr: u32, clkf: u32, clkod: u32) -> u32 {
    let numerator = in_hz as u64 * clkf as u64;
    let denominator = clkr.max(1) as u64 * clkod.max(1) as u64;
    (numerator / denominator) as u32
}

/// The core (ACLK) frequency.
pub fn cpu_hz() -> u32 {
    if reg::field(CLK_SEL0, ACLK_SEL.0, ACLK_SEL.1) == 0 {
        return IN0_HZ;
    }
    let divider = reg::field(CLK_SEL0, ACLK_DIVIDER_SEL.0, ACLK_DIVIDER_SEL.1) * 2 + 2;
    pll0_hz() / divider
}

/// APB0/1/2, each a straight divide of the core clock. UART1..3 hang off APB0.
pub fn apb_hz(bus: u8) -> u32 {
    let field = match bus {
        0 => APB0_CLK_SEL,
        1 => APB1_CLK_SEL,
        _ => APB2_CLK_SEL,
    };
    cpu_hz() / (reg::field(CLK_SEL0, field.0, field.1) + 1)
}

/// The clock feeding SPI controller `bus` (0, 1 or 3).
///
/// SPI0/1/2 divide PLL0 by their `clk_th1` threshold; SPI3 picks between the
/// crystal and PLL0 and then halves the thresholded result. These are the
/// numbers the SDK's `sysctl_clock_get_freq` reports, and they only matter here
/// as the numerator of a baud divisor — [`crate::spi`] deliberately aims low
/// enough that being a factor out still lands inside every device's rating.
pub fn spi_hz(bus: u8) -> u32 {
    let threshold = |shift: u32| reg::field(CLK_TH1, shift, 8) + 1;
    match bus {
        0 => pll0_hz() / threshold(0),
        1 => pll0_hz() / threshold(8),
        2 => pll0_hz() / threshold(16),
        _ => {
            let source = if reg::field(CLK_SEL0, SPI3_CLK_SEL.0, SPI3_CLK_SEL.1) == 0 {
                IN0_HZ
            } else {
                pll0_hz()
            };
            source / (2 * threshold(24))
        }
    }
}

/// Clock frequencies as they were found at boot.
///
/// Captured once and carried by value, so the rest of the crate never re-reads
/// SYSCTL on a hot path and a host build can construct one for testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clocks {
    pub cpu_hz: u32,
    pub apb0_hz: u32,
    pub apb1_hz: u32,
    pub apb2_hz: u32,
    /// SPI0, SPI1 and SPI3 in that order — reach these through
    /// [`Clocks::spi_hz`], which takes a controller number rather than an index.
    pub spi: [u32; 3],
}

impl Clocks {
    /// Read the live clock tree. Only meaningful on the chip.
    pub fn detect() -> Self {
        Self {
            cpu_hz: cpu_hz(),
            apb0_hz: apb_hz(0),
            apb1_hz: apb_hz(1),
            apb2_hz: apb_hz(2),
            spi: [spi_hz(0), spi_hz(1), spi_hz(3)],
        }
    }

    /// What a Maix board looks like coming out of the mask ROM: PLL0 at
    /// 806 MHz from the 26 MHz crystal, the core on PLL0/2, APB0 on ACLK/2.
    /// Used as the fallback when a detected core clock is obviously wrong, and
    /// as the reference the tests check the arithmetic against.
    pub const MAIX_DEFAULT: Clocks = Clocks {
        cpu_hz: 403_000_000,
        apb0_hz: 201_500_000,
        apb1_hz: 201_500_000,
        apb2_hz: 201_500_000,
        spi: [403_000_000, 403_000_000, 26_000_000],
    };

    /// The clock feeding SPI controller `bus`. Anything other than 0 or 1 means
    /// SPI3, which is the only other master.
    pub fn spi_hz(&self, bus: u8) -> u32 {
        match bus {
            0 => self.spi[0],
            1 => self.spi[1],
            _ => self.spi[2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MaixPy reports `Pll0:freq:806000000` and `cpu:freq` just under half of
    /// it; those are the register values that produce it.
    #[test]
    fn pll0_matches_the_frequency_maix_boards_report() {
        // clkr = 0, clkf = 61, clkod = 1 -> +1 each: 26 MHz / 1 * 62 / 2.
        assert_eq!(pll_freq(IN0_HZ, 1, 62, 2), 806_000_000);
    }

    /// The multiply overflows 32 bits before the divide, which is the whole
    /// reason `pll_freq` works in 64.
    #[test]
    fn pll_arithmetic_survives_the_widest_multiplier() {
        assert_eq!(pll_freq(IN0_HZ, 1, 64, 1), 1_664_000_000);
    }

    #[test]
    fn a_bypassed_pll_passes_the_crystal_through() {
        assert_eq!(pll_freq(IN0_HZ, 1, 1, 1), IN0_HZ);
    }

    /// The gate bit and the reset bit are the same bit in two registers — a
    /// coincidence in the silicon this crate leans on, so it is worth pinning.
    #[test]
    fn peripheral_bits_match_the_datasheet() {
        assert_eq!(Peripheral::Spi0 as u32, 6);
        assert_eq!(Peripheral::Spi3 as u32, 9);
        assert_eq!(Peripheral::Uart1 as u32, 16);
        assert_eq!(Peripheral::Fpioa as u32, 20);
    }
}
