//! A map keyed by metadata row number.
//!
//! Rows are small dense integers starting at 1, so a vector indexed by row is
//! both smaller than a hash map and constant-time to read. That matters
//! because the interpreter resolves a token on every call, field access and
//! type test: with `std` those lookups hashed with SipHash, and without it
//! `HashMap` is a `BTreeMap`, so each was a tree walk.

#[allow(unused_imports)]
use crate::prelude::*;

#[derive(Debug, Clone)]
pub struct RowMap<T> {
    slots: Vec<Option<T>>,
    len: usize,
}

impl<T> Default for RowMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> RowMap<T> {
    pub const fn new() -> Self {
        Self { slots: Vec::new(), len: 0 }
    }

    #[inline]
    pub fn get(&self, row: &u32) -> Option<&T> {
        self.slots.get(*row as usize).and_then(Option::as_ref)
    }

    #[inline]
    pub fn get_mut(&mut self, row: &u32) -> Option<&mut T> {
        self.slots.get_mut(*row as usize).and_then(Option::as_mut)
    }

    #[inline]
    pub fn contains_key(&self, row: &u32) -> bool {
        self.get(row).is_some()
    }

    pub fn insert(&mut self, row: u32, value: T) -> Option<T> {
        let i = row as usize;
        if i >= self.slots.len() {
            self.slots.resize_with(i + 1, || None);
        }
        let old = self.slots[i].replace(value);
        if old.is_none() {
            self.len += 1;
        }
        old
    }

    pub fn remove(&mut self, row: &u32) -> Option<T> {
        let old = self.slots.get_mut(*row as usize).and_then(Option::take);
        if old.is_some() {
            self.len -= 1;
        }
        old
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.slots.iter().filter_map(Option::as_ref)
    }

    /// Rows and values in row order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, v)| v.as_ref().map(|v| (i as u32, v)))
    }

    pub fn keys(&self) -> impl Iterator<Item = u32> + '_ {
        self.iter().map(|(k, _)| k)
    }
}

impl<T> core::ops::Index<&u32> for RowMap<T> {
    type Output = T;

    fn index(&self, row: &u32) -> &T {
        self.get(row).expect("no entry for row")
    }
}
