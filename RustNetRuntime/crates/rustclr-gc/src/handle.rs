//! Object handles.
//!
//! RustCLR addresses heap objects through a handle table rather than raw
//! pointers. A handle is an index plus a generation counter, so a stale handle
//! is *detected* instead of dereferencing freed memory — the class of bug that
//! motivated replacing the C++ runtime in the first place.
//!
//! The indirection costs one array load per access. In exchange the collector
//! can move, compact, or reallocate objects without rewriting interior
//! pointers, and no `unsafe` appears anywhere in the object graph.

use core::fmt;
use core::num::NonZeroU32;

/// A reference to a heap object, or null.
///
/// Layout is a packed `u64`: 32 bits of slot index and 32 bits of generation.
/// `Handle::NULL` is the all-zero value, so `Default` gives null for free.
///
/// `repr(transparent)` is load-bearing, not decoration. Compiled code holds
/// handles — in a register, in an array descriptor — and reads them back as
/// 64-bit integers, so this type's representation is part of the code
/// generator's ABI. `transparent` is the language's own promise that a
/// single-field wrapper *is* its field, which is why a handle can cross that
/// boundary when a [`Value`](../../rustclr_core/enum.Value.html) cannot: a
/// handle is one `u64` with no discriminant and no payload to reorder.
///
/// It stays safe as well as stable because a handle is an opaque token rather
/// than an address. Compiled code cannot dereference one, so the worst a wrong
/// value can do is fail a generation check and be reported.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    pub const NULL: Handle = Handle(0);

    #[inline]
    pub(crate) fn new(index: u32, generation: NonZeroU32) -> Self {
        // index is stored +1 so that slot 0 with generation 1 is not null.
        Handle(((index as u64 + 1) << 32) | generation.get() as u64)
    }

    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub(crate) const fn index(self) -> usize {
        ((self.0 >> 32) as usize).wrapping_sub(1)
    }

    #[inline]
    pub(crate) const fn generation(self) -> u32 {
        self.0 as u32
    }

    /// The opaque bit pattern, for embedding in an interpreter stack slot.
    #[inline]
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    /// Reconstructs a handle from [`Handle::to_bits`].
    ///
    /// Safe: a bogus value fails validation on the next heap access rather than
    /// producing undefined behaviour.
    #[inline]
    pub const fn from_bits(bits: u64) -> Self {
        Handle(bits)
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            write!(f, "null")
        } else {
            write!(f, "obj#{}g{}", self.index(), self.generation())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_distinguishable_from_slot_zero() {
        let first = NonZeroU32::new(1).unwrap();
        let h = Handle::new(0, first);
        assert!(!h.is_null());
        assert_eq!(h.index(), 0);
        assert_eq!(h.generation(), 1);
        assert!(Handle::NULL.is_null());
    }

    #[test]
    fn bits_round_trip() {
        let h = Handle::new(1234, NonZeroU32::new(7).unwrap());
        assert_eq!(Handle::from_bits(h.to_bits()), h);
    }
}
