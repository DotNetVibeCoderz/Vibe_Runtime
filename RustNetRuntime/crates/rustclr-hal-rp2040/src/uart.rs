//! UART — the RP2040's two PL011s.
//!
//! An ARM PL011, so the register map is the familiar one, with two RP2040
//! details on top: the peripheral is held in reset at power-up like everything
//! else, and it is clocked from `clk_peri`, which has its own enable. Miss
//! either and the UART configures cleanly, reads back plausibly, and transmits
//! nothing.
//!
//! The baud divisor is a 16.6 fixed-point number split across two registers,
//! and the rounding in [`divisors`] is the vendor's: the fractional part is
//! rounded rather than truncated, which is the difference between 115200 and
//! 115177 on a 125 MHz peripheral clock — inside every receiver's tolerance,
//! where truncation is not always.

use rustclr_hal::uart::{Uart, UartConfig};
use rustclr_hal::{HalError, HalResult};

use crate::{reg, resets};

const UARTDR: usize = 0x00;
const UARTFR: usize = 0x18;
const UARTIBRD: usize = 0x24;
const UARTFBRD: usize = 0x28;
const UARTLCR_H: usize = 0x2C;
const UARTCR: usize = 0x30;

/// `UARTFR`: transmit FIFO full, receive FIFO empty, busy.
const FR_TXFF: u32 = 1 << 5;
const FR_RXFE: u32 = 1 << 4;
const FR_BUSY: u32 = 1 << 3;

/// `UARTLCR_H`: word length 8, FIFOs enabled.
const LCR_H_WLEN_8: u32 = 3 << 5;
const LCR_H_FEN: u32 = 1 << 4;

/// `UARTCR`: enable, transmit enable, receive enable.
const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;

/// Bounded, like every wait in this crate: an unbounded spin on a peripheral
/// that never drains hangs the firmware before its service loop starts.
const SPIN_LIMIT: u32 = 1_000_000;

/// The integer and fractional halves of a PL011 baud divisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Divisors {
    pub integer: u32,
    pub fraction: u32,
}

/// Split `peri_hz / (16 * baud)` into the two registers the PL011 wants.
///
/// Computed in eighths and then rounded, which is what the vendor SDK does:
/// `8 * peri / baud` keeps three bits of fraction in integer arithmetic, the
/// top bits become IBRD, and the remainder rounds into FBRD's six.
///
/// A zero baud rate would divide by zero and a divisor of zero stops the
/// clock, so both are refused by the caller rather than clamped here — a UART
/// that silently runs at some other rate is worse than one that fails to
/// configure.
pub fn divisors(peri_hz: u32, baud: u32) -> Option<Divisors> {
    if baud == 0 || peri_hz == 0 {
        return None;
    }
    let eighths = (8 * peri_hz as u64) / baud as u64;
    let integer = (eighths >> 7) as u32;
    if integer == 0 {
        // Faster than the peripheral clock can divide down to.
        return None;
    }
    if integer >= 65_535 {
        // Slower than the 16-bit integer divisor can express. The PL011 caps
        // at 65535 with the fraction forced to zero.
        return Some(Divisors { integer: 65_535, fraction: 0 });
    }
    let fraction = (((eighths & 0x7F) + 1) / 2) as u32;
    Some(Divisors { integer, fraction })
}

#[derive(Clone, Copy)]
pub struct UartDef {
    pub base: usize,
    pub reset: u32,
}

pub const UART0: UartDef = UartDef {
    base: crate::base::UART0,
    reset: resets::UART0,
};
pub const UART1: UartDef = UartDef {
    base: crate::base::UART1,
    reset: resets::UART1,
};

pub struct Rp2040Uart {
    def: UartDef,
    peri_hz: u32,
    configured: bool,
}

impl Rp2040Uart {
    pub const fn new(def: UartDef, peri_hz: u32) -> Self {
        Self { def, peri_hz, configured: false }
    }

    pub fn set_peri_hz(&mut self, hz: u32) {
        self.peri_hz = hz;
    }

    #[inline]
    fn r(&self, offset: usize) -> u32 {
        reg::read(self.def.base + offset)
    }

    #[inline]
    fn w(&self, offset: usize, value: u32) {
        reg::write(self.def.base + offset, value);
    }

    /// Wait for the transmitter to finish before returning.
    ///
    /// A caller that resets or reconfigures immediately after writing would
    /// otherwise cut the last character in half — which on a console reads as
    /// a corrupted log line rather than as a driver ordering bug.
    pub fn flush(&mut self) {
        reg::wait_until(SPIN_LIMIT, || self.r(UARTFR) & FR_BUSY == 0);
    }
}

impl Uart for Rp2040Uart {
    fn configure(&mut self, config: UartConfig) -> HalResult<()> {
        let d = divisors(self.peri_hz, config.baud).ok_or(HalError::InvalidArgument(
            "baud rate is not reachable from clk_peri",
        ))?;

        // Out of reset first. A write to a held peripheral is discarded with
        // no fault, so configuring before this reads back as zeros.
        if !resets::unreset(self.def.reset) {
            return Err(HalError::Timeout);
        }

        // Disable while reprogramming: the PL011 latches the divisor when
        // UARTLCR_H is written, and only then.
        self.w(UARTCR, 0);
        self.w(UARTIBRD, d.integer);
        self.w(UARTFBRD, d.fraction);
        self.w(UARTLCR_H, LCR_H_WLEN_8 | LCR_H_FEN);
        self.w(UARTCR, CR_UARTEN | CR_TXE | CR_RXE);
        self.configured = true;
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> HalResult<usize> {
        if !self.configured {
            return Err(HalError::NotSupported);
        }
        for byte in data {
            if !reg::wait_until(SPIN_LIMIT, || self.r(UARTFR) & FR_TXFF == 0) {
                return Err(HalError::Timeout);
            }
            self.w(UARTDR, *byte as u32);
        }
        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> HalResult<usize> {
        if !self.configured {
            return Err(HalError::NotSupported);
        }
        let mut n = 0;
        while n < buf.len() && self.r(UARTFR) & FR_RXFE == 0 {
            buf[n] = (self.r(UARTDR) & 0xFF) as u8;
            n += 1;
        }
        Ok(n)
    }

    fn flush(&mut self) -> HalResult<()> {
        // Wait for the shift register, not just the FIFO. A caller that
        // resets or reconfigures right after writing would otherwise cut the
        // last character in half, which reads as a corrupted log line rather
        // than as an ordering bug.
        if reg::wait_until(SPIN_LIMIT, || self.r(UARTFR) & FR_BUSY == 0) {
            Ok(())
        } else {
            Err(HalError::Timeout)
        }
    }

    fn bytes_available(&mut self) -> HalResult<usize> {
        Ok(if self.r(UARTFR) & FR_RXFE == 0 { 1 } else { 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate everything in this repo talks at, on the clock this port runs
    /// at. Divisor 67.8125 — IBRD 67, FBRD 52 — which is 115177 baud, 0.02%
    /// off and far inside any receiver's tolerance.
    #[test]
    fn the_stock_115200_divisor_matches_the_vendor_sdk() {
        let d = divisors(125_000_000, 115_200).expect("reachable");
        assert_eq!(d.integer, 67);
        assert_eq!(d.fraction, 52);
    }

    /// The fraction is rounded, not truncated. Truncating here is a rate that
    /// is further off than it needs to be, and the symptom is occasional
    /// framing errors on long lines rather than anything obviously wrong.
    #[test]
    fn the_fraction_rounds_rather_than_truncates() {
        // 9600 on 125 MHz: 8*125e6/9600 = 104166.67, so IBRD 813 and a
        // remainder of 0x66 -> (0x66 + 1) / 2 = 51.
        let d = divisors(125_000_000, 9_600).expect("reachable");
        assert_eq!(d.integer, 813);
        assert_eq!(d.fraction, 51);
    }

    /// A rate the clock cannot divide down to is refused, not approximated. A
    /// UART that silently runs at some other rate is worse than one that fails
    /// to configure.
    #[test]
    fn an_unreachable_rate_is_refused() {
        assert_eq!(divisors(125_000_000, 0), None);
        assert_eq!(divisors(0, 115_200), None);
        // Faster than clk_peri / 16.
        assert_eq!(divisors(1_000_000, 10_000_000), None);
    }

    /// Very slow rates saturate the 16-bit integer divisor rather than
    /// wrapping — wrapping would silently produce a *fast* rate.
    #[test]
    fn a_very_slow_rate_saturates_rather_than_wraps() {
        let d = divisors(125_000_000, 100).expect("saturated");
        assert_eq!(d.integer, 65_535);
        assert_eq!(d.fraction, 0);
    }
}
