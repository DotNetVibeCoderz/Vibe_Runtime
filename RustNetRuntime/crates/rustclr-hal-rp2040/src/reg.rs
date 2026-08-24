//! Raw register access, and the RP2040's atomic aliases.

/// Read a 32-bit peripheral register.
///
/// # Safety
/// `addr` must be a valid peripheral register address. Every caller in this
/// crate takes it from [`crate::base`] plus a datasheet offset.
#[inline(always)]
pub fn read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline(always)]
pub fn write(addr: usize, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

/// Offsets of the register's atomic aliases.
///
/// Every peripheral register on this chip is mirrored three times above
/// itself: writing to `+0x1000` XORs, `+0x2000` sets, `+0x3000` clears. The
/// hardware does it in one bus transaction.
const ALIAS_XOR: usize = 0x1000;
const ALIAS_SET: usize = 0x2000;
const ALIAS_CLR: usize = 0x3000;

/// Set the given bits, atomically.
///
/// Preferred over read-modify-write everywhere in this crate. Not for speed —
/// an RMW on a register that an interrupt or the other core also touches
/// silently loses whichever update landed in between, and this part has two
/// cores.
#[inline(always)]
pub fn set_bits(addr: usize, bits: u32) {
    write(addr + ALIAS_SET, bits);
}

#[inline(always)]
pub fn clear_bits(addr: usize, bits: u32) {
    write(addr + ALIAS_CLR, bits);
}

#[inline(always)]
pub fn toggle_bits(addr: usize, bits: u32) {
    write(addr + ALIAS_XOR, bits);
}

/// Replace the bits in `mask` with `value`, atomically in two writes.
///
/// Two writes rather than one because there is no atomic "write these bits":
/// the clear and the set are each atomic, and between them the field reads as
/// zero. That is fine for a configuration field and wrong for a field the
/// hardware acts on continuously, which is why the drivers here configure
/// peripherals while they are held in reset.
#[inline(always)]
pub fn replace_bits(addr: usize, mask: u32, value: u32) {
    clear_bits(addr, mask);
    set_bits(addr, value & mask);
}

/// Spin until `predicate` holds, or give up.
///
/// Bounded like every wait in this crate's siblings: an unbounded spin on a
/// peripheral that never comes ready hangs the firmware before its service
/// loop starts, and that reads as a board which will not enumerate rather than
/// as a driver bug.
pub fn wait_until(limit: u32, mut predicate: impl FnMut() -> bool) -> bool {
    for _ in 0..limit {
        if predicate() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}
