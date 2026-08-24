//! Where a program spent its time, and what it allocated.
//!
//! # Sampling, not instrumenting
//!
//! Timing every call would change what it measures: a method whose body is
//! three instructions would spend most of its life in the profiler. This takes
//! **samples** instead. Every so often the interpreter stops between two
//! instructions, looks at its own frame stack, and records what it finds; the
//! elapsed time since the previous sample is charged to that stack.
//!
//! Charging *measured* elapsed time rather than one tick per sample is what
//! makes the numbers survive a jittery clock. `Thread::sleep(1ms)` on Windows
//! is 1–15 ms depending on the timer resolution the process happens to have,
//! and a profiler that assumed its period was honoured would report a program
//! as fifteen times faster than it was.
//!
//! # It samples itself
//!
//! There is no sampling thread. A thread that reads another thread's frame
//! stack has to stop it first, and the machinery for that already exists —
//! it is the collector's. Instead the interpreter counts down and reads the
//! clock itself, which means:
//!
//! - no synchronisation on the path that runs every instruction,
//! - the profiler works on a microcontroller, where there is no second thread
//!   to spare and [`Host::monotonic_nanos`] is a hardware timer,
//! - a sample is taken *between* instructions, so the frame stack it walks is
//!   never mid-update.
//!
//! The cost when nothing is profiling is one `Option::is_some` per
//! instruction, the same branch the debugger hook uses.
//!
//! # What it cannot see
//!
//! Time inside a **compiled** method is charged to whichever interpreted frame
//! called into it: compiled code does not run `step`, so the countdown does not
//! advance there. Time inside a long native call lands on the method that made
//! it, which is right, but it lands *after* the call returns rather than
//! during. Both are stated in `docs/limitations.md` rather than smoothed over.

use crate::types::{MethodId, TypeId, TypeRegistry};

#[allow(unused_imports)]
use crate::prelude::*;

/// How often, in nanoseconds, a sample is wanted.
pub const DEFAULT_PERIOD_NANOS: u64 = 1_000_000;

/// How many instructions pass between clock reads.
///
/// The clock is the expensive part — on Windows `Instant::now` is a `QPC` call
/// — so it is not read every instruction. 512 is short enough that a sample is
/// never late by a visible amount (a few microseconds of interpretation) and
/// long enough that the read does not show up in a profile of the profiler.
const CHECK_INTERVAL: u32 = 512;

/// One method's share of a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MethodProfile {
    /// Time sampled with this method on top of the stack.
    pub self_nanos: u64,
    /// Time sampled with this method anywhere on the stack.
    pub inclusive_nanos: u64,
    /// Samples that had this method on top.
    pub self_samples: u64,
    /// Times the method was entered. Counted, not sampled — a call is a rare
    /// enough event that an increment on it does not distort anything, and
    /// "called four million times" is often the whole answer.
    pub calls: u64,
}

/// One type's share of what was allocated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllocProfile {
    pub count: u64,
    pub bytes: u64,
}

/// What collection cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcProfile {
    pub collections: u64,
    pub total_nanos: u64,
    pub longest_nanos: u64,
}

/// The collector, sampling, and allocation attribution for one run.
pub struct Profiler {
    period_nanos: u64,
    /// Instructions left before the clock is read again.
    countdown: u32,
    /// When the run began, and when the last sample was charged.
    started: u64,
    last_sample: u64,
    /// Whether the clock has been read at all yet.
    running: bool,
    samples: u64,
    sampled_nanos: u64,
    /// Indexed by `MethodId`, not keyed by it. A profiler that hashed on the
    /// path that runs every call would be measuring itself.
    methods: Vec<MethodProfile>,
    /// Indexed by `TypeId`.
    types: Vec<AllocProfile>,
    /// Arrays, indexed by *element* type.
    ///
    /// Separate because an array's own type is often not in the registry:
    /// `new int[n]` needs no `TypeSpec` row, so every such array would be
    /// charged to `System.Array` and the answer would be "45% arrays" rather
    /// than "45% `Node[]`".
    arrays: Vec<AllocProfile>,
    gc: GcProfile,
    /// Scratch for the distinct methods in one sampled stack, kept between
    /// samples so a sample allocates nothing.
    seen: Vec<MethodId>,
}

impl Profiler {
    /// A profiler sized for a program, sampling every `period_nanos`.
    ///
    /// Sized up front from the registry: the tables are indexed, so growing
    /// them during a run would mean a bounds check with a branch on the path
    /// that counts calls.
    pub fn new(registry: &TypeRegistry, period_nanos: u64) -> Self {
        Self {
            period_nanos: period_nanos.max(1),
            countdown: CHECK_INTERVAL,
            started: 0,
            last_sample: 0,
            running: false,
            samples: 0,
            sampled_nanos: 0,
            methods: vec![MethodProfile::default(); registry.method_count()],
            types: vec![AllocProfile::default(); registry.type_count()],
            arrays: vec![AllocProfile::default(); registry.type_count()],
            gc: GcProfile::default(),
            seen: Vec::with_capacity(64),
        }
    }

    pub fn period_nanos(&self) -> u64 {
        self.period_nanos
    }

    /// Whether enough instructions have passed to be worth reading the clock.
    ///
    /// Inlined and branch-only: this runs once per instruction while profiling
    /// and must not touch the clock, the tables, or anything shared.
    #[inline]
    pub fn tick(&mut self) -> bool {
        self.countdown -= 1;
        if self.countdown == 0 {
            self.countdown = CHECK_INTERVAL;
            true
        } else {
            false
        }
    }

    /// Charges elapsed time to `stack` if a sampling period has passed.
    ///
    /// `now` is read by the caller because the clock lives on the host, which
    /// the profiler deliberately does not hold: a profiler that owned the clock
    /// would be a second thing to keep in step across `worker()`.
    pub fn sample(&mut self, now: u64, stack: &[MethodId]) {
        if !self.running {
            self.running = true;
            self.started = now;
            self.last_sample = now;
            return;
        }
        let elapsed = now.saturating_sub(self.last_sample);
        if elapsed < self.period_nanos {
            return;
        }
        self.last_sample = now;
        self.samples += 1;
        self.sampled_nanos += elapsed;

        let Some(top) = stack.last() else { return };
        if let Some(entry) = self.methods.get_mut(top.0 as usize) {
            entry.self_nanos += elapsed;
            entry.self_samples += 1;
        }

        // Inclusive time counts a method once per sample however many frames
        // it occupies: a recursive method that is 30 frames deep spent one
        // period there, not thirty.
        self.seen.clear();
        for method in stack {
            if self.seen.contains(method) {
                continue;
            }
            self.seen.push(*method);
            if let Some(entry) = self.methods.get_mut(method.0 as usize) {
                entry.inclusive_nanos += elapsed;
            }
        }
    }

    /// Records a call. One increment; no lookup.
    #[inline]
    pub fn entered(&mut self, method: MethodId) {
        if let Some(entry) = self.methods.get_mut(method.0 as usize) {
            entry.calls += 1;
        }
    }

    /// Records an allocation of `bytes` for an instance of `type_id`.
    #[inline]
    pub fn allocated(&mut self, type_id: TypeId, bytes: usize) {
        if let Some(entry) = self.types.get_mut(type_id.0 as usize) {
            entry.count += 1;
            entry.bytes += bytes as u64;
        }
    }

    /// Records an array allocation, charged to its element type.
    #[inline]
    pub fn allocated_array(&mut self, element: TypeId, bytes: usize) {
        if let Some(entry) = self.arrays.get_mut(element.0 as usize) {
            entry.count += 1;
            entry.bytes += bytes as u64;
        }
    }

    /// Records a collection that took `nanos`.
    pub fn collected(&mut self, nanos: u64) {
        self.gc.collections += 1;
        self.gc.total_nanos += nanos;
        self.gc.longest_nanos = self.gc.longest_nanos.max(nanos);
    }

    /// Marks the end of the run, so wall time covers everything after the
    /// first sample rather than stopping at the last one.
    pub fn finish(&mut self, now: u64) {
        if self.running {
            self.sampled_nanos = self.sampled_nanos.max(now.saturating_sub(self.started));
        }
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Resolves the counters into named, sorted rows.
    pub fn report(&self, registry: &TypeRegistry) -> Report {
        let mut methods: Vec<MethodRow> = self
            .methods
            .iter()
            .enumerate()
            .filter(|(_, m)| m.self_nanos > 0 || m.inclusive_nanos > 0 || m.calls > 0)
            .map(|(index, m)| MethodRow {
                name: registry.method(MethodId(index as u32)).qualified_name.clone(),
                profile: *m,
            })
            .collect();
        methods.sort_by(|a, b| {
            b.profile
                .self_nanos
                .cmp(&a.profile.self_nanos)
                .then(b.profile.calls.cmp(&a.profile.calls))
                .then(a.name.cmp(&b.name))
        });

        let named = |id: TypeId| -> String {
            let ty = registry.ty(id);
            if ty.namespace.is_empty() {
                ty.name.clone()
            } else {
                format!("{}.{}", ty.namespace, ty.name)
            }
        };
        let mut types: Vec<TypeRow> = self
            .types
            .iter()
            .enumerate()
            .filter(|(_, t)| t.count > 0)
            .map(|(index, t)| TypeRow { name: named(TypeId(index as u32)), profile: *t })
            .chain(self.arrays.iter().enumerate().filter(|(_, t)| t.count > 0).map(
                |(index, t)| TypeRow {
                    name: format!("{}[]", named(TypeId(index as u32))),
                    profile: *t,
                },
            ))
            .collect();
        types.sort_by(|a, b| {
            b.profile
                .bytes
                .cmp(&a.profile.bytes)
                .then(b.profile.count.cmp(&a.profile.count))
                .then(a.name.cmp(&b.name))
        });

        Report {
            wall_nanos: self.sampled_nanos,
            samples: self.samples,
            period_nanos: self.period_nanos,
            methods,
            types,
            gc: self.gc,
        }
    }
}

/// A method and its share, ready to print.
#[derive(Debug, Clone)]
pub struct MethodRow {
    pub name: String,
    pub profile: MethodProfile,
}

/// A type and what it allocated, ready to print.
#[derive(Debug, Clone)]
pub struct TypeRow {
    pub name: String,
    pub profile: AllocProfile,
}

/// Everything one run measured.
#[derive(Debug, Clone)]
pub struct Report {
    /// Time covered by sampling. Not the process's wall clock: it starts at
    /// the first sample, so loading and linking are outside it.
    pub wall_nanos: u64,
    pub samples: u64,
    pub period_nanos: u64,
    /// Sorted by self time, descending.
    pub methods: Vec<MethodRow>,
    /// Sorted by bytes, descending.
    pub types: Vec<TypeRow>,
    pub gc: GcProfile,
}

impl Report {
    /// This method's share of sampled time, 0–100.
    pub fn percent(&self, nanos: u64) -> f64 {
        if self.wall_nanos == 0 {
            0.0
        } else {
            nanos as f64 * 100.0 / self.wall_nanos as f64
        }
    }

    /// Total allocations across every type.
    pub fn total_allocations(&self) -> (u64, u64) {
        self.types
            .iter()
            .fold((0, 0), |(c, b), row| (c + row.profile.count, b + row.profile.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiler(methods: usize) -> Profiler {
        Profiler {
            period_nanos: 1_000,
            countdown: CHECK_INTERVAL,
            started: 0,
            last_sample: 0,
            running: false,
            samples: 0,
            sampled_nanos: 0,
            methods: vec![MethodProfile::default(); methods],
            types: Vec::new(),
            arrays: Vec::new(),
            gc: GcProfile::default(),
            seen: Vec::new(),
        }
    }

    #[test]
    fn elapsed_time_is_charged_not_one_tick_per_sample() {
        // The point of measuring rather than assuming: a sampler that fired
        // late would report a program as faster than it was.
        let mut p = profiler(2);
        let a = MethodId(0);
        p.sample(0, &[a]);
        p.sample(50_000, &[a]);
        assert_eq!(p.samples, 1, "one sample");
        assert_eq!(
            p.methods[0].self_nanos, 50_000,
            "the whole 50µs gap belongs to the stack that was running across it"
        );
    }

    #[test]
    fn a_period_that_has_not_passed_is_not_a_sample() {
        let mut p = profiler(2);
        p.sample(0, &[MethodId(0)]);
        p.sample(500, &[MethodId(0)]);
        assert_eq!(p.samples, 0, "500ns is under the 1µs period");
        assert_eq!(p.methods[0].self_nanos, 0);
    }

    #[test]
    fn recursion_counts_once_towards_inclusive_time() {
        // A method 4 frames deep spent one period there, not four.
        let mut p = profiler(2);
        let f = MethodId(0);
        p.sample(0, &[f]);
        p.sample(10_000, &[f, f, f, f]);
        assert_eq!(p.methods[0].inclusive_nanos, 10_000);
        assert_eq!(p.methods[0].self_nanos, 10_000, "it is also on top");
    }

    #[test]
    fn a_caller_gets_inclusive_time_and_the_callee_gets_self_time() {
        let mut p = profiler(2);
        let caller = MethodId(0);
        let callee = MethodId(1);
        p.sample(0, &[caller, callee]);
        p.sample(10_000, &[caller, callee]);
        assert_eq!(p.methods[0].self_nanos, 0, "the caller was not running");
        assert_eq!(p.methods[0].inclusive_nanos, 10_000, "but its callee was");
        assert_eq!(p.methods[1].self_nanos, 10_000);
    }

    #[test]
    fn the_clock_is_read_once_every_interval() {
        let mut p = profiler(1);
        let mut ticks = 0;
        for _ in 0..CHECK_INTERVAL * 4 {
            if p.tick() {
                ticks += 1;
            }
        }
        assert_eq!(ticks, 4, "four intervals, four clock reads");
    }
}
