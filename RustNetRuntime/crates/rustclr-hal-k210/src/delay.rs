//! Delay and monotonic clock, off the core's own cycle counter.
//!
//! `mcycle` is 64 bits wide on RV64 and reads in a single instruction, which
//! makes this markedly simpler than the Cortex-M port: no 32-bit wrap to track,
//! no peripheral to ungate, no trace unit to enable. At 400 MHz the counter
//! takes 1400 years to overflow, so `now_us` is monotonic for every practical
//! purpose.
//!
//! `mcycle` rather than the user-mode `cycle` alias: the firmware runs in
//! machine mode, where `mcycle` is unconditionally readable, whereas `cycle`
//! needs `mcounteren` to have been opened up first.
//!
//! The CLINT's `mtime` would also serve, but it ticks at a divided rate the
//! datasheet does not pin down as clearly as the core clock, and this counter
//! is both finer and cheaper to read.

use rustclr_hal::delay::Delay;

/// Read the cycle counter.
#[cfg(target_arch = "riscv64")]
#[inline(always)]
pub fn cycles() -> u64 {
    let value: u64;
    // SAFETY: reading a CSR has no side effects. `mcycle` is always readable
    // in machine mode, which is where this firmware runs.
    unsafe { core::arch::asm!("csrr {}, mcycle", out(reg) value, options(nomem, nostack)) };
    value
}

/// Host stand-in, so the crate compiles and unit-tests off-chip. Advances on
/// every read, which is enough to keep a spin loop from being infinite if one
/// is ever reached in a test.
#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
pub fn cycles() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static FAKE: AtomicU64 = AtomicU64::new(0);
    FAKE.fetch_add(1_000, Ordering::Relaxed)
}

pub struct CycleDelay {
    cpu_hz: u32,
}

impl CycleDelay {
    pub fn new(cpu_hz: u32) -> Self {
        Self { cpu_hz }
    }

    pub fn set_cpu_hz(&mut self, cpu_hz: u32) {
        self.cpu_hz = cpu_hz;
    }

    /// Cycles per microsecond, clamped to at least one so a nonsense clock
    /// reading turns a delay into a very short wait rather than a division by
    /// zero.
    #[inline]
    pub fn cycles_per_us(&self) -> u64 {
        (self.cpu_hz as u64 / 1_000_000).max(1)
    }
}

impl Delay for CycleDelay {
    fn delay_us(&mut self, us: u64) {
        let target = cycles().wrapping_add(us.saturating_mul(self.cycles_per_us()));
        while cycles() < target {
            core::hint::spin_loop();
        }
    }

    fn now_us(&self) -> u64 {
        cycles() / self.cycles_per_us()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_per_us_tracks_the_core_clock() {
        assert_eq!(CycleDelay::new(403_000_000).cycles_per_us(), 403);
        assert_eq!(CycleDelay::new(26_000_000).cycles_per_us(), 26);
    }

    /// A clock reading below 1 MHz would otherwise divide by zero in `now_us`.
    #[test]
    fn an_impossible_clock_still_yields_a_usable_divisor() {
        assert_eq!(CycleDelay::new(0).cycles_per_us(), 1);
    }
}
