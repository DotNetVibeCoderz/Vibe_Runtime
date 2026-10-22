//! Collections that land inside a native call.
//!
//! A native's arguments are popped off the caller's stack before it runs, and
//! what it gathers from managed callbacks lives in Rust locals. Under a
//! collector that runs at every opportunity, a LINQ predicate that allocates
//! used to free the very delegate being called, or the objects a `Select` had
//! collected so far. In a browser app this surfaced, rarely and by timing, as a
//! `NullReferenceException` in a data grid's search.

use rustclr_core::{CaptureHost, Interpreter, Value};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll"
);

#[test]
fn values_held_by_a_native_survive_a_collection_inside_its_callbacks() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    interp.heap.set_collector(Box::new(rustclr_core::gc::MarkSweep {
        allocation_trigger: 2 * 1024,
        minimum_heap: 0,
        growth_factor: 1.0,
    }));
    interp.loader.load_from_file(FIXTURE).expect("fixture loads");
    let mut run = |name: &str| {
        let method = (0..interp.loader.registry.method_count())
            .map(|n| rustclr_core::MethodId(n as u32))
            .find(|m| interp.loader.registry.method(*m).name == name)
            .unwrap_or_else(|| panic!("the fixture declares {name}"));
        interp.invoke(method, vec![Value::I32(300)]).expect("runs under constant collection")
    };
    // Expected values from dotnet.
    let parts = [("GcConstText", 2100), ("GcStringsOnly", 1090), ("GcStringsArray", 1090), ("GcTagsArray", 2290), ("GcLoopLoop", 2290), ("GcSelectLoop", 2290), ("GcLoopSum", 2290), ("GcPartSelect", 2290), ("GcPartWhere", 138), ("GcPartAny", 1), ("GcPartLengths", 2590), ("LinqUnderCollection", 3029), ("GcEventUnsubscribe", 1990)];
    let got: Vec<(&str, Option<Value>)> = parts.iter().map(|(n, _)| (*n, run(n))).collect();
    let collections = interp.stats.collections;
    let wrong: Vec<String> = parts
        .iter()
        .zip(&got)
        .filter(|((_, want), (_, value))| value != &Some(Value::I32(*want)))
        .map(|((name, want), (_, value))| format!("{name}: got {value:?}, want {want}"))
        .collect();
    assert!(wrong.is_empty(), "after {collections} collections:
{}", wrong.join("
"));
    assert!(collections > 10, "the collector should have run many times, ran {collections}");
}
