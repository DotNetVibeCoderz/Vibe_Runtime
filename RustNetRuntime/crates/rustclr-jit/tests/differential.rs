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
use rustclr_jit::{Compiler, JitTier, X64Backend};

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

/// The point of the inliner: a method the backend used to decline is compiled
/// now, and still produces the same answer.
///
/// Counting compiled methods across the whole fixture would be the obvious
/// check and is a misleading one — inlining `Scale` into `Blend` means `Scale`
/// is never *called*, so it never gets hot enough to compile, and the total
/// can go down while the inliner is working perfectly. So this names the
/// method and asks the backend about it directly.
#[test]
fn inlining_widens_what_the_backend_will_take() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    if !cfg!(target_arch = "x86_64") {
        eprintln!("skipping: the x86-64 backend declines on this host");
        return;
    }

    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    interp.loader.load_from_file(FIXTURE).expect("fixture loads");

    // `Blend` calls two small static leaves and is otherwise pure integer
    // arithmetic — the exact shape the inliner exists for.
    let blend = (0..interp.loader.registry.method_count())
        .map(|i| rustclr_core::MethodId(i as u32))
        .find(|&id| interp.loader.registry.method(id).name == "Blend")
        .expect("the fixture defines Blend");

    let mut plain = X64Backend::new();
    plain.inline = false;
    assert!(
        !plain.can_compile(&interp.loader.registry, blend),
        "Blend contains calls, so without inlining the backend must decline it"
    );

    let mut inlining = X64Backend::new();
    assert!(inlining.inline, "inlining is on by default");
    assert!(
        inlining.can_compile(&interp.loader.registry, blend),
        "with inlining, Blend should get past the screen"
    );
    let compiled = inlining
        .compile(&interp.loader, blend)
        .expect("and should then actually emit");
    assert!(!compiled.bytes.is_empty(), "no machine code came out");
}

/// Inlining must not change what the program prints.
///
/// The differential test above already compares compiled against interpreted;
/// this compares compiled-with-inlining against compiled-without, which is the
/// axis that would catch a bad argument order or a stale local index.
#[test]
fn inlining_does_not_change_the_answer() {
    let output = |inline: bool| -> Option<String> {
        if !std::path::Path::new(FIXTURE).exists() {
            eprintln!("skipping: conformance fixture not built");
            return None;
        }
        let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
        rustclr_bcl::install(&mut interp);
        let mut tier = JitTier::with_threshold(1);
        tier.set_inline(inline);
        interp.native_tier = Some(Box::new(tier));
        let assembly = interp.loader.load_from_file(FIXTURE).expect("fixture loads");
        interp.run_entry_point(assembly).expect("fixture runs");
        Some(interp.captured_output().unwrap_or_default())
    };

    let Some(plain) = output(false) else { return };
    let Some(inlined) = output(true) else { return };
    assert_eq!(plain, inlined, "inlining changed the program's output");
    assert!(inlined.contains("failures=0"), "the fixture reported failures:\n{inlined}");
}
