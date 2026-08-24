//! Over-the-air updates: two slots, and a rollback that does not need anyone
//! to be watching.
//!
//! Tooling streams a sealed [`rustclr_secureboot`] container into the
//! **inactive** slot, chunk by chunk. `finish` verifies it and marks the slot
//! pending; the device reboots into it; the new firmware calls `confirm_boot`
//! once it is satisfied it works. If the device reboots [`MAX_ATTEMPTS`] times
//! without ever confirming, `on_boot` puts the old slot back.
//!
//! # Why the update never lands on the running slot
//!
//! Because power fails halfway through writes. Writing over the firmware that
//! is currently executing turns any interruption into a board that will not
//! boot and needs a probe attached to recover. Two slots make a failed update
//! cost a reboot instead.
//!
//! # Why confirmation is the new firmware's job
//!
//! An image can verify perfectly and still be broken — a bad pin map, a
//! peripheral that never initialises, a network stack that cannot associate.
//! Only the new firmware can tell whether it actually works, so only the new
//! firmware may confirm itself. Silence is treated as failure, which is the
//! safe direction: a device that cannot report is a device that rolls back.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[allow(unused_imports)]
use alloc::{format, string::{String, ToString}, vec, vec::Vec};

use rustclr_secureboot::{verify, BootError, ChipFamily};

pub const MAX_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
    pub fn index(self) -> usize {
        match self {
            Slot::A => 0,
            Slot::B => 1,
        }
    }
}

/// Persistent state the bootloader reads. On MCUs this lives in a flash
/// sector; the trait lets firmware/host provide the medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtaState {
    pub active: Slot,
    /// Slot that was just flashed and awaits its first successful boot.
    pub pending: Option<Slot>,
    pub boot_attempts: u8,
}

impl Default for OtaState {
    fn default() -> Self {
        Self { active: Slot::A, pending: None, boot_attempts: 0 }
    }
}

pub trait SlotStorage {
    fn read(&self, slot: Slot) -> Vec<u8>;
    fn write(&mut self, slot: Slot, data: &[u8]);
    fn append(&mut self, slot: Slot, chunk: &[u8]);
    fn state(&self) -> OtaState;
    fn set_state(&mut self, state: OtaState);
}

/// In-memory storage (host firmware + tests).
#[derive(Default)]
pub struct MemSlots {
    slots: [Vec<u8>; 2],
    state: OtaState,
}

impl MemSlots {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SlotStorage for MemSlots {
    fn read(&self, slot: Slot) -> Vec<u8> {
        self.slots[slot.index()].clone()
    }
    fn write(&mut self, slot: Slot, data: &[u8]) {
        self.slots[slot.index()] = data.to_vec();
    }
    fn append(&mut self, slot: Slot, chunk: &[u8]) {
        self.slots[slot.index()].extend_from_slice(chunk);
    }
    fn state(&self) -> OtaState {
        self.state
    }
    fn set_state(&mut self, state: OtaState) {
        self.state = state;
    }
}

#[derive(Debug, PartialEq)]
pub enum OtaError {
    UpdateInProgressRequired,
    Verify(BootError),
    NothingToRollback,
}

pub struct OtaManager<S: SlotStorage> {
    pub storage: S,
    key: Vec<u8>,
    chip: ChipFamily,
    receiving: bool,
}

impl<S: SlotStorage> OtaManager<S> {
    pub fn new(storage: S, key: Vec<u8>, chip: ChipFamily) -> Self {
        Self { storage, key, chip, receiving: false }
    }

    pub fn target_slot(&self) -> Slot {
        self.storage.state().active.other()
    }

    /// Start receiving an update into the inactive slot.
    pub fn begin(&mut self) {
        let target = self.target_slot();
        self.storage.write(target, &[]);
        self.receiving = true;
    }

    pub fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), OtaError> {
        if !self.receiving {
            return Err(OtaError::UpdateInProgressRequired);
        }
        self.storage.append(self.target_slot(), chunk);
        Ok(())
    }

    /// Verify the received image and mark the slot pending activation.
    pub fn finish(&mut self) -> Result<(), OtaError> {
        if !self.receiving {
            return Err(OtaError::UpdateInProgressRequired);
        }
        self.receiving = false;
        let target = self.target_slot();
        let image = self.storage.read(target);
        verify(&image, &self.key, self.chip).map_err(OtaError::Verify)?;
        let mut st = self.storage.state();
        st.pending = Some(target);
        st.boot_attempts = 0;
        self.storage.set_state(st);
        Ok(())
    }

    /// Bootloader entry: decides which slot to boot and handles rollback.
    /// Returns the slot to boot from.
    pub fn on_boot(&mut self) -> Slot {
        let mut st = self.storage.state();
        if let Some(pending) = st.pending {
            if st.boot_attempts >= MAX_ATTEMPTS {
                // New image never confirmed: roll back.
                st.pending = None;
                st.boot_attempts = 0;
                self.storage.set_state(st);
                return st.active;
            }
            st.boot_attempts += 1;
            self.storage.set_state(st);
            return pending;
        }
        st.active
    }

    /// Called by the new firmware once it considers itself healthy.
    pub fn confirm_boot(&mut self) {
        let mut st = self.storage.state();
        if let Some(pending) = st.pending.take() {
            st.active = pending;
            st.boot_attempts = 0;
            self.storage.set_state(st);
        }
    }

    /// Explicit rollback (user-requested).
    pub fn rollback(&mut self) -> Result<Slot, OtaError> {
        let mut st = self.storage.state();
        if st.pending.is_some() {
            st.pending = None;
            st.boot_attempts = 0;
            self.storage.set_state(st);
            return Ok(st.active);
        }
        Err(OtaError::NothingToRollback)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_secureboot::{seal, ImageKind};

    const KEY: &[u8] = b"a provisioned device key, 32 by!";

    fn manager() -> OtaManager<MemSlots> {
        OtaManager::new(MemSlots::new(), KEY.to_vec(), ChipFamily::Any)
    }

    fn stream(m: &mut OtaManager<MemSlots>, payload: &[u8]) -> Result<(), OtaError> {
        let image = seal(KEY, ImageKind::Firmware, ChipFamily::Any, payload);
        m.begin();
        // In chunks, because that is how it arrives over a network.
        for chunk in image.chunks(7) {
            m.write_chunk(chunk)?;
        }
        m.finish()
    }

    #[test]
    fn an_update_lands_in_the_other_slot_and_boots_after_confirmation() {
        let mut m = manager();
        assert_eq!(m.target_slot(), Slot::B, "A is running, so B is the target");
        stream(&mut m, b"new firmware").expect("a sealed image is accepted");

        // The device reboots into the pending slot, and the new firmware says
        // it works.
        assert_eq!(m.on_boot(), Slot::B);
        m.confirm_boot();
        assert_eq!(m.on_boot(), Slot::B, "confirmed, so B is now the active slot");
        assert_eq!(m.target_slot(), Slot::A, "and the next update goes to A");
    }

    /// The whole point of the design: an image that verifies but does not work
    /// must not strand the device. Nobody is there to press a button.
    #[test]
    fn firmware_that_never_confirms_is_rolled_back() {
        let mut m = manager();
        stream(&mut m, b"a broken build").expect("it verifies — it just does not work");

        for attempt in 1..=MAX_ATTEMPTS {
            assert_eq!(m.on_boot(), Slot::B, "attempt {attempt} tries the new image");
        }
        assert_eq!(m.on_boot(), Slot::A, "after {MAX_ATTEMPTS} silent boots, the old slot is back");
        assert_eq!(m.on_boot(), Slot::A, "and it stays back");
    }

    /// A confirmation on the last allowed attempt still counts. Off by one
    /// here means a working image gets rolled back.
    #[test]
    fn confirming_on_the_last_attempt_is_in_time() {
        let mut m = manager();
        stream(&mut m, b"slow to start").unwrap();
        for _ in 1..MAX_ATTEMPTS {
            assert_eq!(m.on_boot(), Slot::B);
        }
        assert_eq!(m.on_boot(), Slot::B);
        m.confirm_boot();
        assert_eq!(m.on_boot(), Slot::B);
    }

    #[test]
    fn an_image_that_does_not_verify_is_never_made_pending() {
        let mut m = manager();
        let mut image = seal(KEY, ImageKind::Firmware, ChipFamily::Any, b"tampered");
        let last = image.len() - 1;
        image[last] ^= 0xFF;

        m.begin();
        m.write_chunk(&image).unwrap();
        assert!(matches!(m.finish(), Err(OtaError::Verify(_))));
        // The running slot is untouched: a rejected update is a no-op, not a
        // half-applied one.
        assert_eq!(m.on_boot(), Slot::A);
    }

    #[test]
    fn an_image_signed_with_another_key_is_refused() {
        let mut m = manager();
        let image = seal(b"some other key, also 32 bytes!!!", ImageKind::Firmware, ChipFamily::Any, b"x");
        m.begin();
        m.write_chunk(&image).unwrap();
        assert!(m.finish().is_err());
    }

    #[test]
    fn chunks_have_to_be_preceded_by_begin() {
        let mut m = manager();
        assert!(m.write_chunk(b"stray bytes").is_err());
    }

    #[test]
    fn an_explicit_rollback_discards_a_pending_image() {
        let mut m = manager();
        stream(&mut m, b"never mind").unwrap();
        assert_eq!(m.rollback().expect("rolls back"), Slot::A);
        assert_eq!(m.on_boot(), Slot::A);
    }
}
