//! Static-field storage, shared between the threads running managed code.
//!
//! `static int Total;` is one slot however many threads reach it — that is
//! what `static` means in C#, and it is the one piece of a [`Loader`] that two
//! threads cannot each have their own copy of.
//!
//! Everything else in a loader is settled before anything runs: the type
//! registry, the assemblies, the resolved tokens. That is what makes the
//! design work at all — a clone of a loader taken after loading is *identical*
//! to the original, so two threads reading their own copies behave exactly as
//! if they shared one, with no lock on the path that runs every instruction.
//! Only the mutable part needs to be genuinely shared, and this is it.
//!
//! [`Loader`]: crate::loader::Loader

use crate::prelude::*;
use crate::value::Value;

#[cfg(feature = "std")]
use std::sync::{Arc, Mutex, MutexGuard};

#[cfg(not(feature = "std"))]
use alloc::rc::Rc;
#[cfg(not(feature = "std"))]
use core::cell::{RefCell, RefMut};

/// Static-field values, indexed by `FieldId`.
#[derive(Clone, Default)]
pub struct SharedStatics {
    #[cfg(feature = "std")]
    inner: Arc<Mutex<Vec<Value>>>,
    #[cfg(not(feature = "std"))]
    inner: Rc<RefCell<Vec<Value>>>,
}

impl SharedStatics {
    /// Locks the storage.
    ///
    /// Deliberately short-lived at every call site: a static read copies its
    /// value out and a write puts one in, so the guard never spans managed
    /// code. Holding one across a call would deadlock the moment that call
    /// touched another static.
    #[cfg(feature = "std")]
    pub fn lock(&self) -> MutexGuard<'_, Vec<Value>> {
        // A poisoned lock means a thread panicked mid-write. There is no
        // sensible half-updated static, so this propagates.
        self.inner.lock().expect("static storage is poisoned")
    }

    #[cfg(not(feature = "std"))]
    pub fn lock(&self) -> RefMut<'_, Vec<Value>> {
        self.inner.borrow_mut()
    }
}
