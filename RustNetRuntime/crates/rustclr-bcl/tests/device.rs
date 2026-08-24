//! The HAL, driven by a real C# program against the simulated board.
//!
//! `tests/fixtures/Device` is the Conformance fixture's sibling for hardware.
//! It cannot be diffed against `dotnet` the way Conformance is — there is no
//! board there, which is the whole point of these bindings — so it asserts
//! against the simulator's documented behaviour instead, and reports its own
//! tally the same way.

use rustclr_core::{CaptureHost, Interpreter};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Device/bin/Release/net10.0/Device.dll"
);

/// The fixture's output, run with or without a board installed.
fn run(with_board: bool) -> Option<(String, i32)> {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: the Device fixture is not built");
        return None;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    if with_board {
        interp.board = Some(Box::new(rustclr_hal_host::HostBoard::new()));
    }
    let id = interp.loader.load_from_file(FIXTURE).expect("the fixture loads");
    let code = interp.run_entry_point(id).unwrap_or(-1);
    let out = interp.host.captured_output().unwrap_or_default().to_string();
    Some((out, code))
}

#[test]
fn every_hal_check_passes_against_the_simulator() {
    let Some((out, code)) = run(true) else { return };
    assert!(
        out.contains("failures=0"),
        "the device fixture reported failures:\n{out}"
    );
    assert_eq!(code, 0, "the fixture exited non-zero:\n{out}");
    // The tally is asserted, not just the absence of failures: a fixture whose
    // checks all stopped running would also report zero of them.
    let checks: u32 = out
        .split("checks=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("the fixture prints its tally");
    assert!(checks >= 43, "only {checks} checks ran:\n{out}");
}

#[test]
fn a_referenced_assembly_is_loaded_from_beside_the_program() {
    // `Device.dll` references `RustNet.Device.dll`, which sits next to it. A
    // loader that did not follow the reference would fail every `RustNet.Hal`
    // token, which is exactly what happened before it did.
    if !std::path::Path::new(FIXTURE).exists() {
        return;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    interp.loader.load_from_file(FIXTURE).expect("the fixture loads");

    assert!(
        interp.loader.registry.find_type_by_name("RustNet.Hal.Gpio").is_some(),
        "the referenced library's types are not in the registry"
    );
    // Four now, not one: the device libraries are separate assemblies so a
    // program pays only for what it references. `Device.dll` names Hal, IO,
    // Security and Graphics, and every one has to be found beside it.
    for expected in ["RustNet.Hal", "RustNet.IO", "RustNet.Security", "RustNet.Graphics"] {
        assert!(
            interp.loader.assemblies().iter().any(|a| a.name == expected),
            "{expected} was never loaded"
        );
    }
}

#[test]
fn reaching_for_hardware_that_is_not_there_says_so() {
    // Not a silent no-op: a program that writes a pin on a machine with no
    // board is doing something that will not happen, and it is entitled to
    // find that out. The message has to name what was missing and what to do.
    let Some((out, code)) = run(false) else { return };
    assert_ne!(code, 0, "the fixture should not have succeeded:\n{out}");
    assert!(
        out.contains("FAIL a board is installed"),
        "the fixture did not notice the missing board:\n{out}"
    );
}
