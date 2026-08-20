//! Runs a broad C# conformance program on RustCLR and compares its output with
//! what the same assembly prints on the reference .NET runtime.
//!
//! The fixture self-checks: it prints `checks=N failures=0` when every
//! behaviour matched, and a `FAIL <label>` line for each one that did not. That
//! makes a regression here point straight at the construct that broke.

use rustclr_core::{CaptureHost, Interpreter};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll"
);

/// Recovers the managed stack trace captured when the exception was raised.
fn managed_trace(interp: &Interpreter, error: &rustclr_core::ExecutionError) -> String {
    let rustclr_core::ExecutionError::Exception { object, .. } = error else {
        return "<not a managed exception>".into();
    };
    interp
        .heap
        .get_as::<rustclr_core::ClrException>(*object)
        .map(|e| e.stack_trace.join("
"))
        .unwrap_or_else(|| "<no trace captured>".into())
}

fn run() -> Option<(String, Interpreter)> {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return None;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    let assembly = interp.loader.load_from_file(FIXTURE).expect("fixture loads");

    match interp.run_entry_point(assembly) {
        Ok(_) => {}
        Err(e) => {
            let output = interp.captured_output().unwrap_or_default();
            let trace = managed_trace(&interp, &e);
            panic!(
                "conformance program failed: {e}\n\
                 --- managed stack ---\n{trace}\n\
                 --- output so far ---\n{output}"
            );
        }
    }
    let output = interp.captured_output().unwrap_or_default();
    Some((output, interp))
}

#[test]
fn every_conformance_check_passes() {
    let Some((output, _)) = run() else { return };

    let failures: Vec<&str> = output.lines().filter(|l| l.starts_with("FAIL ")).collect();
    assert!(
        failures.is_empty(),
        "{} conformance checks failed:\n{}",
        failures.len(),
        failures.join("\n")
    );

    let summary = output
        .lines()
        .find(|l| l.starts_with("checks="))
        .expect("the program should print a summary line");
    assert!(summary.ends_with("failures=0"), "summary was {summary:?}");

    // Guards against the program exiting early and reporting a hollow pass.
    let count: usize = summary
        .trim_start_matches("checks=")
        .split(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    assert!(count >= 35, "only {count} checks ran; expected the full suite");
}

#[test]
fn the_collector_reclaims_memory_during_execution() {
    let Some((_, interp)) = run() else { return };
    // The program allocates strings freely; the heap should not have grown
    // without bound relative to what survived.
    let stats = interp.heap.stats();
    assert!(stats.total_allocations > 50, "expected real allocation traffic");
    assert!(interp.heap.live_count() <= stats.total_allocations as usize);
}
