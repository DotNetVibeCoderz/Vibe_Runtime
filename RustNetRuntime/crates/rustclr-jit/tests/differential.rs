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


/// A method that loops compiles on its *first* call.
///
/// A call count is a proxy for "worth compiling", and it is the wrong proxy for
/// a method called once that runs for seconds. `Sieve` and `Matrix` are exactly
/// that shape: before this they stayed interpreted however long they ran, and
/// the benchmark reported the interpreted time while a 20x speedup sat unused
/// behind a flag.
///
/// This is not on-stack replacement — the decision happens *before* the call,
/// so a method already running still cannot be swapped.
#[test]
fn a_looping_method_compiles_without_waiting_for_a_call_count() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    // The *default* threshold, which is the point: no flag and no repeated
    // calls, and it compiles anyway because the bodies loop.
    let tier = JitTier::new();
    assert!(tier.threshold > 1, "the default must not be compile-everything");
    interp.native_tier = Some(Box::new(tier));

    let assembly = interp.loader.load_from_file(FIXTURE).expect("fixture loads");
    interp.run_entry_point(assembly).expect("fixture runs");

    let (compiled, _) = interp.native_tier.as_ref().expect("a tier").stats();
    assert!(
        compiled > 0,
        "a method with a loop should compile on its first call, not wait for a count"
    );
}

/// No method of a real assembly may be turned away by the *verifier*.
///
/// `CompileError::Unsupported` is a judgement — "the backend does not take this
/// shape" — and every method is free to earn one. `CompileError::Invalid` is a
/// different claim entirely: that Roslyn emitted IL whose stack does not add
/// up. On a fixture the C# compiler produced, that claim is always the
/// verifier's bug, never the method's.
///
/// This is the check that was missing. The x86-64 path modelled every `call`
/// with the *enclosing* method's argument count and return kind, which is
/// correct only when a method calls itself. Any other call moved the depth by
/// the caller's delta instead of the callee's, the running depth drifted, and
/// the walk eventually reported "reached with stack depth 2, but 0 on another
/// path" — a stack error in a method whose stack was fine.
///
/// It stayed invisible because both answers were "declined": a method that
/// calls something else is refused either way, so the wrong reason changed
/// nothing observable. It mattered anyway — `max_stack_observed` comes off the
/// same walk and sizes the frame, and a verifier that cries wolf is one people
/// stop reading.
#[test]
fn valid_il_is_never_reported_as_invalid() {
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

    let mut backend = X64Backend::new();
    let mut checked = 0usize;
    let mut invalid = Vec::new();

    for i in 0..interp.loader.registry.method_count() {
        let id = rustclr_core::MethodId(i as u32);
        if !matches!(interp.loader.registry.method(id).kind, rustclr_core::MethodKind::Il(_)) {
            continue;
        }
        checked += 1;
        if let Err(rustclr_jit::CompileError::Invalid(e)) = backend.compile(&interp.loader, id) {
            invalid.push(format!("{}: {e}", interp.loader.registry.method(id).name));
        }
    }

    // Without this the loop could pass by examining nothing at all — the shape
    // of mistake this file has already made twice.
    assert!(checked > 50, "only {checked} IL methods examined; the fixture should have many more");
    assert!(
        invalid.is_empty(),
        "the verifier rejected {} method(s) of compiler-generated IL:\n  {}",
        invalid.len(),
        invalid.join("\n  ")
    );
}

/// Scalar replacement must actually fire, and must change what compiles.
///
/// An optimisation that produces identical output is indistinguishable from
/// one that never ran — the mistake this file has already recorded twice. So
/// this compiles the same method both ways and asserts the answer differs:
/// with the pass off the backend declines it for object access, with the pass
/// on it emits code.
#[test]
fn scalar_replacement_widens_what_the_backend_will_take() {
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

    let find = |name: &str| {
        (0..interp.loader.registry.method_count())
            .map(|i| rustclr_core::MethodId(i as u32))
            .find(|&id| interp.loader.registry.method(id).name == name)
            .unwrap_or_else(|| panic!("the fixture defines {name}"))
    };

    // `SplitsInFields` makes a Pair, updates each field on a different branch
    // of a loop, and never lets it escape.
    let splits = find("SplitsInFields");

    // Both settings compile it now — field access on a heap object reaches a
    // helper. What scalar replacement changes is *what* comes out: with it, the
    // object never exists and its fields are frame slots; without it, every
    // access is a call. The emitted size is the visible difference, and it is
    // the honest one to assert on now that "declines" no longer is.
    let mut plain = X64Backend::new();
    plain.scalarise = false;
    let by_calls = plain
        .compile(&interp.loader, splits)
        .expect("field access compiles even without scalar replacement");
    assert_eq!(
        plain.objects_scalarised, 0,
        "with the pass off, nothing should have been retired"
    );

    let mut flattening = X64Backend::new();
    assert!(flattening.scalarise, "scalar replacement is on by default");
    let compiled = flattening
        .compile(&interp.loader, splits)
        .expect("with scalar replacement it should compile");
    assert!(!compiled.bytes.is_empty(), "no machine code came out");
    assert_eq!(
        flattening.objects_scalarised, 1,
        "exactly one object should have stopped existing"
    );
    assert!(
        compiled.bytes.len() < by_calls.bytes.len(),
        "scalarised code should be smaller than one call per field access:          {} bytes against {}",
        compiled.bytes.len(),
        by_calls.bytes.len()
    );

    // And the other direction, which matters more: an object handed to another
    // method escapes, and replacing it would be wrong. The pass must refuse.
    let escapes = find("EscapesFromFields");
    let mut asked = X64Backend::new();
    let before = asked.objects_scalarised;
    let _ = asked.compile(&interp.loader, escapes);
    assert_eq!(
        asked.objects_scalarised, before,
        "an object passed to another method escapes and must not be scalarised"
    );
}

/// Guarded devirtualisation must fire, and must refuse an open hierarchy.
///
/// The refusing half is the one that matters. Class-hierarchy analysis is only
/// a proof when no other assembly can introduce a subtype; if it were trusted
/// on a public type, a later `Assembly.Load` could produce a receiver no guard
/// matches, and the compiled code would trap on a program that is perfectly
/// valid.
#[test]
fn devirtualisation_fires_and_refuses() {
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

    let find = |name: &str| {
        (0..interp.loader.registry.method_count())
            .map(|i| rustclr_core::MethodId(i as u32))
            .find(|&id| interp.loader.registry.method(id).name == name)
            .unwrap_or_else(|| panic!("the fixture defines {name}"))
    };

    // `SumSides` dispatches through a private abstract base with two sealed
    // implementations — closed, and both overrides return a constant.
    let sum_sides = find("SumSides");

    let mut plain = X64Backend::new();
    plain.devirtualise = false;
    assert!(
        plain.compile(&interp.loader, sum_sides).is_err(),
        "without devirtualisation a virtual call must be declined"
    );

    let mut guarded = X64Backend::new();
    assert!(guarded.devirtualise, "devirtualisation is on by default");
    let compiled = guarded
        .compile(&interp.loader, sum_sides)
        .expect("with devirtualisation it should compile");
    assert!(!compiled.bytes.is_empty(), "no machine code came out");

    // Two implementations, so two guards.
    let body = match &interp.loader.registry.method(sum_sides).kind {
        rustclr_core::MethodKind::Il(b) => b,
        _ => panic!("SumSides has IL"),
    };
    let instructions = rustclr_core::opcode::decode_all(&body.il).expect("decode");
    let plans = rustclr_jit::devirt::plan(&interp.loader, sum_sides, &instructions);
    let cases = plans.values().next().expect("the call site was planned");
    assert_eq!(cases.len(), 2, "PolyA and PolyB are the only implementations");
    let mut values: Vec<i64> = cases.iter().map(|(_, v)| *v).collect();
    values.sort_unstable();
    assert_eq!(values, vec![3, 4], "the guards carry each override's constant");

    // And the refusal, asserted over every `callvirt` in the whole corpus: no
    // planned site may dispatch on a type another assembly could extend.
    // Public hierarchies are everywhere in the BCL, and trusting analysis on
    // one would mean a later `Assembly.Load` could produce a receiver no guard
    // matches — trapping on a program that is perfectly valid.
    let mut planned = 0usize;
    let mut open_types_seen = 0usize;
    for i in 0..interp.loader.registry.method_count() {
        let caller = rustclr_core::MethodId(i as u32);
        let info = interp.loader.registry.method(caller);
        let rustclr_core::MethodKind::Il(body) = &info.kind else { continue };
        let Ok(ins) = rustclr_core::opcode::decode_all(&body.il) else { continue };
        let assembly = interp.loader.assembly(info.assembly);
        for (offset, _) in rustclr_jit::devirt::plan(&interp.loader, caller, &ins) {
            planned += 1;
            let call = ins.iter().find(|i| i.offset == offset).expect("the site exists");
            let token = call.operand.as_token().expect("callvirt carries a token");
            let declared = interp
                .loader
                .resolve_method_token(assembly, token)
                .expect("the site resolved when it was planned");
            let owner = interp.loader.registry.method(declared).declaring_type;
            assert!(
                !interp.loader.registry.ty(owner).externally_extensible,
                "planned a guarded dispatch on {}, which another assembly could extend",
                interp.loader.registry.ty(owner).full_name()
            );
        }
    }
    for t in interp.loader.registry.iter_types() {
        if t.externally_extensible {
            open_types_seen += 1;
        }
    }
    // Without these the loop above could pass by examining nothing.
    assert!(planned > 0, "no call site was planned, so the refusal proves nothing");
    assert!(open_types_seen > 0, "no extensible type was loaded to refuse");
}

/// A compiled loop that allocates must survive a collection reaching into it.
///
/// This is the check the safe-point work exists for, and the one that could not
/// be written before it: allocation inside a loop was declined precisely
/// *because* a collection could not be allowed to happen there. So the test
/// arranges for many collections to happen there, and asserts the answer.
///
/// The collector is swapped for one with a tiny trigger so a compiled body of a
/// few thousand iterations is interrupted repeatedly. Without a safe point the
/// backend would decline these methods outright; with one that failed to
/// publish the frame's roots, the arrays would be swept while the compiled code
/// still held pointers into them and the sums would come out wrong.
#[test]
fn a_compiled_loop_survives_collection() {
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
    // Collect as often as the policy allows, so a compiled body is interrupted
    // rather than merely eligible to be. The two allocating methods are called
    // directly rather than through the whole fixture: a trigger this small
    // starves the thread pool, and an async check timing out would say nothing
    // about safe points.
    interp.heap.set_collector(Box::new(rustclr_core::gc::MarkSweep {
        allocation_trigger: 16 * 1024,
        minimum_heap: 4 * 1024,
        growth_factor: 1.0,
    }));
    interp.native_tier = Some(Box::new(JitTier::with_threshold(1)));
    interp.loader.load_from_file(FIXTURE).expect("fixture loads");

    let cases = [
        ("AllocatesInALoop", 4000i32, 4000i32 * 6),
        ("SurvivesTheNextAllocation", 3000, 3000 * 3),
        // The one that needs the frame map: an array live at every safe point.
        ("CarriesAnArrayAcrossIterations", 4000, 4000 * 7),
        // And one that needs the collector to *trace* a reference field:
        // every node but the head is reachable only through another object.
        ("ReferenceFieldChain", 3000, 3000),
        // The one that needs the frame *chain*: the collection happens inside
        // a different compiled method, whose frame knows nothing about what
        // this one is holding.
        ("HoldsAcrossACall", 2000, 4 * (2000 + 5)),
    ];
    for (name, argument, expected) in cases {
        let method = (0..interp.loader.registry.method_count())
            .map(|i| rustclr_core::MethodId(i as u32))
            .find(|&id| interp.loader.registry.method(id).name == name)
            .unwrap_or_else(|| panic!("the fixture defines {name}"));
        let got = interp
            .invoke(method, vec![rustclr_core::Value::I32(argument)])
            .unwrap_or_else(|e| panic!("{name} failed under an aggressive collector: {e}"));
        assert_eq!(
            got,
            Some(rustclr_core::Value::I32(expected)),
            "{name} returned the wrong answer under an aggressive collector"
        );
    }

    // Without these the test could pass by never collecting and never
    // compiling — the shape of mistake this file has recorded three times.
    // Counted on the heap rather than the interpreter: a collection driven
    // from a compiled safe point never passes through `maybe_collect`, so the
    // interpreter's own tally would read zero and this assertion would be
    // measuring the wrong thing.
    let collections = interp.heap.stats().collections;
    assert!(
        collections > 1,
        "only {collections} collection(s) ran, so no compiled body was interrupted"
    );
    assert!(
        interp.stats.native_tier_calls > 0,
        "no compiled code ran, so no safe point was reached"
    );
}

/// Mutual recursion between two compiled methods.
///
/// The case that forced call tables to be resolved in two passes. A table entry
/// names the callee's entry *and the callee's own table*, because a callee
/// reads its table out of the argument array its caller builds. With one pass
/// and a cycle, the first method to be resolved would have to name a table the
/// second had not allocated yet — and named null, which compiled code then
/// jumped through.
///
/// It also has to be filled *in place*: allocating a fresh table for the second
/// pass moves it, leaving the address the first pass recorded pointing at freed
/// memory. Both mistakes crash rather than answer wrongly, and both were made.
#[test]
fn two_compiled_methods_may_call_each_other() {
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
    interp.native_tier = Some(Box::new(JitTier::with_threshold(1)));
    interp.loader.load_from_file(FIXTURE).expect("fixture loads");

    let even = (0..interp.loader.registry.method_count())
        .map(|i| rustclr_core::MethodId(i as u32))
        .find(|&id| interp.loader.registry.method(id).name == "IsEvenLength")
        .expect("the fixture defines IsEvenLength");

    for (argument, expected) in [(9i32, 1i32), (8, 0), (0, 0), (127, 1)] {
        let got = interp
            .invoke(even, vec![rustclr_core::Value::I32(argument)])
            .unwrap_or_else(|e| panic!("IsEvenLength({argument}) failed: {e}"));
        assert_eq!(
            got,
            Some(rustclr_core::Value::I32(expected)),
            "IsEvenLength({argument}) came back wrong"
        );
    }
    assert!(
        interp.stats.native_tier_calls > 0,
        "the recursion was interpreted, so this tested nothing"
    );
}

/// Compiling *everything* must still print what the interpreter prints.
///
/// The ordinary differential test compiles at threshold 1 too, but this runs
/// the whole fixture end to end the way the CLI does. It is here because the
/// call-table bugs above only appeared once enough methods compiled to call
/// each other, and neither showed up at the default threshold at all.
#[test]
fn the_whole_fixture_agrees_with_everything_compiled() {
    let Some(interpreted) = run(false) else { return };
    let Some(compiled) = run(true) else { return };
    assert_eq!(
        interpreted, compiled,
        "compiling every method changed what the program prints"
    );
    assert!(interpreted.contains("failures=0"), "the fixture itself failed");
}

/// The backend must decline a loop that allocates when it cannot poll.
///
/// The permission and the obligation are the same change: allocation in a loop
/// is allowed *because* the body can be stopped there. A backend that granted
/// the permission without emitting the poll would produce a method that can
/// exhaust the heap where the interpreter would not.
#[test]
fn allocation_in_a_loop_requires_a_safe_point() {
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

    let allocating = (0..interp.loader.registry.method_count())
        .map(|i| rustclr_core::MethodId(i as u32))
        .find(|&id| interp.loader.registry.method(id).name == "AllocatesInALoop")
        .expect("the fixture defines AllocatesInALoop");

    let body = match &interp.loader.registry.method(allocating).kind {
        rustclr_core::MethodKind::Il(b) => b,
        _ => panic!("AllocatesInALoop has IL"),
    };
    let instructions = rustclr_core::opcode::decode_all(&body.il).expect("decode");
    let analysis = rustclr_jit::verify::analyse(
        &instructions,
        body.max_stack,
        &body.exception_clauses,
        |ins| if ins.op == rustclr_core::Op::Ret { -1 } else { 0 },
    )
    .expect("verify");

    // It has a loop, and every back-edge lands with the stack empty — so it
    // gets safe points, and therefore permission to allocate inside the loop.
    let sites = rustclr_jit::translate::safepoints(&instructions, &analysis)
        .expect("its back-edges are at depth zero");
    assert!(!sites.is_empty(), "the method loops, so it must have a safe point");

    // And it compiles.
    let mut backend = X64Backend::new();
    let compiled = backend
        .compile(&interp.loader, allocating)
        .expect("a polling body may allocate in a loop");
    assert!(!compiled.bytes.is_empty(), "no machine code came out");
}

/// A compiled loop's memory must not grow with its trip count.
///
/// This is the invariant the whole safe-point design turns on, and the reason
/// allocation inside a loop was forbidden before it. The first attempt at
/// lifting that ban kept every allocation *pinned* for the length of the call,
/// which passed every correctness test and was strictly worse than declining:
/// a million iterations meant a million live arrays, so a program that ran
/// interpreted could exhaust the heap compiled.
///
/// So this measures rather than trusts. The same method is run with ten times
/// the iterations and the peak live heap is compared: it must stay flat,
/// because each iteration's array is dead by the next safe point and pruned
/// there.
#[test]
fn a_compiled_loop_does_not_grow_with_its_trip_count() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    if !cfg!(target_arch = "x86_64") {
        eprintln!("skipping: the x86-64 backend declines on this host");
        return;
    }

    let peak_for = |iterations: i32| -> usize {
        let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
        rustclr_bcl::install(&mut interp);
        interp.heap.set_collector(Box::new(rustclr_core::gc::MarkSweep {
            allocation_trigger: 64 * 1024,
            minimum_heap: 16 * 1024,
            growth_factor: 1.0,
        }));
        interp.native_tier = Some(Box::new(JitTier::with_threshold(1)));
        interp.loader.load_from_file(FIXTURE).expect("fixture loads");
        let method = (0..interp.loader.registry.method_count())
            .map(|i| rustclr_core::MethodId(i as u32))
            .find(|&id| interp.loader.registry.method(id).name == "AllocatesInALoop")
            .expect("the fixture defines AllocatesInALoop");
        let got = interp
            .invoke(method, vec![rustclr_core::Value::I32(iterations)])
            .expect("runs");
        assert_eq!(
            got,
            Some(rustclr_core::Value::I32(iterations * 6)),
            "wrong answer at {iterations} iterations"
        );
        assert!(
            interp.stats.native_tier_calls > 0,
            "the loop was interpreted, so this measures nothing"
        );
        interp.heap.stats().peak_live_bytes
    };

    let small = peak_for(2_000);
    let large = peak_for(20_000);
    assert!(
        large < small * 2,
        "ten times the iterations took {large} bytes against {small}: \
         a compiled loop is retaining what it allocates"
    );
}

/// The inliner must flatten more than one level of call.
///
/// One level takes a caller whose callees are leaves. Real code is not written
/// that way: a helper is built out of smaller helpers, and with one level the
/// caller of *that* stays interpreted. The `inlined` benchmark was 3.6× .NET
/// for exactly this reason, and 0.8× once the inliner could reach through.
///
/// The depth is the thing under test, so this compiles a method whose callee is
/// itself not a leaf and asserts it gets past the backend — and that the same
/// method is declined when the backend is told not to inline at all, so the
/// assertion cannot pass for some unrelated reason.
#[test]
fn the_inliner_flattens_more_than_one_level() {
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

    let find = |name: &str| {
        (0..interp.loader.registry.method_count())
            .map(|i| rustclr_core::MethodId(i as u32))
            .find(|&id| interp.loader.registry.method(id).name == name)
            .unwrap_or_else(|| panic!("the fixture defines {name}"))
    };

    // `ThreeLevels` calls `TripleThenAdd`, which is itself built from two
    // leaves — so splicing it is a second level.
    let deep = find("ThreeLevels");

    // Both settings compile it now — a call to another compiled method has a
    // calling convention. What inlining changes is whether the call survives:
    // spliced, the body reaches no other method and its call table is empty.
    // That is the honest discriminator now that "declines" no longer is.
    let mut plain = X64Backend::new();
    plain.inline = false;
    let by_call = plain
        .compile(&interp.loader, deep)
        .expect("a call to another compiled method is emitted, not declined");
    assert_eq!(
        by_call.call_table.len(),
        1,
        "without inlining the call should remain, targeting one callee"
    );

    let mut inlining = X64Backend::new();
    let compiled = inlining
        .compile(&interp.loader, deep)
        .expect("with inlining, a two-level chain should compile");
    assert!(!compiled.bytes.is_empty(), "no machine code came out");
    assert!(
        compiled.call_table.is_empty(),
        "with inlining the chain should be spliced away, leaving no call"
    );

    // And the answer is the same either way, which is the part that matters:
    // splicing must not change what a program computes.
    let mut interpreted = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interpreted);
    interpreted.loader.load_from_file(FIXTURE).expect("fixture loads");
    let plain_answer = interpreted
        .invoke(deep, vec![rustclr_core::Value::I32(6)])
        .expect("interpreted");

    let mut compiled_run = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut compiled_run);
    compiled_run.native_tier = Some(Box::new(JitTier::with_threshold(1)));
    compiled_run.loader.load_from_file(FIXTURE).expect("fixture loads");
    let compiled_answer = compiled_run
        .invoke(deep, vec![rustclr_core::Value::I32(6)])
        .expect("compiled");
    assert_eq!(
        plain_answer, compiled_answer,
        "splicing two levels changed the answer"
    );
    assert!(
        compiled_run.stats.native_tier_calls > 0,
        "the compiled run never entered compiled code, so it compared nothing"
    );
}
