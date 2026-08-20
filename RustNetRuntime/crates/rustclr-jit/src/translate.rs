//! The architecture-independent half of code generation.
//!
//! Three backends emit very different bytes, but they all do the same *walk*:
//! IL is a stack machine, the verifier already knows the evaluation-stack depth
//! at every instruction, so each stack slot has a statically known home and
//! every opcode becomes "read these slots, compute, write that slot".
//!
//! Only the encoding differs. Keeping the walk here means the x86-64 backend —
//! the one this host can actually execute, and therefore the one the
//! differential test proves — exercises the same translation the AArch64 and
//! RISC-V backends use. A bug in the walk fails on x86-64 before it can ship
//! silently on an architecture nobody here can run.
//!
//! # The slot model
//!
//! Evaluation-stack slots live in registers where a backend has spare ones and
//! spill to the frame beyond that. Locals and spilled slots are frame offsets;
//! arguments arrive as a pointer to an `i64` array, which is one incoming
//! parameter and therefore the same shape under every calling convention.

use crate::verify::MethodAnalysis;
use crate::CompileError;
use rustclr_core::opcode::{Instruction, Op, Operand};

/// A two-operand integer operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    /// Arithmetic right shift.
    Shr,
    /// Logical right shift.
    ShrUn,
}

/// A one-operand integer operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// Truncate to 32 bits and sign-extend back, which is what `conv.i4` means
    /// for the evaluation stack.
    SignExtend32,
}

/// A comparison, used both for `ceq`-style materialisation and for branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    /// Unsigned forms.
    Below,
    BelowOrEqual,
    Above,
    AboveOrEqual,
}

/// What a backend must be able to emit.
///
/// Register numbers are the backend's own; the walk only ever passes back what
/// [`Self::temp`] and [`Self::cached_slot_register`] handed it.
pub trait Backend {
    /// Scratch registers, at least three, that the walk may clobber freely.
    fn temp(&self, which: usize) -> u8;

    /// The register holding evaluation-stack slot `depth`, if this backend
    /// keeps that depth in one. `None` means the slot lives in the frame.
    fn cached_slot_register(&self, depth: usize) -> Option<u8>;

    /// Frame offset of local `index`.
    fn local_offset(&self, index: usize) -> i32;

    /// Frame offset of spilled evaluation-stack slot `depth`.
    fn spill_offset(&self, depth: usize) -> i32;

    fn prologue(&mut self, locals: usize, max_stack: usize);
    fn epilogue(&mut self);

    fn mov_reg(&mut self, dst: u8, src: u8);
    fn load_imm(&mut self, dst: u8, value: i64);
    fn load_frame(&mut self, dst: u8, offset: i32);
    fn store_frame(&mut self, offset: i32, src: u8);
    /// Read argument `index` from the incoming array.
    fn load_arg(&mut self, dst: u8, index: usize);
    /// Write argument `index` back to the incoming array, for `starg`.
    fn store_arg(&mut self, index: usize, src: u8);

    fn binop(&mut self, op: BinOp, dst: u8, lhs: u8, rhs: u8);
    fn unop(&mut self, op: UnOp, dst: u8, src: u8);
    /// Materialise `lhs <cond> rhs` as 0 or 1 in `dst`.
    fn compare(&mut self, cond: Cond, dst: u8, lhs: u8, rhs: u8);

    fn branch(&mut self, target: u32);
    fn branch_compare(&mut self, cond: Cond, lhs: u8, rhs: u8, target: u32);
    /// Branch when `src` is zero (`brfalse`) or non-zero (`brtrue`).
    fn branch_zero(&mut self, src: u8, non_zero: bool, target: u32);

    /// Return, with the value already in the register the ABI wants.
    fn ret(&mut self, src: u8);
    /// Return zero, for a void method.
    fn ret_void(&mut self);

    /// Record that `il_offset` begins here, so branches can be resolved.
    fn label(&mut self, il_offset: u32);
}

/// Whether an opcode the walk understands is one this crate emits code for.
pub fn is_supported(op: Op) -> bool {
    use Op::*;
    matches!(
        op,
        Nop | Ldarg0
            | Ldarg1
            | Ldarg2
            | Ldarg3
            | LdargS
            | Ldarg
            | StargS
            | Starg
            | Ldloc0
            | Ldloc1
            | Ldloc2
            | Ldloc3
            | LdlocS
            | Ldloc
            | Stloc0
            | Stloc1
            | Stloc2
            | Stloc3
            | StlocS
            | Stloc
            | LdcI4M1
            | LdcI40
            | LdcI41
            | LdcI42
            | LdcI43
            | LdcI44
            | LdcI45
            | LdcI46
            | LdcI47
            | LdcI48
            | LdcI4S
            | LdcI4
            | LdcI8
            | Add
            | Sub
            | Mul
            | Div
            | Rem
            | And
            | Or
            | Xor
            | Shl
            | Shr
            | ShrUn
            | Neg
            | Not
            | Dup
            | Pop
            | Ceq
            | Cgt
            | CgtUn
            | Clt
            | CltUn
            | Br
            | BrS
            | Brtrue
            | BrtrueS
            | Brfalse
            | BrfalseS
            | Beq
            | BeqS
            | BneUn
            | BneUnS
            | Bge
            | BgeS
            | Bgt
            | BgtS
            | Ble
            | BleS
            | Blt
            | BltS
            | BgeUn
            | BgeUnS
            | BgtUn
            | BgtUnS
            | BleUn
            | BleUnS
            | BltUn
            | BltUnS
            | ConvI4
            | ConvI8
            | ConvI
            | Ret
    )
}

/// Reads evaluation-stack slot `depth` into `dst`.
fn read_slot<B: Backend>(b: &mut B, dst: u8, depth: usize) {
    match b.cached_slot_register(depth) {
        Some(r) => b.mov_reg(dst, r),
        None => {
            let off = b.spill_offset(depth);
            b.load_frame(dst, off)
        }
    }
}

/// Writes `src` into evaluation-stack slot `depth`.
fn write_slot<B: Backend>(b: &mut B, depth: usize, src: u8) {
    match b.cached_slot_register(depth) {
        Some(r) => b.mov_reg(r, src),
        None => {
            let off = b.spill_offset(depth);
            b.store_frame(off, src)
        }
    }
}

/// Translates a verified method body by driving a backend.
///
/// `analysis` supplies the evaluation-stack depth at every instruction, which
/// is what makes the stack machine addressable without simulating it.
pub fn translate<B: Backend>(
    b: &mut B,
    instructions: &[Instruction],
    analysis: &MethodAnalysis,
    locals: usize,
    returns_value: bool,
) -> Result<(), CompileError> {
    b.prologue(locals, analysis.max_stack_observed as usize);

    for ins in instructions {
        b.label(ins.offset);
        let depth = *analysis.depth_at.get(&ins.offset).unwrap_or(&0);
        if depth < 0 {
            return Err(CompileError::Unsupported(
                "negative evaluation-stack depth".into(),
            ));
        }
        one(b, ins, depth as usize, returns_value)?;
    }

    // A body that falls off the end without `ret` is invalid IL, but emitting
    // a return keeps the page well-formed rather than running into whatever
    // follows it in memory.
    b.ret_void();
    Ok(())
}

fn one<B: Backend>(
    b: &mut B,
    ins: &Instruction,
    depth: usize,
    returns_value: bool,
) -> Result<(), CompileError> {
    use Op::*;

    let t0 = b.temp(0);
    let t1 = b.temp(1);

    match ins.op {
        Nop => {}

        Ldarg0 | Ldarg1 | Ldarg2 | Ldarg3 | LdargS | Ldarg => {
            let index = match ins.op {
                Ldarg0 => 0,
                Ldarg1 => 1,
                Ldarg2 => 2,
                Ldarg3 => 3,
                _ => ins.operand.as_var().unwrap_or(0) as usize,
            };
            b.load_arg(t0, index);
            write_slot(b, depth, t0);
        }

        // `starg` assigns to a parameter. The argument array is the callee's
        // own copy, so writing to it has exactly the local effect C# gives an
        // assignment to a parameter.
        StargS | Starg => {
            let index = ins.operand.as_var().unwrap_or(0) as usize;
            read_slot(b, t0, depth - 1);
            b.store_arg(index, t0);
        }

        Ldloc0 | Ldloc1 | Ldloc2 | Ldloc3 | LdlocS | Ldloc => {
            let index = match ins.op {
                Ldloc0 => 0,
                Ldloc1 => 1,
                Ldloc2 => 2,
                Ldloc3 => 3,
                _ => ins.operand.as_var().unwrap_or(0) as usize,
            };
            let off = b.local_offset(index);
            b.load_frame(t0, off);
            write_slot(b, depth, t0);
        }

        Stloc0 | Stloc1 | Stloc2 | Stloc3 | StlocS | Stloc => {
            let index = match ins.op {
                Stloc0 => 0,
                Stloc1 => 1,
                Stloc2 => 2,
                Stloc3 => 3,
                _ => ins.operand.as_var().unwrap_or(0) as usize,
            };
            read_slot(b, t0, depth - 1);
            let off = b.local_offset(index);
            b.store_frame(off, t0);
        }

        LdcI4M1 | LdcI40 | LdcI41 | LdcI42 | LdcI43 | LdcI44 | LdcI45 | LdcI46 | LdcI47
        | LdcI48 | LdcI4S | LdcI4 => {
            let value = match ins.op {
                LdcI4M1 => -1,
                LdcI40 => 0,
                LdcI41 => 1,
                LdcI42 => 2,
                LdcI43 => 3,
                LdcI44 => 4,
                LdcI45 => 5,
                LdcI46 => 6,
                LdcI47 => 7,
                LdcI48 => 8,
                _ => ins.operand.as_i32().unwrap_or(0),
            };
            b.load_imm(t0, value as i64);
            write_slot(b, depth, t0);
        }

        LdcI8 => {
            let Operand::I64(v) = ins.operand else {
                return Err(CompileError::Unsupported("ldc.i8 without an operand".into()));
            };
            b.load_imm(t0, v);
            write_slot(b, depth, t0);
        }

        Add | Sub | Mul | Div | Rem | And | Or | Xor | Shl | Shr | ShrUn => {
            let op = match ins.op {
                Add => BinOp::Add,
                Sub => BinOp::Sub,
                Mul => BinOp::Mul,
                Div => BinOp::Div,
                Rem => BinOp::Rem,
                And => BinOp::And,
                Or => BinOp::Or,
                Xor => BinOp::Xor,
                Shl => BinOp::Shl,
                Shr => BinOp::Shr,
                _ => BinOp::ShrUn,
            };
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.binop(op, t0, t0, t1);
            write_slot(b, depth - 2, t0);
        }

        Neg | Not | ConvI4 => {
            let op = match ins.op {
                Neg => UnOp::Neg,
                Not => UnOp::Not,
                _ => UnOp::SignExtend32,
            };
            read_slot(b, t0, depth - 1);
            b.unop(op, t0, t0);
            write_slot(b, depth - 1, t0);
        }

        // Already 64-bit on the evaluation stack.
        ConvI8 | ConvI => {}

        Dup => {
            read_slot(b, t0, depth - 1);
            write_slot(b, depth, t0);
        }

        Pop => {}

        Ceq | Cgt | CgtUn | Clt | CltUn => {
            let cond = match ins.op {
                Ceq => Cond::Equal,
                Cgt => Cond::Greater,
                CgtUn => Cond::Above,
                Clt => Cond::Less,
                _ => Cond::Below,
            };
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.compare(cond, t0, t0, t1);
            write_slot(b, depth - 2, t0);
        }

        Br | BrS => {
            let target = branch_target(ins)?;
            b.branch(target);
        }

        Brtrue | BrtrueS | Brfalse | BrfalseS => {
            let target = branch_target(ins)?;
            read_slot(b, t0, depth - 1);
            b.branch_zero(t0, matches!(ins.op, Brtrue | BrtrueS), target);
        }

        Beq | BeqS | BneUn | BneUnS | Bge | BgeS | Bgt | BgtS | Ble | BleS | Blt | BltS
        | BgeUn | BgeUnS | BgtUn | BgtUnS | BleUn | BleUnS | BltUn | BltUnS => {
            let target = branch_target(ins)?;
            // The `.un` forms are unsigned comparisons, except `bne.un`, which
            // is plain inequality — there is no ordering to interpret.
            let cond = match ins.op {
                Beq | BeqS => Cond::Equal,
                BneUn | BneUnS => Cond::NotEqual,
                Bge | BgeS => Cond::GreaterOrEqual,
                Bgt | BgtS => Cond::Greater,
                Ble | BleS => Cond::LessOrEqual,
                Blt | BltS => Cond::Less,
                BgeUn | BgeUnS => Cond::AboveOrEqual,
                BgtUn | BgtUnS => Cond::Above,
                BleUn | BleUnS => Cond::BelowOrEqual,
                _ => Cond::Below,
            };
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.branch_compare(cond, t0, t1, target);
        }

        Ret => {
            if returns_value {
                read_slot(b, t0, depth - 1);
                b.ret(t0);
            } else {
                b.ret_void();
            }
        }

        other => return Err(CompileError::Unsupported(format!("{other:?}"))),
    }
    Ok(())
}

fn branch_target(ins: &Instruction) -> Result<u32, CompileError> {
    ins.operand
        .as_target()
        .ok_or_else(|| CompileError::Unsupported("branch without a target".into()))
}

/// Resolves recorded branch fixups into relative displacements.
///
/// Shared because every backend patches the same way: a list of
/// `(code offset, IL target)` and a map from IL offset to code offset. Only the
/// width and the encoding of the displacement differ, which is what `patch`
/// supplies.
pub fn resolve_branches(
    code: &mut [u8],
    fixups: &[(usize, u32)],
    labels: &std::collections::HashMap<u32, usize>,
    mut patch: impl FnMut(&mut [u8], usize, isize) -> Result<(), CompileError>,
) -> Result<(), CompileError> {
    for (at, target) in fixups {
        let Some(&destination) = labels.get(target) else {
            return Err(CompileError::Unsupported(format!(
                "branch to IL offset {target:#x}, which is not an instruction boundary"
            )));
        };
        patch(code, *at, destination as isize - *at as isize)?;
    }
    Ok(())
}
