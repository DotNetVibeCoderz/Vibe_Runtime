//! Runs the two-assembly fixture: generic code and interfaces declared in one
//! assembly, instantiated and implemented in another. Every shape it checks
//! once worked inside a single assembly and failed across two — the
//! conformance fixture, being one assembly, could not see that.

use rustclr_core::{CaptureHost, Interpreter};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/CrossAssembly/App/bin/Release/net10.0");

#[test]
fn generic_code_and_interfaces_work_across_assemblies() {
    let lib = format!("{DIR}/CrossLib.dll");
    let app = format!("{DIR}/CrossApp.dll");
    if !std::path::Path::new(&app).exists() {
        eprintln!("skipping: cross-assembly fixture not built");
        return;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    interp.loader.load_from_file(&lib).expect("library loads");
    let assembly = interp.loader.load_from_file(&app).expect("program loads");
    if let Err(e) = interp.run_entry_point(assembly) {
        panic!("cross-assembly program failed: {e}\n{}", interp.captured_output().unwrap_or_default());
    }
    let output = interp.captured_output().unwrap_or_default();
    let failures: Vec<&str> = output.lines().filter(|l| l.starts_with("FAIL ")).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(output.contains("failures=0"), "unexpected output:\n{output}");
}
