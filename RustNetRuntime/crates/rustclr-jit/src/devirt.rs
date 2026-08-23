//! Guarded devirtualisation: a `callvirt` whose every answer is known.
//!
//! A virtual call cannot be resolved at compile time in general — that is the
//! point of one. But it can be resolved *by cases* when two things hold: the
//! set of types that could reach the call site is known, and each of their
//! overrides is simple enough to emit inline. Then the call becomes a compare
//! against the receiver's runtime type and a branch, with no call at all.
//!
//! # Why the case list can be trusted
//!
//! Class-hierarchy analysis over the loaded types is normally a *guess*: this
//! runtime can `Assembly.Load` more types later, and one of them might override
//! the method. So the analysis is only used where that is impossible — where
//! the declaring type is not visible outside its own assembly, and therefore
//! cannot be derived from outside it. A `private` nested class is the common
//! case, and the check is [`RuntimeType::externally_extensible`].
//!
//! Get this wrong and a receiver arrives whose type matches no guard. There is
//! no on-stack replacement here and no way back to the interpreter from a body
//! that has already allocated, so the fall-through **traps** rather than
//! guessing — a proof that turned out to be wrong reports itself instead of
//! returning a number nobody can account for.
//!
//! # What an override has to look like
//!
//! Each one must fold to a constant: a branch-free body that reads neither
//! `this` nor any argument and ends in `ret`. `int Sides() => 3` is the shape,
//! and it is a real one — a virtual constant, a discriminator, a tag. Anything
//! else is declined, because emitting it would mean either inlining an
//! arbitrary body per branch or making a real call, and a general calling
//! convention between compiled methods does not exist yet.
//!
//! That is a narrow capability and it is described narrowly on purpose. It is
//! not "the backend compiles virtual calls".

use rustclr_core::opcode::{Instruction, Op};
use rustclr_core::{Loader, MethodId, TypeId, TypeKind};
use std::collections::BTreeMap;

/// How many overrides a guard chain is willing to test.
///
/// A linear chain of compares beats a jump table at this size, and a call site
/// with more implementations than this is polymorphic enough that guarding it
/// would cost more than it saves.
const MAX_CANDIDATES: usize = 4;

/// The cases for one `callvirt`: a type's registry index, and the constant its
/// override produces.
pub type Cases = Vec<(i64, i64)>;

/// Plans a guarded dispatch for every `callvirt` that qualifies.
///
/// Returns the offsets it can handle. A `callvirt` missing from the map is not
/// an error here — the shape check declines the method later if it cannot be
/// compiled another way.
pub fn plan(
    loader: &Loader,
    method: MethodId,
    instructions: &[Instruction],
) -> BTreeMap<u32, Cases> {
    let info = loader.registry.method(method);
    let assembly = loader.assembly(info.assembly);
    let mut plans = BTreeMap::new();

    for ins in instructions {
        if ins.op != Op::Callvirt {
            continue;
        }
        let Some(token) = ins.operand.as_token() else { continue };
        let Some(declared) = loader.resolve_method_token(assembly, token) else { continue };
        if let Some(cases) = cases_for(loader, declared) {
            plans.insert(ins.offset, cases);
        }
    }
    plans
}

/// The complete set of answers `declared` can give, or `None` when that set
/// cannot be known or cannot be emitted.
fn cases_for(loader: &Loader, declared: MethodId) -> Option<Cases> {
    let info = loader.registry.method(declared);
    // A receiver and nothing else, returning a value. An override taking
    // arguments would need them marshalled per branch, which is the general
    // calling-convention problem this deliberately avoids.
    if !info.signature.has_this || info.arg_count() != 1 || info.returns_void() {
        return None;
    }
    let slot = info.vtable_slot?;
    let declaring = loader.registry.ty(info.declaring_type);

    // Interfaces are excluded: dispatch through one matches by name and shape
    // rather than by slot, and any type at all may implement one.
    if declaring.kind != TypeKind::Class {
        return None;
    }
    // The proof obligation. Without this, a type loaded later could override
    // the method and arrive at a call site whose guards do not mention it.
    if declaring.externally_extensible {
        return None;
    }

    let root = info.declaring_type;
    let mut cases: Cases = Vec::new();
    for candidate in loader.registry.iter_types() {
        if candidate.kind != TypeKind::Class || candidate.is_abstract {
            continue;
        }
        if !derives_from(loader, candidate.id, root) {
            continue;
        }
        // Every concrete subtype must be accounted for, so a candidate whose
        // override cannot be folded fails the whole site rather than being
        // quietly left out of the chain.
        let target = *candidate.vtable.get(slot as usize)?;
        let value = constant_result(loader, target)?;
        cases.push((candidate.id.index() as i64, value));
        if cases.len() > MAX_CANDIDATES {
            return None;
        }
    }
    // One case is not a guard, it is a certainty — but it still has to be
    // tested, because the receiver could be null. Zero cases means no concrete
    // implementation exists and the call could only ever throw.
    if cases.is_empty() {
        return None;
    }
    Some(cases)
}

/// Whether `ty` is `root` or descends from it.
fn derives_from(loader: &Loader, ty: TypeId, root: TypeId) -> bool {
    let mut current = Some(ty);
    // Bounded by the hierarchy depth; a cycle would mean corrupt metadata, and
    // the bound stops it becoming a hang.
    for _ in 0..64 {
        let Some(id) = current else { return false };
        if id == root {
            return true;
        }
        current = loader.registry.ty(id).base;
    }
    false
}

/// The constant a method always returns, when it always returns one.
///
/// Deliberately tiny: push a literal, maybe widen it, return. Constant folding
/// over arithmetic would be a different feature, and every case it added would
/// be one more thing to be wrong about in code that has no way to report being
/// wrong.
fn constant_result(loader: &Loader, method: MethodId) -> Option<i64> {
    let info = loader.registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else {
        return None;
    };
    if !body.exception_clauses.is_empty() {
        return None;
    }
    let instructions = rustclr_core::opcode::decode_all(&body.il).ok()?;
    let mut value: Option<i64> = None;
    for ins in &instructions {
        use Op::*;
        match ins.op {
            Nop => {}
            LdcI4M1 => value = Some(-1),
            LdcI40 => value = Some(0),
            LdcI41 => value = Some(1),
            LdcI42 => value = Some(2),
            LdcI43 => value = Some(3),
            LdcI44 => value = Some(4),
            LdcI45 => value = Some(5),
            LdcI46 => value = Some(6),
            LdcI47 => value = Some(7),
            LdcI48 => value = Some(8),
            LdcI4S | LdcI4 => value = Some(ins.operand.as_i32()? as i64),
            LdcI8 => {
                value = Some(match ins.operand {
                    rustclr_core::opcode::Operand::I64(v) => v,
                    _ => return None,
                })
            }
            // Widening a literal that is already an `i64` in this
            // representation changes nothing.
            ConvI8 | ConvI4 => {
                value?;
            }
            Ret => return value,
            _ => return None,
        }
    }
    None
}
