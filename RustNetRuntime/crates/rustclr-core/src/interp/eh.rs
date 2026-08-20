//! Exception dispatch: two-pass unwinding with `finally` support.
//!
//! `try`/`catch`/`finally` in IL is not structured control flow — it is a table
//! of offset ranges plus the `leave` and `endfinally` instructions. This module
//! implements the state machine that connects them:
//!
//! * `leave` queues every `finally` between the current position and the
//!   target, then runs them in order before branching.
//! * A thrown exception searches each frame for a matching `catch`, running any
//!   intervening `finally` blocks as the stack unwinds.
//! * `endfinally` advances that queue, either running the next `finally`,
//!   branching to the pending `leave` target, or resuming propagation.

use super::*;

#[allow(unused_imports)]
use crate::prelude::*;

impl Interpreter {
    /// Turns a thrown object into a propagating error.
    pub(super) fn exception_from_handle(&mut self, handle: Handle) -> ExecutionError {
        let type_name = self.type_name_of(handle);
        let message = self
            .heap
            .get_as::<ClrException>(handle)
            .map(|e| e.message.clone())
            .unwrap_or_default();
        ExecutionError::Exception {
            kind: ClrExceptionKind::Managed(type_name),
            message,
            object: handle,
        }
    }

    /// Allocates the managed object for an exception raised by the runtime.
    pub fn materialise_exception(&mut self, error: &mut ExecutionError) -> Handle {
        let ExecutionError::Exception { kind, message, object } = error else {
            return Handle::NULL;
        };
        if !object.is_null() && self.heap.is_valid(*object) {
            return *object;
        }
        let type_name = kind.type_name().to_string();
        let type_id = self
            .loader
            .registry
            .find_type_by_name(&type_name)
            .unwrap_or_else(|| self.loader.core().exception);
        let stack_trace = self.stack_trace();
        self.stats.allocations += 1;
        let handle = self.heap.alloc(ClrException {
            type_id,
            message: message.clone(),
            inner: Handle::NULL,
            stack_trace,
        });
        *object = handle;
        handle
    }

    /// Propagates an exception, unwinding frames until a handler is found.
    ///
    /// Returns `Ok(())` when a handler (or a `finally`) took control, so the
    /// caller should keep stepping. Returns `Err` when the exception escaped
    /// past `base_depth`.
    pub(super) fn dispatch_exception(
        &mut self,
        mut error: ExecutionError,
        base_depth: usize,
    ) -> ExecResult<()> {
        let exception_object = self.materialise_exception(&mut error);
        let exception_type = self.type_of(exception_object);

        while self.frames.len() > base_depth {
            let offset = self.frame_ref().executing_offset();
            let clauses = self.frame_ref().code.exception_clauses.clone();

            // Innermost clauses come first in the table, which is the order a
            // handler search must respect.
            let mut catch_target = None;
            let mut finallies = Vec::new();

            for clause in &clauses {
                if !clause.try_contains(offset) {
                    continue;
                }
                match clause.kind {
                    HandlerKind::Catch(type_token) => {
                        if catch_target.is_none()
                            && self.catch_matches(type_token, exception_type)?
                        {
                            catch_target = Some(clause.handler_offset);
                        }
                    }
                    HandlerKind::Filter(_) => {
                        // Filters need to run managed code mid-unwind. Treating
                        // them as non-matching is a documented narrowing of the
                        // spec rather than silently wrong behaviour.
                    }
                    HandlerKind::Finally | HandlerKind::Fault => {
                        // A finally already executing for this exception must
                        // not run again.
                        if !clause.handler_contains(offset) {
                            finallies.push(clause.handler_offset);
                        }
                    }
                }
                if catch_target.is_some() {
                    break;
                }
            }

            if !finallies.is_empty() {
                // Run the finally blocks first; propagation resumes afterwards.
                let frame = self.frame();
                frame.in_flight = Some(Box::new(error.clone()));
                frame.pending_finallies = finallies;
                frame.finally_resume = Some(catch_target.unwrap_or(PROPAGATE));
                let first = self.frame().pending_finallies.remove(0);
                self.frame().stack.clear();
                self.branch_to(first)?;
                return Ok(());
            }

            if let Some(handler) = catch_target {
                let frame = self.frame();
                frame.in_flight = Some(Box::new(error));
                // A catch handler starts with just the exception on the stack.
                frame.stack.clear();
                frame.stack.push(Value::Obj(exception_object));
                self.branch_to(handler)?;
                return Ok(());
            }

            self.frames.pop();
        }

        Err(error)
    }

    /// Whether a `catch` clause's type token matches the thrown exception.
    fn catch_matches(&mut self, type_token: Token, thrown: Option<TypeId>) -> ExecResult<bool> {
        // A null token catches everything, as does a handler for System.Object.
        if type_token.is_null() {
            return Ok(true);
        }
        let Some(thrown) = thrown else { return Ok(true) };

        let assembly = self.frame_ref().assembly;
        let Some(handler_type) = self
            .loader
            .resolve_type_token(self.loader.assembly(assembly), type_token)
        else {
            // An unresolvable catch type cannot be proven to match, and
            // swallowing the exception would be worse than letting it escape.
            return Ok(false);
        };

        if handler_type == self.loader.core().object || handler_type == self.loader.core().exception
        {
            return Ok(true);
        }
        Ok(self.loader.registry.is_assignable_to(thrown, handler_type))
    }

    /// `leave` / `leave.s`: exits a protected region, running its `finally`
    /// blocks on the way out.
    pub(super) fn do_leave(&mut self, target: u32) -> ExecResult<StepOutcome> {
        let offset = self.frame_ref().executing_offset();
        let clauses = self.frame_ref().code.exception_clauses.clone();

        // Every finally whose try region we are leaving, innermost first.
        let mut pending: Vec<u32> = clauses
            .iter()
            .filter(|c| matches!(c.kind, HandlerKind::Finally))
            .filter(|c| c.try_contains(offset) && !c.try_contains(target))
            .map(|c| c.handler_offset)
            .collect();

        // `leave` empties the evaluation stack (III.3.55).
        self.frame().stack.clear();
        self.frame().in_flight = None;

        if pending.is_empty() {
            self.branch_to(target)?;
            return Ok(StepOutcome::Continue);
        }

        let first = pending.remove(0);
        let frame = self.frame();
        frame.pending_finallies = pending;
        frame.finally_resume = Some(target);
        self.branch_to(first)?;
        Ok(StepOutcome::Continue)
    }

    /// `endfinally`: advance the queued-finally state machine.
    pub(super) fn do_endfinally(&mut self) -> ExecResult<StepOutcome> {
        if !self.frame_ref().pending_finallies.is_empty() {
            let next = self.frame().pending_finallies.remove(0);
            self.frame().stack.clear();
            self.branch_to(next)?;
            return Ok(StepOutcome::Continue);
        }

        match self.frame().finally_resume.take() {
            Some(PROPAGATE) => {
                let pending = self.frame().in_flight.take();
                match pending {
                    Some(e) => Err(*e),
                    None => Ok(StepOutcome::Continue),
                }
            }
            Some(target) => {
                self.frame().stack.clear();
                // A finally that ran on the way to a catch hands control there.
                let carrying = self.frame().in_flight.clone();
                if let Some(e) = carrying {
                    if let ExecutionError::Exception { object, .. } = e.as_ref() {
                        let obj = *object;
                        self.frame().stack.push(Value::Obj(obj));
                    }
                }
                self.branch_to(target)?;
                Ok(StepOutcome::Continue)
            }
            None => {
                // `endfinally` outside a protected region: fall through rather
                // than abort, matching how the CLR treats a stray handler exit.
                Ok(StepOutcome::Continue)
            }
        }
    }
}
