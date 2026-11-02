//! `Instant` over the embedder's monotonic clock; `Duration` is core's.

pub use core::time::Duration;

use core::ops::{Add, AddAssign, Sub, SubAssign};

/// A point on the monotonic clock (nanoseconds since an arbitrary origin).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Instant(u64);

impl Instant {
    pub fn now() -> Instant {
        Instant(crate::hooks::now_ns())
    }

    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.saturating_duration_since(earlier)
    }

    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        self.0.checked_sub(earlier.0).map(Duration::from_nanos)
    }

    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }

    pub fn elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(*self)
    }

    pub fn checked_add(&self, d: Duration) -> Option<Instant> {
        u64::try_from(d.as_nanos()).ok().and_then(|n| self.0.checked_add(n)).map(Instant)
    }

    pub fn checked_sub(&self, d: Duration) -> Option<Instant> {
        u64::try_from(d.as_nanos()).ok().and_then(|n| self.0.checked_sub(n)).map(Instant)
    }

    /// Nanoseconds since the clock's origin (for the waits).
    pub(crate) fn nanos(&self) -> u64 {
        self.0
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, d: Duration) -> Instant {
        self.checked_add(d).unwrap_or(Instant(u64::MAX))
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, d: Duration) {
        *self = *self + d;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;
    fn sub(self, d: Duration) -> Instant {
        self.checked_sub(d).unwrap_or(Instant(0))
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, d: Duration) {
        *self = *self - d;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, other: Instant) -> Duration {
        self.saturating_duration_since(other)
    }
}
