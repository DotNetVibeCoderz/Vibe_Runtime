//! The profiler against a real program.
//!
//! The unit tests in `rustclr-core::profile` check the arithmetic; these check
//! that the arithmetic is fed the right things — that the method the program
//! actually spends its time in is the one that comes out on top, that calls are
//! counted exactly, and that an allocation is charged to the type that made it.

use rustclr_core::{profile::Profiler, CaptureHost, Interpreter};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll"
);

fn interpreter() -> Option<(Interpreter, rustclr_core::AssemblyId)> {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: the Conformance fixture is not built");
        return None;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    let id = interp.loader.load_from_file(FIXTURE).expect("the fixture loads");
    Some((interp, id))
}

#[test]
fn calls_are_counted_exactly() {
    let Some((mut interp, id)) = interpreter() else { return };
    // Sampling is turned effectively off — a period longer than the run — so
    // this measures counting alone.
    interp.profiler = Some(Profiler::new(&interp.loader.registry, u64::MAX / 4));
    interp.run_entry_point(id).expect("the fixture runs");

    let report = interp.profiler.as_ref().unwrap().report(&interp.loader.registry);
    let fib = report
        .methods
        .iter()
        .find(|m| m.name.contains("FibRecursive"))
        .expect("the fixture calls FibRecursive");
    // fib(20) makes 21,891 calls. An exact number, because a call count is
    // counted rather than sampled — if this is ever approximate, something has
    // started guessing.
    assert_eq!(
        fib.profile.calls, 21_891,
        "recursive calls are counted exactly, not estimated"
    );
}

#[test]
fn an_allocation_is_charged_to_the_type_that_made_it() {
    let Some((mut interp, id)) = interpreter() else { return };
    interp.profiler = Some(Profiler::new(&interp.loader.registry, u64::MAX / 4));
    interp.run_entry_point(id).expect("the fixture runs");

    let report = interp.profiler.as_ref().unwrap().report(&interp.loader.registry);
    let node = report
        .types
        .iter()
        .find(|t| t.name.ends_with("Node"))
        .expect("the fixture allocates Node");
    assert!(
        node.profile.count >= 40_000,
        "the allocation-pressure loop makes 40,000 nodes; got {}",
        node.profile.count
    );
    assert!(node.profile.bytes > 0, "an object with fields is not zero bytes");

    // Arrays are charged to their element type, not lumped under System.Array —
    // "45% arrays" is not an answer anybody can act on.
    assert!(
        report.types.iter().any(|t| t.name.ends_with("Int32[]")),
        "int arrays are named by element type: {:?}",
        report.types.iter().take(8).map(|t| &t.name).collect::<Vec<_>>()
    );
}

#[test]
fn every_allocation_on_the_profiled_thread_is_attributed() {
    // The profiler and the collector must not disagree about how much was
    // allocated: the profiler takes the collector's own size for each object
    // precisely so that "what allocated the bytes" adds up to "how many bytes".
    //
    // Two things are outside it, both stated rather than smoothed over:
    // a worker thread has its own interpreter and no profiler, and compiled
    // code allocates through the tier's own sink. `rustnet profile` interprets
    // by default for the second reason.
    let Some((mut interp, id)) = interpreter() else { return };
    interp.profiler = Some(Profiler::new(&interp.loader.registry, u64::MAX / 4));
    interp.run_entry_point(id).expect("the fixture runs");

    let report = interp.profiler.as_ref().unwrap().report(&interp.loader.registry);
    let (count, bytes) = report.total_allocations();
    let heap = interp.heap.stats();

    assert!(
        count <= heap.total_allocations && bytes <= heap.total_bytes_allocated,
        "the profiler cannot attribute more than the heap allocated: {count}/{bytes}          against {}/{}",
        heap.total_allocations,
        heap.total_bytes_allocated
    );
    // The fixture is mostly single-threaded, so the unattributed remainder is
    // small. A site on the *interpreted* path that stopped being counted would
    // show up here as a much larger gap — one such site did.
    let attributed = count as f64 * 100.0 / heap.total_allocations as f64;
    assert!(
        attributed > 99.0,
        "only {attributed:.1}% of allocations were attributed;          an interpreted allocation site is bypassing the counted path"
    );
}

#[test]
fn a_single_threaded_program_is_attributed_exactly() {
    // With one thread there is no remainder at all: every allocation the heap
    // accounted for is charged to exactly one type, byte for byte. This is the
    // check that would have caught the `await` cell going straight to the heap.
    let hello = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/HelloWorld/bin/Release/net10.0/HelloWorld.dll"
    );
    if !std::path::Path::new(hello).exists() {
        eprintln!("skipping: the HelloWorld fixture is not built");
        return;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    let id = interp.loader.load_from_file(hello).expect("the fixture loads");
    interp.profiler = Some(Profiler::new(&interp.loader.registry, u64::MAX / 4));
    interp.run_entry_point(id).expect("the fixture runs");

    let report = interp.profiler.as_ref().unwrap().report(&interp.loader.registry);
    let (count, bytes) = report.total_allocations();
    let heap = interp.heap.stats();
    assert_eq!(count, heap.total_allocations, "every allocation is charged to one type");
    assert_eq!(bytes, heap.total_bytes_allocated, "and for the size the heap used");
}

#[test]
fn sampling_finds_the_method_the_program_is_in() {
    let Some((mut interp, id)) = interpreter() else { return };
    // 200µs: short enough to gather samples from a two-second run, long enough
    // that the clock reads do not dominate.
    interp.profiler = Some(Profiler::new(&interp.loader.registry, 200_000));
    interp.run_entry_point(id).expect("the fixture runs");
    let now = interp.host.monotonic_nanos();
    interp.profiler.as_mut().unwrap().finish(now);

    let report = interp.profiler.as_ref().unwrap().report(&interp.loader.registry);
    assert!(report.samples > 0, "a multi-second run produced no samples");

    // `Main` calls everything, so inclusive time must reach it and self time
    // must not: a profiler that could not tell those apart would report the
    // entry point as the hot method of every program ever written.
    let main = report
        .methods
        .iter()
        .find(|m| m.name.ends_with("Program::Main()"))
        .expect("Main was sampled");
    assert!(
        main.profile.inclusive_nanos >= main.profile.self_nanos * 4,
        "Main's time is its callees': self {} vs inclusive {}",
        main.profile.self_nanos,
        main.profile.inclusive_nanos
    );

    // And the hottest method by self time is not Main.
    let hottest = &report.methods[0];
    assert!(
        !hottest.name.ends_with("Program::Main()"),
        "the entry point came out hottest, which means self time is inclusive time"
    );
}

#[test]
fn nothing_is_recorded_when_no_profiler_is_attached() {
    // The hook has to be inert, not merely cheap: a run without a profiler
    // must not leave one behind or half-fill one.
    let Some((mut interp, id)) = interpreter() else { return };
    interp.run_entry_point(id).expect("the fixture runs");
    assert!(interp.profiler.is_none(), "a profiler appeared from nowhere");
    assert!(
        interp.heap.stats().total_allocations > 0,
        "the fixture allocated, so the counters that are always on still moved"
    );
}

