//! The interpreter and the code generator must agree, exactly.
//!
//! A JIT bug does not announce itself: the program keeps running and quietly
//! produces a different number. So the conformance fixture is executed twice in
//! the same process — once interpreted, once with every eligible method
//! compiled on its first call — and the outputs are compared byte for byte.
//!
//! This is the check that would have caught the frame-layout bug that made
//! compiled methods return with their caller's registers destroyed.

use rustclr_core::{CaptureHost, Interpreter};
use rustclr_jit::JitTier;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll"
);

/// Runs the fixture, optionally with the code generator installed.
fn run(compile_everything: bool) -> Option<String> {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return None;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    if compile_everything {
        // Threshold 1 compiles on the first call, so the compiled path is
        // actually taken rather than left cold by a short-running fixture.
        interp.native_tier = Some(Box::new(JitTier::with_threshold(1)));
    }
    let assembly = interp.loader.load_from_file(FIXTURE).expect("fixture loads");
    let outcome = interp.run_entry_point(assembly);
    let output = interp.captured_output().unwrap_or_default();
    if let Err(e) = outcome {
        panic!(
            "fixture failed with {}: {e}\n{output}",
            if compile_everything { "the code generator" } else { "the interpreter" }
        );
    }
    Some(output)
}

#[test]
fn compiled_and_interpreted_output_is_identical() {
    let Some(interpreted) = run(false) else { return };
    let Some(compiled) = run(true) else { return };
    assert_eq!(
        interpreted, compiled,
        "the code generator disagrees with the interpreter"
    );
    assert!(
        interpreted.contains("failures=0"),
        "the fixture itself reported failures:\n{interpreted}"
    );
}

#[test]
fn the_backend_actually_compiles_something() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    interp.native_tier = Some(Box::new(JitTier::with_threshold(1)));
    let assembly = interp.loader.load_from_file(FIXTURE).expect("fixture loads");
    interp.run_entry_point(assembly).expect("fixture runs");

    let (methods, bytes) = interp.native_tier.as_ref().expect("tier").stats();
    // Without this the test above would pass trivially on a backend that
    // silently declined every method.
    assert!(methods > 0, "no method was compiled, so nothing was compared");
    assert!(bytes > 0, "no machine code was emitted");
    assert!(
        interp.stats.native_tier_calls > 0,
        "compiled code was never entered"
    );
}

#[test]
fn declining_a_method_is_not_an_error() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    let tier = JitTier::with_threshold(1);
    interp.native_tier = Some(Box::new(tier));
    let assembly = interp.loader.load_from_file(FIXTURE).expect("fixture loads");

    // The fixture is full of methods the baseline backend cannot take —
    // allocation, calls, exception handling, strings. Every one must fall back
    // to interpretation rather than failing the run.
    interp.run_entry_point(assembly).expect("declined methods are interpreted");
}
