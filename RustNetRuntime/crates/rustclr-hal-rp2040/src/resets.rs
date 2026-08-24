//! The RESETS block: every peripheral starts held, and stays held until asked.
//!
//! This is the first thing that catches anyone coming to the RP2040 from
//! another Cortex-M part. There is no clock-enable register to forget — there
//! is a *reset* register, every peripheral is in it at power-up, and a write
//! to a held peripheral is discarded with no fault and no flag. A UART
//! configured before it is released reads back as zeros and transmits nothing,
//! which looks exactly like a wrong pin assignment.

use crate::base::RESETS;
use crate::reg;

const RESET: usize = RESETS + 0x00;
const RESET_DONE: usize = RESETS + 0x08;

/// Bits in `RESET`, from the datasheet (2.14.3). Only the ones this HAL
/// releases are named.
pub const IO_BANK0: u32 = 1 << 5;
pub const PADS_BANK0: u32 = 1 << 8;
pub const PLL_SYS: u32 = 1 << 12;
pub const PLL_USB: u32 = 1 << 13;
pub const TIMER: u32 = 1 << 21;
pub const UART0: u32 = 1 << 22;
pub const UART1: u32 = 1 << 23;

/// How long to wait for a peripheral to acknowledge coming out of reset.
/// Generous: this is a handful of clock cycles in practice, and the cost of
/// being wrong is a hang.
const SPIN_LIMIT: u32 = 1_000_000;

/// Release `mask` from reset and wait for the hardware to confirm it.
///
/// The acknowledge matters. Releasing reset and configuring immediately works
/// most of the time and fails when the peripheral has not caught up, which
/// produces a driver that is intermittently dead — the worst kind to chase.
pub fn unreset(mask: u32) -> bool {
    reg::clear_bits(RESET, mask);
    reg::wait_until(SPIN_LIMIT, || reg::read(RESET_DONE) & mask == mask)
}

/// Put `mask` back into reset.
pub fn hold(mask: u32) {
    reg::set_bits(RESET, mask);
}
