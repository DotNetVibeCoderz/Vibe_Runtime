//! The debugger hooks, against a real Roslyn-compiled assembly.
//!
//! Stepping is easy to get subtly wrong and easy to test badly: a step that
//! always stops *eventually* looks identical to one that stops in the right
//! place, if all a test checks is that the program finished. So these assert on
//! the sequence of stops — where each landed and how deep — and on the answers
//! still being right with a debugger attached.

use rustclr_core::debug::{DebugCommand, ScriptedDebugger, StopReason};
use rustclr_core::{CaptureHost, Interpreter, MethodId, MethodKind, Value};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll"
);

fn interpreter() -> Option<Interpreter> {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return None;
    }
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    interp.loader.load_from_file(FIXTURE).expect("fixture loads");
    Some(interp)
}

fn method(interp: &Interpreter, name: &str) -> MethodId {
    (0..interp.loader.registry.method_count())
        .map(|i| MethodId(i as u32))
        .find(|&id| interp.loader.registry.method(id).name == name)
        .unwrap_or_else(|| panic!("the fixture defines {name}"))
}

fn il_offsets(interp: &Interpreter, m: MethodId) -> Vec<u32> {
    let MethodKind::Il(body) = &interp.loader.registry.method(m).kind else {
        panic!("expected IL");
    };
    rustclr_core::opcode::decode_all(&body.il)
        .expect("decode")
        .iter()
        .map(|i| i.offset)
        .collect()
}

#[test]
fn a_breakpoint_stops_before_the_instruction_it_names() {
    let Some(mut interp) = interpreter() else { return };
    let triple = method(&interp, "Triple"); // v * 3 + (v >> 1)
    let offsets = il_offsets(&interp, triple);
    // The third instruction, so "stopped before it" is distinguishable both
    // from "stopped at the start" and from "stopped at the end".
    let target = offsets[2];

    interp.breakpoints.add(triple, target);
    let (debugger, log) = ScriptedDebugger::new(vec![DebugCommand::Run, DebugCommand::Run]);
    interp.attach_debugger(Box::new(debugger));

    let answer = interp.invoke(triple, vec![Value::I32(8)]).expect("runs");
    assert_eq!(
        answer,
        Some(Value::I32(8 * 3 + (8 >> 1))),
        "attaching a debugger changed the answer"
    );

    let stops = log.lock().unwrap();
    assert_eq!(stops.len(), 2, "expected an entry stop and a breakpoint: {stops:?}");
    assert_eq!(stops[0].reason, StopReason::Entry);
    assert_eq!(stops[1].reason, StopReason::Breakpoint);
    assert_eq!(stops[1].il_offset, target, "stopped at the wrong offset");
    assert!(stops[1].method.contains("Triple"));
}

#[test]
fn stepping_into_enters_the_callee_and_stepping_over_does_not() {
    let Some(mut interp) = interpreter() else { return };
    // `TwoLevels` calls `Triple`, `Quintuple` and `Add4`.
    let two_levels = method(&interp, "TwoLevels");
    let entry = il_offsets(&interp, two_levels)[0];

    interp.breakpoints.add(two_levels, entry);
    let steps = vec![DebugCommand::StepInto; 9];
    let mut script = vec![DebugCommand::Run];
    script.extend(steps);
    script.push(DebugCommand::Run);
    let (debugger, log) = ScriptedDebugger::new(script);
    interp.attach_debugger(Box::new(debugger));
    interp.invoke(two_levels, vec![Value::I32(9)]).expect("runs");

    let into = log.lock().unwrap().clone();
    let start_depth = into
        .iter()
        .find(|s| s.reason == StopReason::Breakpoint)
        .expect("hit the breakpoint")
        .depth;
    assert!(
        into.iter().any(|s| s.depth > start_depth),
        "stepping into never entered a callee: {into:?}"
    );

    // The same place, the same number of steps, stepping over: the depth must
    // never exceed where it started.
    let Some(mut interp) = interpreter() else { return };
    interp.breakpoints.add(two_levels, entry);
    let mut script = vec![DebugCommand::Run];
    script.extend(vec![DebugCommand::StepOver; 9]);
    script.push(DebugCommand::Run);
    let (debugger, log) = ScriptedDebugger::new(script);
    interp.attach_debugger(Box::new(debugger));
    interp.invoke(two_levels, vec![Value::I32(9)]).expect("runs");

    let over = log.lock().unwrap().clone();
    let after_break: Vec<_> = over
        .iter()
        .skip_while(|s| s.reason != StopReason::Breakpoint)
        .collect();
    assert!(after_break.len() > 2, "not enough stops to prove anything: {over:?}");
    for stop in &after_break {
        assert!(
            stop.depth <= start_depth,
            "stepping over entered a callee at depth {} (started at {start_depth}): {over:?}",
            stop.depth
        );
    }
}

#[test]
fn stepping_out_returns_to_the_caller() {
    let Some(mut interp) = interpreter() else { return };
    let triple = method(&interp, "Triple");
    let two_levels = method(&interp, "TwoLevels");

    // Break inside the callee, then step out: the next stop must be shallower
    // and in the caller.
    interp.breakpoints.add(triple, il_offsets(&interp, triple)[0]);
    let (debugger, log) = ScriptedDebugger::new(vec![
        DebugCommand::Run,
        DebugCommand::StepOut,
        DebugCommand::Run,
    ]);
    interp.attach_debugger(Box::new(debugger));
    interp.invoke(two_levels, vec![Value::I32(9)]).expect("runs");

    let stops = log.lock().unwrap().clone();
    let inside = stops
        .iter()
        .position(|s| s.reason == StopReason::Breakpoint)
        .expect("broke inside Triple");
    let after = stops.get(inside + 1).expect("a stop after stepping out");
    assert_eq!(after.reason, StopReason::Step);
    assert!(
        after.depth < stops[inside].depth,
        "stepping out did not leave the frame: {stops:?}"
    );
    assert!(
        after.method.contains("TwoLevels"),
        "stepping out landed in {} rather than the caller",
        after.method
    );
}

#[test]
fn a_debugger_can_read_arguments_and_locals() {
    let Some(mut interp) = interpreter() else { return };
    let triple = method(&interp, "Triple");
    let offsets = il_offsets(&interp, triple);
    interp.breakpoints.add(triple, offsets[1]);

    // Written inline, because what is under test is the *view* the hook hands
    // over rather than any scripted behaviour.
    struct Reader {
        seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    }
    impl rustclr_core::debug::Debugger for Reader {
        fn stopped(
            &mut self,
            reason: StopReason,
            frames: &[rustclr_core::debug::DebugFrame<'_>],
            _control: &mut rustclr_core::debug::DebugControl<'_>,
        ) -> DebugCommand {
            if reason == StopReason::Breakpoint {
                self.seen.lock().unwrap().extend(frames[0].arguments.iter().cloned());
            }
            DebugCommand::Run
        }
    }
    let seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
    interp.attach_debugger(Box::new(Reader { seen: seen.clone() }));
    interp.invoke(triple, vec![Value::I32(21)]).expect("runs");

    let seen = seen.lock().unwrap();
    assert_eq!(
        &seen[..],
        &[Value::I32(21)],
        "the debugger did not see the argument it was called with"
    );
}

#[test]
fn a_debugger_can_set_a_breakpoint_while_stopped() {
    let Some(mut interp) = interpreter() else { return };
    let triple = method(&interp, "Triple");
    let offsets = il_offsets(&interp, triple);

    // Nothing is set up front: the debugger adds one at its entry stop, and it
    // has to take effect on this same run.
    struct SelfArming {
        target: (MethodId, u32),
        armed: bool,
        hits: std::sync::Arc<std::sync::Mutex<usize>>,
    }
    impl rustclr_core::debug::Debugger for SelfArming {
        fn stopped(
            &mut self,
            reason: StopReason,
            _frames: &[rustclr_core::debug::DebugFrame<'_>],
            control: &mut rustclr_core::debug::DebugControl<'_>,
        ) -> DebugCommand {
            if !self.armed {
                control.breakpoints.add(self.target.0, self.target.1);
                self.armed = true;
            } else if reason == StopReason::Breakpoint {
                *self.hits.lock().unwrap() += 1;
            }
            DebugCommand::Run
        }
    }
    let hits: std::sync::Arc<std::sync::Mutex<usize>> = Default::default();
    interp.attach_debugger(Box::new(SelfArming {
        target: (triple, offsets[2]),
        armed: false,
        hits: hits.clone(),
    }));
    interp.invoke(triple, vec![Value::I32(3)]).expect("runs");

    assert_eq!(
        *hits.lock().unwrap(),
        1,
        "a breakpoint set while stopped did not take effect on the same run"
    );
}
