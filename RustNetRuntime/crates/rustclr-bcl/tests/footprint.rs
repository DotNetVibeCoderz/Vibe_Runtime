//! What RustBCL costs in RAM, measured rather than reasoned about.
//!
//! `Tier::for_budget` in `embedded/demo-common` decides whether a board can
//! hold the full binding set or has to fall back to the console subset. It
//! decides from two constants, and a constant that drifts from the code it
//! describes is worse than no constant at all: the board picks `Full`, the
//! allocator refuses a 46-byte request part-way through registration, and the
//! firmware panics on a chip nobody can attach a debugger to.
//!
//! That is not hypothetical. This test exists because it happened: an M5Stack
//! Core2 with 278,528 bytes booted, reported `bcl tier full (260702 bytes
//! needed)`, and died — the bindings had grown past the number while the
//! number stayed still.
//!
//! # How it measures
//!
//! A `GlobalAlloc` that counts. The figure that matters is **peak live**, not
//! total allocated: a board needs enough memory at once, and the loader frees
//! plenty along the way.
//!
//! # Why the numbers here are larger than a board's
//!
//! This runs with `std`, where the prelude is `HashMap`; a board builds
//! without it and gets `BTreeMap`, which is smaller per entry and has no
//! table to grow. So the values asserted below are an **upper bound** on what
//! a chip pays, which is the safe direction for a threshold to be wrong in.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static COUNTING: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarding to the system allocator with the caller's layout.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && COUNTING.load(Ordering::Relaxed) == 1 {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNTING.load(Ordering::Relaxed) == 1 {
            // Saturating: a free of memory allocated before counting began
            // would otherwise wrap this to a `usize` near its maximum, and the
            // peak would be nonsense rather than visibly wrong.
            let _ = LIVE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                Some(live.saturating_sub(layout.size()))
            });
        }
        // SAFETY: `ptr` came from `alloc` with this same layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Serialises the measurements.
///
/// The counter is a `GlobalAlloc`, so it sees every allocation in the process
/// — including another test's. Cargo runs the tests in one binary on several
/// threads by default, and two measurements at once produce a peak that
/// belongs to neither.
static MEASURING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Peak live bytes while `build` runs. Call with [`MEASURING`] held.
fn peak_of(build: impl FnOnce()) -> usize {
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    COUNTING.store(1, Ordering::Relaxed);
    build();
    COUNTING.store(0, Ordering::Relaxed);
    PEAK.load(Ordering::Relaxed)
}

/// Peak live bytes for one tier, with one-time costs excluded.
///
/// The first interpreter in a process pays for lazy statics and allocator
/// warm-up that the second does not, so whichever test ran first measured
/// several kilobytes more than the others — an order-dependent number, and a
/// threshold guarded by one would fail at random. Building and dropping one
/// interpreter first makes the figure the same however the tests are
/// scheduled.
fn measure(install: fn(&mut rustclr_core::Interpreter)) -> usize {
    // Held across the warm-up as well as the measurement: the counter is a
    // `GlobalAlloc` and sees the whole process, so another test's warm-up
    // inside this window is another test's allocations in this number.
    let _serialised = MEASURING.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut warmup =
            rustclr_core::Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
        install(&mut warmup);
        std::hint::black_box(&warmup);
    }
    peak_of(|| {
        let mut interp =
            rustclr_core::Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
        install(&mut interp);
        // Kept alive to the end of the closure so the peak covers all of it.
        std::hint::black_box(&interp);
    })
}

fn full_tier() -> usize {
    measure(rustclr_bcl::install)
}

fn minimal_tier() -> usize {
    measure(rustclr_bcl::install_minimal)
}

/// The constants `Tier::for_budget` decides from, kept in step by hand.
///
/// Read out of the firmware crate's source rather than imported: it is not a
/// workspace member (it builds for four bare-metal targets and has its own
/// dependency tree), so this test reads the file the way a person would.
fn declared(name: &str) -> usize {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../embedded/demo-common/src/lib.rs"
    ))
    .expect("the shared firmware demo is where the thresholds live");
    let needle = format!("pub const {name}: usize = ");
    let start = source.find(&needle).unwrap_or_else(|| panic!("{name} is not declared")) + needle.len();
    let end = source[start..].find(';').expect("a terminated constant") + start;
    source[start..end]
        .replace('_', "")
        .trim()
        .parse()
        .expect("a decimal literal")
}

#[test]
fn the_full_tier_threshold_is_not_below_what_the_bindings_cost() {
    let measured = full_tier();
    let threshold = declared("FULL_BCL_BYTES");
    eprintln!("full tier: measured {measured} bytes, declared {threshold}");
    assert!(
        threshold >= measured,
        "FULL_BCL_BYTES says {threshold} and the bindings now peak at {measured}.\n\
         A board with a budget between the two picks the full tier and then dies\n\
         mid-registration — which is exactly what an M5Stack Core2 did. Raise the\n\
         constant in embedded/demo-common/src/lib.rs to at least {measured}."
    );
}

#[test]
fn the_minimal_tier_threshold_is_not_below_what_it_costs() {
    let measured = minimal_tier();
    let threshold = declared("MINIMAL_BCL_BYTES");
    eprintln!("minimal tier: measured {measured} bytes, declared {threshold}");
    assert!(
        threshold >= measured,
        "MINIMAL_BCL_BYTES says {threshold} and the console subset now peaks at \
         {measured}. A board between the two reports `minimal` and then dies."
    );
}

/// The subset has to be genuinely smaller, or there is no point offering it.
#[test]
fn the_minimal_tier_is_smaller_than_the_full_one() {
    let full = full_tier();
    let minimal = minimal_tier();
    eprintln!("full {full} bytes, minimal {minimal} bytes, saving {}", full - minimal);
    assert!(
        minimal < full,
        "the minimal tier costs {minimal} against the full tier's {full}"
    );
}
