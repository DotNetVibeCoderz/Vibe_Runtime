//! Raw register access.
//!
//! Every peripheral in this crate is driven by absolute address, with no PAC and
//! no ownership tokens — the same choice `rustclr-hal-stm32` makes, and for the
//! same reason: the firmware needs to poke a handful of these from a panic
//! handler, where no peripheral value is in scope.
//!
//! **Off-chip these are no-ops.** The crate compiles for the host so it can be
//! unit-tested alongside the rest of the workspace, and a test that exercises
//! pad allocation or a baud divisor would otherwise dereference a K210
//! peripheral address on x86 and take the test runner down with it. Writes are
//! dropped and reads answer zero, which leaves every pure calculation testable
//! and makes anything that genuinely depends on a device's reply — a flash
//! status poll, a FIFO level — report a timeout instead of lying.

#[cfg(target_arch = "riscv64")]
#[inline(always)]
pub fn write(addr: usize, value: u32) {
    // SAFETY: fixed peripheral addresses from the K210 datasheet; only
    // meaningful when executing on the chip itself.
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
pub fn read(addr: usize) -> u32 {
    // SAFETY: see `write`.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
pub fn write(_addr: usize, _value: u32) {}

#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
pub fn read(_addr: usize) -> u32 {
    0
}

/// Clear the bits in `clear`, then set the bits in `set`.
#[inline(always)]
pub fn modify(addr: usize, clear: u32, set: u32) {
    write(addr, (read(addr) & !clear) | set);
}

/// Replace the `width`-bit field at `shift`.
#[inline(always)]
pub fn set_field(addr: usize, shift: u32, width: u32, value: u32) {
    let mask = ((1u32 << width) - 1) << shift;
    modify(addr, mask, (value << shift) & mask);
}

/// Read the `width`-bit field at `shift`.
#[inline(always)]
pub fn field(addr: usize, shift: u32, width: u32) -> u32 {
    (read(addr) >> shift) & ((1u32 << width) - 1)
}
