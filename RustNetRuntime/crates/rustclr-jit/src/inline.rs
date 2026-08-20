//! Inlining, so a method that calls a small one can still be compiled.
//!
//! The baseline backends decline any method containing a `call`. That is a
//! sharper limit than it sounds: a body of pure integer arithmetic is refused
//! outright if it factors one line into a helper, which is exactly what
//! well-written code does. Inlining the helper turns such a method back into a
//! leaf, and it is then compiled like any other.
//!
//! This consumes [`MethodAnalysis::is_inline_candidate`], which has existed
//! since the crate did and had no consumer until now.
//!
//! # Branch-free callees, and a full renumber
//!
//! Only callees containing **no branches** are spliced, so nothing inside the
//! inlined body needs its targets fixed up. That is a real restriction — a
//! helper with an `if` in it is not inlined — but it buys a transformation
//! whose correctness is easy to see, which for a compiler is worth more than
//! reach.
//!
//! The *caller's* offsets do have to move, because splicing changes the length
//! of everything after the call site. Rather than compute new byte offsets,
//! the whole stream is renumbered one instruction per offset: instruction `i`
//! sits at offset `i` with length 1. Branch targets are then remapped through
//! a table from each surviving caller instruction's old offset to its new
//! index. This works because every consumer downstream — the stack-depth
//! analysis, the label map, the branch fixups — treats offsets purely as
//! identifiers, never as byte positions.
//!
//! An earlier attempt gave spliced instructions offsets in a high range instead
//! and left the caller's alone. It was wrong: the depth analysis reaches an
//! instruction's successor as `offset + length`, so the splice boundary broke
//! the chain and the depth map came back sparse, which the translator then read
//! as a depth of zero.
//!
//! # Arguments become locals
//!
//! At the call site the arguments are already on the evaluation stack, in
//! order. They are popped into fresh locals — last argument first, since the
//! stack yields them in reverse — and the callee's `ldarg.N` is rewritten to
//! read the local that received argument `N`. The callee's own locals are
//! remapped to fresh ones above those, and its trailing `ret` is dropped: the
//! value it would have returned is simply left on the stack, which is where the
//! caller expects it.

use crate::translate::is_supported;
use rustclr_core::opcode::{decode_all, Instruction, Op, Operand};
use rustclr_core::{Loader, MethodId, MethodKind};
use rustclr_metadata::TypeSig;
use std::collections::HashMap;

/// The result of inlining: a rewritten instruction stream and the locals it
/// needs beyond the caller's own.
pub struct Inlined {
    pub instructions: Vec<Instruction>,
    /// Local slots appended for inlined arguments and bodies.
    pub extra_locals: Vec<TypeSig>,
    /// How many call sites were replaced.
    pub sites: usize,
}

/// Rewrites `instructions`, splicing in every call this can safely inline.
///
/// Returns `None` when nothing was inlined, so a caller can keep the original
/// stream rather than paying for a copy.
pub fn inline_calls(
    loader: &Loader,
    caller: MethodId,
    instructions: &[Instruction],
) -> Option<Inlined> {
    let info = loader.registry.method(caller);
    let MethodKind::Il(body) = &info.kind else { return None };
    let assembly = loader.assembly(info.assembly);

    let mut out: Vec<Instruction> = Vec::with_capacity(instructions.len());
    // `from[i]` is the old offset of the caller instruction that produced
    // `out[i]`, or `None` for an instruction spliced in from a callee. Only
    // the former can be a branch target.
    let mut from: Vec<Option<u32>> = Vec::with_capacity(instructions.len());
    let mut extra_locals: Vec<TypeSig> = Vec::new();
    let mut next_local = body.locals.len();
    let mut sites = 0;

    for ins in instructions {
        let inlinable = if ins.op == Op::Call {
            ins.operand
                .as_token()
                .and_then(|t| loader.resolve_method_token(assembly, t))
                .and_then(|target| candidate(loader, target))
        } else {
            None
        };

        let Some(callee) = inlinable else {
            from.push(Some(ins.offset));
            out.push(ins.clone());
            continue;
        };

        // Fresh locals: one per argument, then one per local the callee has.
        let arg_base = next_local;
        for _ in 0..callee.arg_count {
            extra_locals.push(TypeSig::I8);
            next_local += 1;
        }
        let local_base = next_local;
        for _ in 0..callee.locals {
            extra_locals.push(TypeSig::I8);
            next_local += 1;
        }

        // A branch may target the call itself, so the first instruction that
        // replaces it inherits its offset for the remap table.
        let mut provenance = Some(ins.offset);

        // The stack yields the arguments in reverse, so store the last first.
        for index in (0..callee.arg_count).rev() {
            from.push(provenance.take());
            out.push(spliced(Op::Stloc, Operand::Var((arg_base + index) as u32)));
        }

        for body_ins in &callee.body {
            // The trailing `ret` is dropped: whatever it would have returned is
            // already on the stack, which is where the caller wants it.
            if body_ins.op == Op::Ret {
                continue;
            }
            let (op, operand) = remap(body_ins, arg_base, local_base);
            from.push(provenance.take());
            out.push(spliced(op, operand));
        }

        // A callee of no arguments whose body is a bare `ret` splices to
        // nothing at all, which would strand a branch that targeted the call.
        if let Some(offset) = provenance {
            from.push(Some(offset));
            out.push(spliced(Op::Nop, Operand::None));
        }
        sites += 1;
    }

    if sites == 0 {
        return None;
    }

    renumber(&mut out, &from, instructions);
    Some(Inlined { instructions: out, extra_locals, sites })
}

/// Renumbers the stream one instruction per offset and remaps branch targets.
///
/// Offsets stop meaning byte positions here. Nothing downstream depends on
/// their being byte positions — they are identifiers for `depth_at`, for the
/// label map and for branch fixups — but they must stay dense and ordered,
/// because the depth analysis walks to a successor as `offset + length`.
fn renumber(out: &mut [Instruction], from: &[Option<u32>], original: &[Instruction]) {
    let mut map: HashMap<u32, u32> = HashMap::new();
    for (index, old) in from.iter().enumerate() {
        if let Some(old) = old {
            // `entry` rather than `insert`: the first output instruction for a
            // given old offset is the one a branch should land on.
            map.entry(*old).or_insert(index as u32);
        }
    }
    // A `leave` or a forward branch may target the byte just past the last
    // instruction, which is the method's end rather than any instruction.
    let old_end = original.last().map_or(0, |i| i.next_offset());
    map.insert(old_end, out.len() as u32);

    for (index, ins) in out.iter_mut().enumerate() {
        ins.offset = index as u32;
        ins.length = 1;
        match &mut ins.operand {
            Operand::Target(t) => {
                if let Some(&new) = map.get(t) {
                    *t = new;
                }
            }
            Operand::Targets(ts) => {
                for t in ts.iter_mut() {
                    if let Some(&new) = map.get(t) {
                        *t = new;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Builds a spliced instruction. Its offset is assigned by [`renumber`].
fn spliced(op: Op, operand: Operand) -> Instruction {
    Instruction { offset: 0, length: 1, op, operand }
}

/// A callee that may be spliced, decoded and checked.
struct Candidate {
    body: Vec<Instruction>,
    arg_count: usize,
    locals: usize,
}

/// Whether a method can be inlined here, and its decoded body if so.
fn candidate(loader: &Loader, method: MethodId) -> Option<Candidate> {
    let info = loader.registry.method(method);
    let MethodKind::Il(body) = &info.kind else { return None };

    // Static only: an instance call would need `this` threaded through as well,
    // and the backends decline instance methods anyway.
    if info.signature.has_this || !body.exception_clauses.is_empty() {
        return None;
    }
    if !crate::translate::shape_is_compilable(&loader.registry, method) {
        return None;
    }

    let instructions = decode_all(&body.il).ok()?;

    // Small, loop-free and exception-free. Measured, not assumed: the branch
    // check below would catch a loop anyway, but a fabricated analysis would
    // make `is_inline_candidate` answer a question it was not asked.
    let returns_value = !info.returns_void();
    let analysis = crate::verify::analyse(
        &instructions,
        body.max_stack,
        &body.exception_clauses,
        |ins| if ins.op == Op::Ret && returns_value { -1 } else { 0 },
    )
    .ok()?;
    if !analysis.is_inline_candidate() {
        return None;
    }

    // Branch-free, so nothing inside needs its offsets renumbered and nothing
    // outside can name an instruction within it.
    if instructions.iter().any(|i| is_branch(i.op) || !is_supported(i.op)) {
        return None;
    }
    // Exactly one `ret`, and it must be last, or dropping it would change the
    // control flow rather than just the frame.
    match instructions.last() {
        Some(last) if last.op == Op::Ret => {}
        _ => return None,
    }
    if instructions.iter().filter(|i| i.op == Op::Ret).count() != 1 {
        return None;
    }

    Some(Candidate {
        body: instructions,
        arg_count: info.signature.params.len(),
        locals: body.locals.len(),
    })
}

fn is_branch(op: Op) -> bool {
    use Op::*;
    matches!(
        op,
        Br | BrS
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
            | Switch
            | Leave
            | LeaveS
    )
}

/// Rewrites one callee instruction into the caller's local numbering.
fn remap(ins: &Instruction, arg_base: usize, local_base: usize) -> (Op, Operand) {
    use Op::*;
    let var = |n: usize| Operand::Var(n as u32);
    match ins.op {
        Ldarg0 => (Ldloc, var(arg_base)),
        Ldarg1 => (Ldloc, var(arg_base + 1)),
        Ldarg2 => (Ldloc, var(arg_base + 2)),
        Ldarg3 => (Ldloc, var(arg_base + 3)),
        LdargS | Ldarg => (Ldloc, var(arg_base + ins.operand.as_var().unwrap_or(0) as usize)),
        StargS | Starg => (Stloc, var(arg_base + ins.operand.as_var().unwrap_or(0) as usize)),

        Ldloc0 => (Ldloc, var(local_base)),
        Ldloc1 => (Ldloc, var(local_base + 1)),
        Ldloc2 => (Ldloc, var(local_base + 2)),
        Ldloc3 => (Ldloc, var(local_base + 3)),
        LdlocS | Ldloc => (Ldloc, var(local_base + ins.operand.as_var().unwrap_or(0) as usize)),

        Stloc0 => (Stloc, var(local_base)),
        Stloc1 => (Stloc, var(local_base + 1)),
        Stloc2 => (Stloc, var(local_base + 2)),
        Stloc3 => (Stloc, var(local_base + 3)),
        StlocS | Stloc => (Stloc, var(local_base + ins.operand.as_var().unwrap_or(0) as usize)),

        other => (other, ins.operand.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(offset: u32, length: u32, op: Op, operand: Operand) -> Instruction {
        Instruction { offset, length, op, operand }
    }

    #[test]
    fn renumbering_keeps_offsets_dense_and_contiguous() {
        // The depth analysis reaches a successor as `offset + length`, so a gap
        // anywhere in the stream silently truncates the walk.
        let original = vec![at(0, 1, Op::LdcI40, Operand::None), at(1, 5, Op::Ret, Operand::None)];
        let mut out = vec![
            spliced(Op::LdcI40, Operand::None),
            spliced(Op::Nop, Operand::None),
            spliced(Op::Ret, Operand::None),
        ];
        renumber(&mut out, &[Some(0), None, Some(1)], &original);
        for (i, ins) in out.iter().enumerate() {
            assert_eq!(ins.offset, i as u32);
            assert_eq!(ins.next_offset(), i as u32 + 1);
        }
    }

    #[test]
    fn a_branch_over_a_splice_still_lands_on_its_target() {
        // Caller: `br 4; ...; ret@4`. Splicing an extra instruction before the
        // target moves it, and the branch has to move with it.
        let original = vec![
            at(0, 2, Op::BrS, Operand::Target(4)),
            at(2, 1, Op::LdcI40, Operand::None),
            at(4, 1, Op::Ret, Operand::None),
        ];
        let mut out = vec![
            at(0, 2, Op::BrS, Operand::Target(4)),
            at(2, 1, Op::LdcI40, Operand::None),
            spliced(Op::Nop, Operand::None),
            at(4, 1, Op::Ret, Operand::None),
        ];
        renumber(&mut out, &[Some(0), Some(2), None, Some(4)], &original);
        assert_eq!(out[0].operand, Operand::Target(3), "the branch must follow its target");
    }

    #[test]
    fn a_branch_to_the_end_of_the_method_is_remapped_too() {
        // Targeting one past the last instruction is legal and means "fall out
        // of the method"; it is not the offset of any instruction.
        let original = vec![at(0, 2, Op::BrS, Operand::Target(3)), at(2, 1, Op::Ret, Operand::None)];
        let mut out = vec![
            at(0, 2, Op::BrS, Operand::Target(3)),
            spliced(Op::Nop, Operand::None),
            at(2, 1, Op::Ret, Operand::None),
        ];
        renumber(&mut out, &[Some(0), None, Some(2)], &original);
        assert_eq!(out[0].operand, Operand::Target(3), "end-of-method is the new length");
    }

    #[test]
    fn branches_are_recognised_so_they_are_never_spliced() {
        assert!(is_branch(Op::BrS));
        assert!(is_branch(Op::BltS));
        assert!(is_branch(Op::Switch));
        assert!(!is_branch(Op::Add));
        assert!(!is_branch(Op::Ret));
    }

    #[test]
    fn arguments_and_locals_are_remapped_to_distinct_slots() {
        let ins = Instruction { offset: 0, length: 1, op: Op::Ldarg1, operand: Operand::None };
        assert_eq!(remap(&ins, 4, 9), (Op::Ldloc, Operand::Var(5)));

        let ins = Instruction { offset: 0, length: 1, op: Op::Stloc2, operand: Operand::None };
        assert_eq!(remap(&ins, 4, 9), (Op::Stloc, Operand::Var(11)));

        // Anything else passes through untouched.
        let ins = Instruction { offset: 0, length: 1, op: Op::Mul, operand: Operand::None };
        assert_eq!(remap(&ins, 4, 9), (Op::Mul, Operand::None));
    }
}
