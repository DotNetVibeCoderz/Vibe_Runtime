//! End-to-end: load a real C#-compiled assembly and execute it.
//!
//! These are the tests that prove the whole stack works together — metadata,
//! loader, interpreter, GC and native BCL. They skip when the fixture has not
//! been built so `cargo test` still passes without the .NET SDK.

use rustclr_core::{CaptureHost, Interpreter, Value};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/HelloWorld/bin/Release/net10.0/HelloWorld.dll"
);

/// Builds an interpreter with the BCL installed and the fixture loaded.
fn load() -> Option<(Interpreter, rustclr_core::AssemblyId)> {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: fixture not built ({FIXTURE})");
        return None;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    let assembly = interp.loader.load_from_file(FIXTURE).expect("fixture loads");
    Some((interp, assembly))
}

macro_rules! setup {
    () => {
        match load() {
            Some(x) => x,
            None => return,
        }
    };
}

/// Reads the captured stdout back off the host.
fn captured(interp: &Interpreter) -> String {
    // `Host` is a trait object; downcast through the concrete capture type by
    // re-reading it from the interpreter's host slot.
    interp.captured_output().unwrap_or_default()
}

#[test]
fn loads_the_assembly_and_finds_its_types() {
    let (interp, assembly) = setup!();
    let name = &interp.loader.assembly(assembly).name;
    assert_eq!(name, "HelloWorld");

    let program = interp
        .loader
        .registry
        .find_type_by_name("HelloWorld.Program")
        .expect("Program type is registered");
    let methods: Vec<&str> = interp
        .loader
        .registry
        .ty(program)
        .methods
        .iter()
        .map(|m| interp.loader.registry.method(*m).name.as_str())
        .collect();
    assert!(methods.contains(&"Add"));
    assert!(methods.contains(&"Factorial"));
    assert!(methods.contains(&"Main"));
}

#[test]
fn executes_a_pure_arithmetic_method() {
    let (mut interp, _) = setup!();
    let program = interp.loader.registry.find_type_by_name("HelloWorld.Program").unwrap();
    let add = interp
        .loader
        .registry
        .ty(program)
        .methods
        .iter()
        .copied()
        .find(|m| interp.loader.registry.method(*m).name == "Add")
        .unwrap();

    let result = interp
        .invoke(add, vec![Value::I32(2), Value::I32(40)])
        .expect("Add executes");
    assert_eq!(result, Some(Value::I32(42)));
}

#[test]
fn executes_a_loop_with_branches() {
    let (mut interp, _) = setup!();
    let program = interp.loader.registry.find_type_by_name("HelloWorld.Program").unwrap();
    let factorial = interp
        .loader
        .registry
        .ty(program)
        .methods
        .iter()
        .copied()
        .find(|m| interp.loader.registry.method(*m).name == "Factorial")
        .unwrap();

    for (input, expected) in [(0, 1), (1, 1), (5, 120), (10, 3_628_800)] {
        let result = interp.invoke(factorial, vec![Value::I32(input)]).unwrap();
        assert_eq!(result, Some(Value::I32(expected)), "Factorial({input})");
    }
}

#[test]
fn runs_the_entry_point_and_produces_console_output() {
    let (mut interp, assembly) = setup!();
    let exit_code = interp.run_entry_point(assembly).expect("Main runs to completion");
    assert_eq!(exit_code, 0);

    let output = captured(&interp);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines, ["Hello from RustCLR", "42", "120"], "got {output:?}");
}

#[test]
fn execution_statistics_are_recorded() {
    let (mut interp, assembly) = setup!();
    interp.run_entry_point(assembly).unwrap();
    let stats = interp.stats;
    assert!(stats.instructions_executed > 0);
    assert!(stats.calls > 0);
    assert!(stats.native_calls > 0, "Console.WriteLine is a native call");
    assert!(stats.max_frame_depth >= 2, "Main calls Add and Factorial");
}

#[test]
fn the_heap_holds_the_string_literals_the_program_used() {
    let (mut interp, assembly) = setup!();
    interp.run_entry_point(assembly).unwrap();
    assert!(interp.heap.live_count() > 0);
    assert!(interp.heap.stats().total_allocations > 0);
}
