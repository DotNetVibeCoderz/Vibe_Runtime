//! Debugger hooks: stopping, stepping, and looking at a stopped program.
//!
//! This is the runtime half of what an editor needs. It does not speak any
//! protocol and knows nothing about VSCode or Rider — it exposes the three
//! things a debug adapter cannot implement for itself, because they need to be
//! *inside* the interpreter:
//!
//! 1. **Stopping** at a chosen instruction, before it runs.
//! 2. **Stepping**, which is a question about frame depth as much as position.
//! 3. **Looking**, at frames and at the values in them.
//!
//! # Why a trait rather than a channel
//!
//! A debugger is consulted synchronously, while the program is stopped and its
//! frames are intact. That is the whole point: a snapshot copied out and
//! inspected later would describe a program that has moved on. So the hook
//! borrows the frames and returns what to do next, and the interpreter does not
//! resume until it answers.
//!
//! # What it costs when nothing is attached
//!
//! One `Option::is_none` per instruction. The location is not built, the
//! breakpoint set is not consulted, and no frames are described — a program run
//! without a debugger pays a branch that predicts perfectly.

use crate::types::MethodId;
use crate::value::Value;
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::string::String;
// Only the std-side scripted debugger keeps a log.
#[cfg(feature = "std")]
use alloc::vec::Vec;

/// Somewhere a program can be stopped: an instruction of a method.
///
/// IL offset rather than instruction index, because an offset is what metadata
/// speaks and what a portable PDB maps to a source line. An index would be an
/// internal detail leaking into an external interface.
pub type Breakpoint = (MethodId, u32);

/// Breakpoints currently set.
///
/// A sorted set rather than a hash set so that the per-instruction check is a
/// binary search over a small structure with no hashing, and so the collection
/// works without `std`.
#[derive(Default, Debug, Clone)]
pub struct Breakpoints {
    at: BTreeSet<Breakpoint>,
}

impl Breakpoints {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, method: MethodId, il_offset: u32) {
        self.at.insert((method, il_offset));
    }

    pub fn remove(&mut self, method: MethodId, il_offset: u32) {
        self.at.remove(&(method, il_offset));
    }

    /// Removes every breakpoint in `method`, which is what an editor does when
    /// the breakpoints for a file are replaced wholesale.
    pub fn clear_method(&mut self, method: MethodId) {
        self.at.retain(|(m, _)| *m != method);
    }

    pub fn clear(&mut self) {
        self.at.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    pub fn len(&self) -> usize {
        self.at.len()
    }

    #[inline]
    pub fn contains(&self, method: MethodId, il_offset: u32) -> bool {
        self.at.contains(&(method, il_offset))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Breakpoint> {
        self.at.iter()
    }
}

/// Why the program stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The first instruction of the first method, before anything runs.
    Entry,
    /// A breakpoint was set here.
    Breakpoint,
    /// A step finished.
    Step,
}

/// What the debugger wants to happen next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCommand {
    /// Run until the next breakpoint, if any.
    Run,
    /// Stop at the very next instruction, wherever it is — including the first
    /// instruction of a method this one calls.
    StepInto,
    /// Stop at the next instruction in this frame or a caller, stepping *over*
    /// anything this instruction calls.
    StepOver,
    /// Run until this frame returns, then stop in the caller.
    StepOut,
    /// Stop debugging; the program runs to completion with no further hooks.
    Detach,
}

/// One frame of a stopped program.
///
/// The values are borrowed, not copied: a debugger reads them while the program
/// is stopped, which is the only time they mean anything.
pub struct DebugFrame<'a> {
    pub method: MethodId,
    /// Qualified name, for a debugger that would otherwise have to reach into
    /// the loader to render a stack.
    pub method_name: &'a str,
    /// IL offset of the instruction *about to run*, not the one that ran.
    pub il_offset: u32,
    pub arguments: &'a [Value],
    pub locals: &'a [Value],
    /// The evaluation stack. Rarely interesting to a user and always
    /// interesting when something is wrong with the runtime itself.
    pub stack: &'a [Value],
}

/// What a debugger may change while the program is stopped.
///
/// Passed separately from the frames because it is the *mutable* half: Rust
/// will hand out both at once only because they are different fields, and
/// saying so in the signature is what makes that safe rather than incidental.
pub struct DebugControl<'a> {
    pub breakpoints: &'a mut Breakpoints,
}

/// The hook an editor's adapter implements.
pub trait Debugger: Send {
    /// Called with the program stopped and its frames intact, innermost first.
    ///
    /// Returning is what resumes the program, so an adapter that wants to wait
    /// for a user simply does not return until it has an answer.
    fn stopped(
        &mut self,
        reason: StopReason,
        frames: &[DebugFrame<'_>],
        control: &mut DebugControl<'_>,
    ) -> DebugCommand;

    /// Called once when the program ends, normally or with an exception.
    fn finished(&mut self, _outcome: Result<i32, String>) {}
}

/// What the interpreter is stepping towards, between stops.
///
/// Depth is what distinguishes the three step kinds, and it is why stepping
/// belongs in the runtime: an adapter outside cannot see a frame being pushed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stepping {
    /// Stop at the next instruction, whatever the depth.
    Into,
    /// Stop at the next instruction at this depth or shallower.
    Over(usize),
    /// Stop at the next instruction strictly shallower than this.
    Out(usize),
}

impl Stepping {
    /// Whether a stop is due, given the depth now.
    #[inline]
    pub(crate) fn satisfied_at(self, depth: usize) -> bool {
        match self {
            Stepping::Into => true,
            Stepping::Over(from) => depth <= from,
            Stepping::Out(from) => depth < from,
        }
    }
}

/// Where one stop happened, as a scripted run records it.
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub reason: StopReason,
    pub method: String,
    pub il_offset: u32,
    /// How many frames were on the stack, so a test can tell stepping *into* a
    /// call from stepping over it.
    pub depth: usize,
}

/// The log a [`ScriptedDebugger`] writes, readable after the run.
///
/// Shared rather than reachable through the trait object: a debugger is moved
/// into the interpreter, and getting it back out to inspect it would mean
/// downcasting, which is a worse thing to build an interface on than a handle
/// the caller already holds.
#[cfg(feature = "std")]
pub type StopLog = std::sync::Arc<std::sync::Mutex<Vec<Stop>>>;

/// A debugger that runs a fixed script.
///
/// For tests, and for a non-interactive `rustnet debug`. It records every stop
/// so a test can assert on the *sequence* — which is what distinguishes working
/// stepping from stepping that merely terminates.
#[cfg(feature = "std")]
pub struct ScriptedDebugger {
    script: Vec<DebugCommand>,
    next: usize,
    log: StopLog,
}

#[cfg(feature = "std")]
impl ScriptedDebugger {
    /// Returns the debugger and a handle to its log.
    pub fn new(script: Vec<DebugCommand>) -> (Self, StopLog) {
        let log: StopLog = Default::default();
        (Self { script, next: 0, log: log.clone() }, log)
    }
}

#[cfg(feature = "std")]
impl Debugger for ScriptedDebugger {
    fn stopped(
        &mut self,
        reason: StopReason,
        frames: &[DebugFrame<'_>],
        _control: &mut DebugControl<'_>,
    ) -> DebugCommand {
        if let Some(top) = frames.first() {
            if let Ok(mut log) = self.log.lock() {
                log.push(Stop {
                    reason,
                    method: String::from(top.method_name),
                    il_offset: top.il_offset,
                    depth: frames.len(),
                });
            }
        }
        let command = self.script.get(self.next).copied().unwrap_or(DebugCommand::Run);
        self.next += 1;
        command
    }
}

/// Boxed for the interpreter to hold.
pub type BoxedDebugger = Box<dyn Debugger>;
