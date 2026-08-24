//! Crystal, PLLs, and the clock tree.
//!
//! The RP2040 wakes on a ring oscillator at roughly 6 MHz — "roughly" because
//! it is an on-die RC and its rate moves with temperature and voltage. Nothing
//! that depends on a known frequency is true until the crystal and the PLLs
//! are up, so a UART configured before [`init`] runs at whatever the ring
//! oscillator happened to be doing.
//!
//! The order below is the one the datasheet requires and it is not
//! rearrangeable: the crystal has to be stable before a PLL locks to it, and a
//! PLL has to be locked before the clock tree is switched onto it. Switching
//! `clk_sys` to a PLL that has not locked stops the processor.

use crate::base::{CLOCKS, PLL_SYS, PLL_USB, XOSC};
use crate::{reg, resets};

// --- XOSC -----------------------------------------------------------------
const XOSC_CTRL: usize = XOSC + 0x00;
const XOSC_STATUS: usize = XOSC + 0x04;
const XOSC_STARTUP: usize = XOSC + 0x0C;

/// 1..15 MHz range, the only one this chip has.
const XOSC_CTRL_FREQ_RANGE_1_15MHZ: u32 = 0xAA0;
/// Magic enable value. A plain 1 does nothing — the field is a 12-bit code.
const XOSC_CTRL_ENABLE: u32 = 0xFAB << 12;
const XOSC_STATUS_STABLE: u32 = 1 << 31;

// --- PLL ------------------------------------------------------------------
const PLL_CS: usize = 0x00;
const PLL_PWR: usize = 0x04;
const PLL_FBDIV_INT: usize = 0x08;
const PLL_PRIM: usize = 0x0C;

const PLL_CS_LOCK: u32 = 1 << 31;
const PLL_PWR_PD: u32 = 1 << 0;
const PLL_PWR_VCOPD: u32 = 1 << 5;
const PLL_PWR_POSTDIVPD: u32 = 1 << 3;

// --- clock tree -----------------------------------------------------------
const CLK_REF_CTRL: usize = CLOCKS + 0x30;
const CLK_REF_SELECTED: usize = CLOCKS + 0x38;
const CLK_SYS_CTRL: usize = CLOCKS + 0x3C;
const CLK_SYS_SELECTED: usize = CLOCKS + 0x44;
const CLK_PERI_CTRL: usize = CLOCKS + 0x48;
const CLK_USB_CTRL: usize = CLOCKS + 0x50;

const CLK_SYS_CTRL_SRC_AUX: u32 = 1;
const CLK_SYS_CTRL_AUXSRC_PLL_SYS: u32 = 0 << 5;
const CLK_REF_CTRL_SRC_XOSC: u32 = 2;
const CLK_PERI_CTRL_ENABLE: u32 = 1 << 11;
const CLK_PERI_CTRL_AUXSRC_CLK_SYS: u32 = 0 << 5;
const CLK_USB_CTRL_ENABLE: u32 = 1 << 11;
const CLK_USB_CTRL_AUXSRC_PLL_USB: u32 = 0 << 5;

/// The USB controller runs from its own 48 MHz clock and nothing else will
/// do — the standard is 12 Mbit/s full speed derived from exactly that.
pub const USB_HZ: u32 = 48_000_000;

const SPIN_LIMIT: u32 = 10_000_000;

/// The crystal on every Pico-family board.
pub const XOSC_HZ: u32 = 12_000_000;

/// What the clock tree ended up at. Passed to whatever needs a rate — a UART
/// divisor is wrong by exactly the amount this is wrong by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    pub xosc_hz: u32,
    pub sys_hz: u32,
    /// `clk_peri` feeds the UARTs and SPIs. Driven from `clk_sys` here.
    pub peri_hz: u32,
    /// `clk_usb`, which must be 48 MHz or the device controller does nothing
    /// at all. Zero when the USB PLL did not come up.
    pub usb_hz: u32,
}

/// A PLL setting: `vco = ref / refdiv * fbdiv`, output `vco / (pd1 * pd2)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PllConfig {
    pub refdiv: u32,
    pub fbdiv: u32,
    pub post_div1: u32,
    pub post_div2: u32,
}

impl PllConfig {
    pub fn vco_hz(&self, ref_hz: u32) -> u64 {
        ref_hz as u64 / self.refdiv as u64 * self.fbdiv as u64
    }

    pub fn output_hz(&self, ref_hz: u32) -> u64 {
        self.vco_hz(ref_hz) / (self.post_div1 as u64 * self.post_div2 as u64)
    }
}

/// The PLL's hard limits, from the datasheet (2.18.2).
const VCO_MIN_HZ: u64 = 750_000_000;
const VCO_MAX_HZ: u64 = 1_600_000_000;
const FBDIV_MIN: u32 = 16;
const FBDIV_MAX: u32 = 320;

/// Find a PLL setting that hits `target_hz` exactly, or `None`.
///
/// Exactly, not approximately. A UART divisor derived from a system clock that
/// is "close" is a baud rate that is close, and the failure shows up as
/// occasional framing errors on long lines rather than as anything obviously
/// wrong — far more expensive to chase than a refused configuration at boot.
///
/// The search prefers the highest VCO, which is what the vendor SDK does: a
/// higher VCO with a larger post-divider has less jitter than a low VCO with a
/// small one.
pub const fn pll_config(ref_hz: u32, target_hz: u32) -> Option<PllConfig> {
    // `refdiv` stays 1: every board in this family has a 12 MHz crystal, and
    // dividing it further only shrinks the set of reachable frequencies.
    let refdiv = 1;
    let mut fbdiv = FBDIV_MAX;
    while fbdiv >= FBDIV_MIN {
        let vco = ref_hz as u64 / refdiv as u64 * fbdiv as u64;
        if vco < VCO_MIN_HZ || vco > VCO_MAX_HZ {
            fbdiv -= 1;
            continue;
        }
        let mut pd1 = 7;
        while pd1 >= 1 {
            let mut pd2 = pd1;
            while pd2 >= 1 {
                // post_div1 >= post_div2 is the datasheet's rule, and the loop
                // shape above enforces it rather than checking for it.
                if vco == target_hz as u64 * pd1 as u64 * pd2 as u64 {
                    return Some(PllConfig {
                        refdiv,
                        fbdiv,
                        post_div1: pd1,
                        post_div2: pd2,
                    });
                }
                pd2 -= 1;
            }
            pd1 -= 1;
        }
        fbdiv -= 1;
    }
    None
}

/// Bring up the crystal, lock the system PLL, and run `clk_sys` from it.
///
/// Returns what the tree is actually running at. A caller that ignores this
/// and assumes its target got a UART at the wrong baud rate.
pub fn init(target_sys_hz: u32) -> Clocks {
    let config = match pll_config(XOSC_HZ, target_sys_hz) {
        Some(c) => c,
        // Unreachable target: stay on the ring oscillator rather than switch
        // clk_sys to a PLL that will never lock, which stops the processor.
        None => return ring_oscillator_only(),
    };

    // Each step is checked, and a failure stops the sequence rather than
    // carrying on into the next one. Switching `clk_sys` onto a PLL that has
    // not locked stops the processor outright — no fault, no fallback, and
    // nothing left running to say so.
    if !start_xosc() {
        return ring_oscillator_only();
    }

    // clk_ref onto the crystal first: the PLLs measure against it, and the
    // watchdog tick and clk_sys's fallback both come from here.
    reg::write(CLK_REF_CTRL, CLK_REF_CTRL_SRC_XOSC);
    if !reg::wait_until(SPIN_LIMIT, || reg::read(CLK_REF_SELECTED) & (1 << 2) != 0) {
        return ring_oscillator_only();
    }

    if !resets::unreset(resets::PLL_SYS | resets::PLL_USB) {
        return ring_oscillator_only();
    }
    if !start_pll(PLL_SYS, config) {
        return ring_oscillator_only();
    }

    // Only now is it safe to switch.
    reg::write(CLK_SYS_CTRL, CLK_SYS_CTRL_AUXSRC_PLL_SYS | CLK_SYS_CTRL_SRC_AUX);
    if !reg::wait_until(SPIN_LIMIT, || reg::read(CLK_SYS_SELECTED) & (1 << 1) != 0) {
        return ring_oscillator_only();
    }

    // The peripheral clock is a separate enable, and a UART on a disabled
    // clk_peri is a UART that configures cleanly and transmits nothing.
    reg::write(
        CLK_PERI_CTRL,
        CLK_PERI_CTRL_ENABLE | CLK_PERI_CTRL_AUXSRC_CLK_SYS,
    );

    // The USB controller has its own PLL and its own 48 MHz requirement, and
    // it is entirely separate from the system clock. Releasing PLL_USB from
    // reset without configuring it — which this port did at first — leaves
    // the controller unclocked, and an unclocked controller presents no
    // pull-up: the host sees nothing plugged in at all, which looks like a
    // dead cable rather than a missing clock.
    let usb_hz = match pll_config(XOSC_HZ, USB_HZ) {
        Some(usb) if start_pll(PLL_USB, usb) => {
            reg::write(
                CLK_USB_CTRL,
                CLK_USB_CTRL_ENABLE | CLK_USB_CTRL_AUXSRC_PLL_USB,
            );
            USB_HZ
        }
        // Reported as zero rather than assumed: the firmware checks this
        // before starting the device controller.
        _ => 0,
    };

    let sys_hz = config.output_hz(XOSC_HZ) as u32;
    Clocks {
        xosc_hz: XOSC_HZ,
        sys_hz,
        peri_hz: sys_hz,
        usb_hz,
    }
}

/// What [`init`] returns when it gives up: still running, still slow, and
/// saying so. `sys_hz` of zero is the caller's signal that no rate is known —
/// a UART configured from it will refuse rather than pick a wrong divisor.
fn ring_oscillator_only() -> Clocks {
    Clocks {
        xosc_hz: XOSC_HZ,
        sys_hz: 0,
        peri_hz: 0,
        usb_hz: 0,
    }
}

fn start_xosc() -> bool {
    // The startup delay is counted in multiples of 256 crystal cycles. One
    // millisecond at 12 MHz is 12000 cycles, so 47 blocks — the SDK rounds the
    // same way, and a crystal given too little time reads stable before it is.
    let startup_delay = (XOSC_HZ / 1000 + 128) / 256;
    reg::write(XOSC_STARTUP, startup_delay);
    reg::write(XOSC_CTRL, XOSC_CTRL_ENABLE | XOSC_CTRL_FREQ_RANGE_1_15MHZ);
    reg::wait_until(SPIN_LIMIT, || {
        reg::read(XOSC_STATUS) & XOSC_STATUS_STABLE != 0
    })
}

fn start_pll(base: usize, config: PllConfig) -> bool {
    // Power everything down, program, then power up: the dividers are latched
    // out of reset and changing them under a running VCO is undefined.
    reg::write(base + PLL_PWR, 0xFFFF_FFFF);
    reg::write(base + PLL_FBDIV_INT, config.fbdiv);
    reg::write(base + PLL_CS, config.refdiv);

    reg::clear_bits(base + PLL_PWR, PLL_PWR_PD | PLL_PWR_VCOPD);
    if !reg::wait_until(SPIN_LIMIT, || reg::read(base + PLL_CS) & PLL_CS_LOCK != 0) {
        return false;
    }

    reg::write(
        base + PLL_PRIM,
        (config.post_div1 << 16) | (config.post_div2 << 12),
    );
    reg::clear_bits(base + PLL_PWR, PLL_PWR_POSTDIVPD);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The setting every Pico runs at, and the one the vendor SDK picks:
    /// 12 MHz reference, VCO 1500 MHz, divided by 6 and 2.
    #[test]
    fn the_stock_125mhz_setting_is_found() {
        let c = pll_config(XOSC_HZ, 125_000_000).expect("125 MHz");
        assert_eq!(c.vco_hz(XOSC_HZ), 1_500_000_000);
        assert_eq!(c.output_hz(XOSC_HZ), 125_000_000);
        assert!(c.post_div1 >= c.post_div2, "datasheet requires pd1 >= pd2");
    }

    /// Whatever the search returns must be exactly the target and inside the
    /// VCO's range — a PLL asked to run outside it does not lock, and clk_sys
    /// switched onto an unlocked PLL stops the processor.
    #[test]
    fn every_found_setting_is_exact_and_in_range() {
        for target in [48_000_000u32, 100_000_000, 125_000_000, 133_000_000] {
            let c = pll_config(XOSC_HZ, target).expect("reachable");
            assert_eq!(c.output_hz(XOSC_HZ), target as u64, "{target} Hz");
            let vco = c.vco_hz(XOSC_HZ);
            assert!(
                (VCO_MIN_HZ..=VCO_MAX_HZ).contains(&vco),
                "{target} Hz gave a VCO of {vco}"
            );
            assert!((FBDIV_MIN..=FBDIV_MAX).contains(&c.fbdiv));
            assert!((1..=7).contains(&c.post_div1));
            assert!((1..=7).contains(&c.post_div2));
        }
    }

    /// The USB controller needs exactly 48 MHz, and the PLL reaches it from a
    /// 12 MHz crystal with a 1200 MHz VCO divided by five twice. Getting this
    /// wrong is a device the host never sees.
    #[test]
    fn the_usb_clock_is_reachable_and_exact() {
        let c = pll_config(XOSC_HZ, USB_HZ).expect("48 MHz");
        assert_eq!(c.output_hz(XOSC_HZ), 48_000_000);
        let vco = c.vco_hz(XOSC_HZ);
        assert!((VCO_MIN_HZ..=VCO_MAX_HZ).contains(&vco), "VCO {vco}");
    }

    /// A target the PLL cannot hit exactly is refused rather than approximated.
    /// An approximate system clock is an approximate baud rate, and that shows
    /// up as occasional framing errors rather than as anything obviously wrong.
    #[test]
    fn an_unreachable_target_is_refused() {
        // 7 MHz needs a VCO below the minimum at every post-divider pair.
        assert_eq!(pll_config(XOSC_HZ, 7_000_000), None);
        assert_eq!(pll_config(XOSC_HZ, 0), None);
    }
}
