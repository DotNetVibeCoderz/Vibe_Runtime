//! IL verification and method analysis.
//!
//! Everything here is a prerequisite for compiling a method: a code generator
//! needs to know the stack depth at every instruction, which offsets are branch
//! targets (and therefore basic-block leaders), and whether the method does
//! anything a simple backend cannot express.
//!
//! Running it as a verifier in its own right catches malformed IL before the
//! interpreter trips over it.

use rustclr_core::metadata::{ExceptionClause, HandlerKind};
use rustclr_core::opcode::{Instruction, Op, OperandKind};
use std::collections::{BTreeSet, HashMap};

/// Sentinel a delta callback returns when it could not resolve a call target.
///
/// Chosen far outside any real stack effect so it cannot collide with one.
pub const UNRESOLVED_CALL: i32 = i32::MIN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// A branch pointed at an offset that is not an instruction boundary.
    BadBranchTarget { at: u32, target: u32 },
    /// The evaluation stack would underflow.
    StackUnderflow { at: u32, op: &'static str },
    /// Two paths reach the same instruction with different stack depths.
    InconsistentStack { at: u32, expected: i32, found: i32 },
    /// The stack is deeper than the method header declared.
    StackOverflow { at: u32, depth: i32, max: u16 },
    /// Control can fall off the end of the method.
    FallsOffTheEnd,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadBranchTarget { at, target } => {
                write!(f, "IL_{at:04X}: branch to IL_{target:04X}, which is not an instruction")
            }
            Self::StackUnderflow { at, op } => {
                write!(f, "IL_{at:04X}: `{op}` would underflow the evaluation stack")
            }
            Self::InconsistentStack { at, expected, found } => write!(
                f,
                "IL_{at:04X}: reached with stack depth {found}, but {expected} on another path"
            ),
            Self::StackOverflow { at, depth, max } => {
                write!(f, "IL_{at:04X}: stack depth {depth} exceeds the declared maximum {max}")
            }
            Self::FallsOffTheEnd => write!(f, "control falls off the end of the method"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// What analysis learned about a method.
#[derive(Debug, Clone, Default)]
pub struct MethodAnalysis {
    /// Highest evaluation-stack depth reached.
    pub max_stack_observed: u16,
    /// IL offsets that begin a basic block.
    pub block_leaders: BTreeSet<u32>,
    /// Stack depth on entry to each instruction offset.
    pub depth_at: HashMap<u32, i32>,
    /// True when the method calls nothing — the easiest case to compile.
    pub is_leaf: bool,
    /// True when the method allocates.
    pub allocates: bool,
    /// True when the method has protected regions.
    pub has_exception_handling: bool,
    /// True when the method jumps backwards, i.e. contains a loop.
    pub has_loops: bool,
    /// Rough size metric used to decide inlining.
    pub il_size: u32,
    pub instruction_count: usize,
    /// Set when a `call` target could not be resolved, which makes the measured
    /// stack depth unreliable past that point.
    pub has_unresolved_calls: bool,
}

impl MethodAnalysis {
    /// Whether a method is a plausible inlining candidate.
    ///
    /// The thresholds mirror the shape of the decision rather than any
    /// particular tuning: small, loop-free, exception-free bodies.
    pub fn is_inline_candidate(&self) -> bool {
        self.il_size <= 32 && !self.has_loops && !self.has_exception_handling
    }

    /// Whether a straightforward native backend could handle this method.
    pub fn is_compilable_by_baseline_backend(&self) -> bool {
        self.is_leaf && !self.allocates && !self.has_exception_handling
    }
}

/// Net effect of an instruction on evaluation-stack depth.
///
/// Instructions whose effect depends on a signature (`call`, `newobj`, `ret`)
/// return `None`; the caller supplies the answer from metadata.
pub fn stack_effect(op: Op) -> Option<(u32, u32)> {
    use Op::*;
    // (popped, pushed)
    Some(match op {
        Nop | Break | Volatile | Readonly | Tail | Unaligned | No | Constrained => (0, 0),

        Ldnull | LdcI4M1 | LdcI40 | LdcI41 | LdcI42 | LdcI43 | LdcI44 | LdcI45 | LdcI46
        | LdcI47 | LdcI48 | LdcI4S | LdcI4 | LdcI8 | LdcR4 | LdcR8 | Ldstr | Ldarg0 | Ldarg1
        | Ldarg2 | Ldarg3 | LdargS | Ldarg | LdargaS | Ldarga | LdlocS | Ldloc | Ldloc0
        | Ldloc1 | Ldloc2 | Ldloc3 | LdlocaS | Ldloca | Ldsfld | Ldsflda | Ldtoken | Sizeof
        | Arglist | Ldftn => (0, 1),

        Pop | StargS | Starg | StlocS | Stloc | Stloc0 | Stloc1 | Stloc2 | Stloc3 | Stsfld
        | Throw | Brtrue | BrtrueS | Brfalse | BrfalseS | Switch | Endfilter => (1, 0),

        Dup => (1, 2),

        Ldfld | Ldflda | Ldlen | Neg | Not | Box | Unbox | UnboxAny | Castclass | Isinst
        | Newarr | Ldobj | Ldvirtftn | Ckfinite | LdindI1 | LdindU1 | LdindI2 | LdindU2
        | LdindI4 | LdindU4 | LdindI8 | LdindI | LdindR4 | LdindR8 | LdindRef | ConvI1
        | ConvI2 | ConvI4 | ConvI8 | ConvR4 | ConvR8 | ConvU4 | ConvU8 | ConvU1 | ConvU2
        | ConvI | ConvU | ConvRUn | ConvOvfI1 | ConvOvfI2 | ConvOvfI4 | ConvOvfI8 | ConvOvfU1
        | ConvOvfU2 | ConvOvfU4 | ConvOvfU8 | ConvOvfI | ConvOvfU | ConvOvfI1Un | ConvOvfI2Un
        | ConvOvfI4Un | ConvOvfI8Un | ConvOvfU1Un | ConvOvfU2Un | ConvOvfU4Un | ConvOvfU8Un
        | ConvOvfIUn | ConvOvfUUn | Refanyval | Refanytype | Mkrefany | Localloc => (1, 1),

        Add | Sub | Mul | Div | DivUn | Rem | RemUn | And | Or | Xor | Shl | Shr | ShrUn | Ceq
        | Cgt | CgtUn | Clt | CltUn | AddOvf | AddOvfUn | SubOvf | SubOvfUn | MulOvf
        | MulOvfUn | Ldelema | LdelemI1 | LdelemU1 | LdelemI2 | LdelemU2 | LdelemI4 | LdelemU4
        | LdelemI8 | LdelemI | LdelemR4 | LdelemR8 | LdelemRef | Ldelem => (2, 1),

        Stfld | StindRef | StindI1 | StindI2 | StindI4 | StindI8 | StindR4 | StindR8 | StindI
        | Stobj | Cpobj => (2, 0),

        // Two-operand conditional branches consume both and push nothing.
        Beq | BeqS | BneUn | BneUnS | Bge | BgeS | Bgt | BgtS | Ble | BleS | Blt | BltS
        | BgeUn | BgeUnS | BgtUn | BgtUnS | BleUn | BleUnS | BltUn | BltUnS => (2, 0),

        Initobj => (1, 0),

        StelemI | StelemI1 | StelemI2 | StelemI4 | StelemI8 | StelemR4 | StelemR8 | StelemRef
        | Stelem | Cpblk | Initblk => (3, 0),

        Br | BrS | Leave | LeaveS | Endfinally | Rethrow => (0, 0),

        // Signature-dependent.
        Call | Callvirt | Calli | Newobj | Ret | Jmp => return None,
    })
}

/// Analyses a decoded method body.
///
/// `stack_delta_for_call` supplies the net effect of signature-dependent
/// instructions, which only the caller can know.
pub fn analyse(
    instructions: &[Instruction],
    declared_max_stack: u16,
    clauses: &[ExceptionClause],
    mut stack_delta_for_call: impl FnMut(&Instruction) -> i32,
) -> Result<MethodAnalysis, VerifyError> {
    let has_exception_handling = !clauses.is_empty();
    let mut analysis = MethodAnalysis {
        is_leaf: true,
        has_exception_handling,
        instruction_count: instructions.len(),
        il_size: instructions.last().map_or(0, |i| i.next_offset()),
        ..Default::default()
    };

    let valid_offsets: BTreeSet<u32> = instructions.iter().map(|i| i.offset).collect();
    let end_offset = analysis.il_size;

    // Every method entry and every branch target starts a basic block.
    analysis.block_leaders.insert(0);
    for ins in instructions {
        let targets: Vec<u32> = match &ins.operand {
            rustclr_core::opcode::Operand::Target(t) => vec![*t],
            rustclr_core::opcode::Operand::Targets(ts) => ts.clone(),
            _ => Vec::new(),
        };
        for t in targets {
            if t != end_offset && !valid_offsets.contains(&t) {
                return Err(VerifyError::BadBranchTarget { at: ins.offset, target: t });
            }
            if t <= ins.offset {
                analysis.has_loops = true;
            }
            analysis.block_leaders.insert(t);
            // The instruction after a branch also leads a block.
            analysis.block_leaders.insert(ins.next_offset());
        }
        match ins.op {
            Op::Call | Op::Callvirt | Op::Calli | Op::Newobj | Op::Jmp => analysis.is_leaf = false,
            Op::Newarr | Op::Box | Op::Ldstr => analysis.allocates = true,
            _ => {}
        }
        if ins.op == Op::Newobj {
            analysis.allocates = true;
        }
    }

    // Abstract interpretation of stack depth, following every edge once.
    let index_of: HashMap<u32, usize> =
        instructions.iter().enumerate().map(|(i, ins)| (ins.offset, i)).collect();
    let mut worklist: Vec<(u32, i32)> = vec![(0, 0)];

    // Handler entry points are reached by the runtime, not by a branch, so they
    // must be seeded explicitly. A `catch` or `filter` starts with the exception
    // already on the stack; a `finally` or `fault` starts empty. Seeding *every*
    // block leader at depth 1 — as an earlier version did — inflates the
    // measured depth and reports methods as exceeding their declared maximum
    // when they do not.
    for clause in clauses {
        match clause.kind {
            HandlerKind::Catch(_) => worklist.push((clause.handler_offset, 1)),
            HandlerKind::Filter(filter_offset) => {
                worklist.push((filter_offset, 1));
                worklist.push((clause.handler_offset, 1));
            }
            HandlerKind::Finally | HandlerKind::Fault => {
                worklist.push((clause.handler_offset, 0))
            }
        }
        analysis.block_leaders.insert(clause.handler_offset);
        analysis.block_leaders.insert(clause.try_offset);
    }

    while let Some((offset, incoming)) = worklist.pop() {
        if offset == end_offset {
            continue;
        }
        let Some(&index) = index_of.get(&offset) else { continue };

        if let Some(&known) = analysis.depth_at.get(&offset) {
            if known != incoming && !has_exception_handling {
                return Err(VerifyError::InconsistentStack {
                    at: offset,
                    expected: known,
                    found: incoming,
                });
            }
            continue;
        }
        analysis.depth_at.insert(offset, incoming);

        let ins = &instructions[index];
        let delta = match stack_effect(ins.op) {
            Some((popped, pushed)) => {
                if incoming < popped as i32 {
                    return Err(VerifyError::StackUnderflow { at: offset, op: ins.op.name() });
                }
                pushed as i32 - popped as i32
            }
            None => {
                let delta = stack_delta_for_call(ins);
                if delta == UNRESOLVED_CALL {
                    analysis.has_unresolved_calls = true;
                    0
                } else {
                    delta
                }
            }
        };

        let depth = incoming + delta;
        if depth < 0 {
            return Err(VerifyError::StackUnderflow { at: offset, op: ins.op.name() });
        }
        analysis.max_stack_observed = analysis.max_stack_observed.max(depth as u16);
        // An unresolved call has an unknown effect, so the running depth after
        // it is a guess. Underflow is still sound — an unknown call is assumed
        // to pop nothing, which can only overestimate — but the maximum is not.
        let depth_is_trustworthy = !analysis.has_unresolved_calls;
        if depth_is_trustworthy && declared_max_stack > 0 && depth > declared_max_stack as i32 {
            return Err(VerifyError::StackOverflow {
                at: offset,
                depth,
                max: declared_max_stack,
            });
        }

        // Successors.
        match (&ins.operand, ins.op) {
            (_, Op::Ret | Op::Throw | Op::Rethrow | Op::Endfinally | Op::Jmp) => {}
            (rustclr_core::opcode::Operand::Target(t), Op::Br | Op::BrS | Op::Leave | Op::LeaveS) => {
                // `leave` empties the stack.
                let carried = if matches!(ins.op, Op::Leave | Op::LeaveS) { 0 } else { depth };
                worklist.push((*t, carried));
            }
            (rustclr_core::opcode::Operand::Target(t), _) => {
                worklist.push((*t, depth));
                worklist.push((ins.next_offset(), depth));
            }
            (rustclr_core::opcode::Operand::Targets(ts), _) => {
                for t in ts {
                    worklist.push((*t, depth));
                }
                worklist.push((ins.next_offset(), depth));
            }
            _ => worklist.push((ins.next_offset(), depth)),
        }
    }

    Ok(analysis)
}

/// Total encoded size of an operand kind, for size estimates.
pub fn operand_size(kind: OperandKind) -> usize {
    kind.fixed_size()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_core::opcode::decode_all;

    fn no_calls(_: &Instruction) -> i32 {
        0
    }

    #[test]
    fn a_simple_add_method_analyses_as_a_leaf() {
        // ldarg.0; ldarg.1; add; ret
        let ins = decode_all(&[0x02, 0x03, 0x58, 0x2A]).unwrap();
        let a = analyse(&ins, 8, &[], |i| if i.op == Op::Ret { -1 } else { 0 }).unwrap();
        assert!(a.is_leaf);
        assert!(!a.allocates);
        assert!(!a.has_loops);
        assert_eq!(a.max_stack_observed, 2);
        assert!(a.is_inline_candidate());
        assert!(a.is_compilable_by_baseline_backend());
    }

    #[test]
    fn a_backward_branch_is_reported_as_a_loop() {
        // ldc.i4.0; br.s -3  (jumps back to offset 0)
        let ins = decode_all(&[0x16, 0x26, 0x2B, 0xFC]).unwrap();
        let a = analyse(&ins, 8, &[], no_calls).unwrap();
        assert!(a.has_loops);
        assert!(!a.is_inline_candidate());
    }

    #[test]
    fn a_branch_into_the_middle_of_an_instruction_is_rejected() {
        // ldc.i4 <4-byte imm>; br.s to offset 2, which is inside the immediate.
        let il = [0x20, 0x01, 0x00, 0x00, 0x00, 0x2B, 0xFA];
        let ins = decode_all(&il).unwrap();
        let err = analyse(&ins, 8, &[], no_calls).unwrap_err();
        assert!(matches!(err, VerifyError::BadBranchTarget { .. }), "got {err:?}");
    }

    #[test]
    fn popping_an_empty_stack_is_rejected() {
        let ins = decode_all(&[0x26, 0x2A]).unwrap(); // pop; ret
        let err = analyse(&ins, 8, &[], no_calls).unwrap_err();
        assert!(matches!(err, VerifyError::StackUnderflow { .. }), "got {err:?}");
    }

    #[test]
    fn exceeding_the_declared_max_stack_is_rejected() {
        // Four loads with a declared maximum of two.
        let ins = decode_all(&[0x16, 0x16, 0x16, 0x16, 0x2A]).unwrap();
        let err = analyse(&ins, 2, &[], no_calls).unwrap_err();
        assert!(matches!(err, VerifyError::StackOverflow { .. }), "got {err:?}");
    }

    #[test]
    fn a_catch_handler_does_not_inflate_the_measured_depth() {
        // ldc.i4.0; pop; leave.s +0; (handler) pop; leave.s +0; ret
        let il = [0x16, 0x26, 0xDE, 0x00, 0x26, 0xDE, 0x00, 0x2A];
        let ins = decode_all(&il).unwrap();
        let clauses = [ExceptionClause {
            kind: HandlerKind::Catch(rustclr_core::metadata::Token::NULL),
            try_offset: 0,
            try_length: 4,
            handler_offset: 4,
            handler_length: 3,
        }];

        // Declared maximum of 1: the try body pushes one value and the handler
        // receives one. Seeding every block leader at depth 1 — the bug this
        // guards against — measured 2 and reported a spurious overflow.
        let a = analyse(&ins, 1, &clauses, no_calls).unwrap();
        assert!(a.has_exception_handling);
        assert_eq!(a.max_stack_observed, 1);
    }

    #[test]
    fn block_leaders_include_branch_targets() {
        // ldc.i4.0; brtrue.s +1; nop; ret
        let ins = decode_all(&[0x16, 0x2D, 0x01, 0x00, 0x2A]).unwrap();
        let a = analyse(&ins, 8, &[], no_calls).unwrap();
        assert!(a.block_leaders.contains(&0));
        assert!(a.block_leaders.contains(&4), "the branch target leads a block");
    }
}
