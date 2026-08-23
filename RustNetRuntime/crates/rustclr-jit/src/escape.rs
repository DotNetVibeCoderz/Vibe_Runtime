//! Scalar replacement: an object that never escapes becomes its fields.
//!
//! The backend declines `ldfld` and `stfld`, which is why the `fields` and
//! `virtual` benchmarks are the slowest rows in the table. The obvious fix is
//! to teach compiled code to read a field out of a heap object — and it is the
//! wrong one. Fields live in a `Vec<Value>`, and `Value` is a Rust enum with
//! boxed payloads whose layout the language does not promise. Emitting machine
//! code that reads it would make that layout part of this backend's ABI: a
//! future compiler is free to reorder the variants, and the failure would be a
//! wrong number rather than a crash.
//!
//! So this takes the other road. When an object provably never leaves the
//! method that made it, it does not need to exist at all: each of its fields
//! becomes an ordinary local, `ldfld` becomes `ldloc`, and `stfld` becomes
//! `stloc`. No layout is assumed, no reference is ever held by compiled code,
//! and the result is faster than field access would have been, because the
//! fields end up in the same frame slots as any other local.
//!
//! # What "never escapes" has to mean
//!
//! Being wrong here is silent, so the analysis refuses everything it cannot
//! follow. An object qualifies only when *every* one of these holds:
//!
//! - It is created by a `newobj` that runs **at most once** — not inside a
//!   loop — because the replacement locals are zeroed by the prologue and
//!   never again.
//! - Its constructor is **trivial**: `ldarg.0; call object::.ctor; ret` and
//!   nothing else. A constructor that does work would have to be inlined
//!   first, and this pass does not do that.
//! - Its type derives directly from `System.Object` and every instance field
//!   is a scalar the backend already handles. Inherited or reference-typed
//!   fields are refused rather than partially handled.
//! - The reference is stored to exactly one local, and **every** use of that
//!   local is the target of a `ldfld`, a `stfld` or a `dup` feeding one. Being
//!   passed to a call, returned, boxed, compared, locked, or having its
//!   address taken all disqualify it.
//! - The reference never crosses a branch: no object-typed slot is live on the
//!   evaluation stack at a branch or a branch target. This is what lets the
//!   walk below be a single linear pass instead of a fixed point, and it costs
//!   nothing on the shape this exists for, where `o.F++` begins and ends
//!   inside one basic block.
//!
//! Anything else returns `None`, and the method is compiled as it was — or
//! declined, exactly as before.

use rustclr_core::opcode::{Instruction, Op, Operand};
use rustclr_core::{FieldId, Loader, MethodId, TypeKind};
use rustclr_metadata::TypeSig;
use std::collections::{BTreeSet, HashMap};

/// The result of scalarising: a rewritten stream and the locals it needs.
pub struct Scalarised {
    pub instructions: Vec<Instruction>,
    /// Local slots appended, one per field of each replaced object.
    pub extra_locals: Vec<TypeSig>,
    /// How many objects stopped existing.
    pub objects: usize,
    /// Locals that held those objects and are now dead.
    ///
    /// Their declared type is still a class, which every shape gate refuses.
    /// Nothing reads or writes them any more, so the caller substitutes a
    /// scalar rather than teaching each gate about liveness.
    pub dead_locals: Vec<u32>,
}

/// One slot of the abstract evaluation stack.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// A reference to the object being scalarised, and which one.
    Object(u32),
    /// Anything else. Also what a pop from below the block's own region
    /// returns, which is sound because no object may cross a block edge.
    Other,
}

/// What to do with an instruction when the rewrite is applied.
#[derive(Clone, Copy)]
enum Action {
    /// The instruction disappears: it only ever moved a reference around.
    Elide,
    /// `ldfld` becomes a read of this local.
    Load(u32),
    /// `stfld` becomes a write of this local.
    Store(u32),
}

/// Rewrites `instructions`, replacing every non-escaping object with locals.
///
/// Returns `None` when there is nothing to replace or the proof fails, so the
/// caller keeps the original stream rather than paying for a copy.
pub fn scalarise(
    loader: &Loader,
    method: MethodId,
    instructions: &[Instruction],
    local_count: usize,
) -> Option<Scalarised> {
    let info = loader.registry.method(method);
    let assembly = loader.assembly(info.assembly);

    // ── Find the candidates ────────────────────────────────────────────────
    //
    // A candidate is a `newobj` of a scalarisable type, stored straight into a
    // local that nothing else ever writes.
    let mut extra_locals: Vec<TypeSig> = Vec::new();
    // Local holding the object -> its fields' synthetic local indices.
    let mut replaced: HashMap<u32, Vec<u32>> = HashMap::new();
    // Offset of each `newobj` and of the `stloc` that consumes it.
    let mut creation_sites: Vec<(u32, u32)> = Vec::new();

    for (i, ins) in instructions.iter().enumerate() {
        if ins.op != Op::Newobj {
            continue;
        }
        // The replacement locals are zeroed once, by the prologue. A `newobj`
        // that runs again would need them zeroed again.
        if crate::translate::inside_a_loop(instructions, ins.offset) {
            continue;
        }
        let Some(token) = ins.operand.as_token() else { continue };
        let Some(ctor) = loader.resolve_method_token(assembly, token) else { continue };
        let Some(fields) = scalarisable_fields(loader, ctor) else { continue };

        // The reference has to go straight into a local, or the analysis below
        // would have to track it across statements.
        let Some(next) = instructions.get(i + 1) else { continue };
        let Some(local) = store_target(next) else { continue };
        if replaced.contains_key(&local) {
            continue;
        }
        // Exactly one store to that local: this one. A second store would mean
        // the local sometimes holds an object this pass did not prove.
        if instructions.iter().filter(|o| store_target(o) == Some(local)).count() != 1 {
            continue;
        }

        let base = (local_count + extra_locals.len()) as u32;
        let mut slots = Vec::with_capacity(fields.len());
        for (n, field) in fields.iter().enumerate() {
            extra_locals.push(loader.registry.field(*field).signature.clone());
            slots.push(base + n as u32);
        }
        replaced.insert(local, slots);
        creation_sites.push((ins.offset, next.offset));
    }

    if replaced.is_empty() {
        return None;
    }

    // ── Prove it, and record the rewrite ───────────────────────────────────
    let mut actions: HashMap<u32, Action> = HashMap::new();
    let creations: HashMap<u32, u32> = creation_sites
        .iter()
        .map(|(newobj, stloc)| {
            let local = store_target(instructions.iter().find(|i| i.offset == *stloc)?)?;
            Some((*newobj, local))
        })
        .collect::<Option<HashMap<u32, u32>>>()?;
    for (newobj, stloc) in &creation_sites {
        actions.insert(*newobj, Action::Elide);
        actions.insert(*stloc, Action::Elide);
    }

    let leaders = branch_targets(instructions);
    let mut stack: Vec<Slot> = Vec::new();

    for ins in instructions {
        // At a block leader nothing object-typed may still be live, so the
        // region below is all `Other` and the walk can start fresh.
        if leaders.contains(&ins.offset) {
            stack.clear();
        }

        // Creating the object pushes it; its store consumes it.
        if let Some(local) = creations.get(&ins.offset) {
            stack.push(Slot::Object(*local));
            continue;
        }
        if let Some(local) = store_target(ins) {
            if replaced.contains_key(&local) {
                if pop(&mut stack) != Slot::Object(local) {
                    return None;
                }
                continue;
            }
        }
        if let Some(local) = load_source(ins) {
            if replaced.contains_key(&local) {
                actions.insert(ins.offset, Action::Elide);
                stack.push(Slot::Object(local));
                continue;
            }
        }
        // Taking the address of a replaced local defeats the whole thing.
        if matches!(ins.op, Op::LdlocaS | Op::Ldloca) {
            if let Some(local) = ins.operand.as_var() {
                if replaced.contains_key(&local) {
                    return None;
                }
            }
        }

        match ins.op {
            Op::Dup => {
                let top = *stack.last().unwrap_or(&Slot::Other);
                if matches!(top, Slot::Object(_)) {
                    actions.insert(ins.offset, Action::Elide);
                }
                stack.push(top);
            }

            Op::Ldfld | Op::Stfld | Op::Ldflda => {
                // `stfld` pops the value first, then the reference.
                if ins.op == Op::Stfld && matches!(pop(&mut stack), Slot::Object(_)) {
                    return None;
                }
                let Slot::Object(local) = pop(&mut stack) else {
                    // A field of some object this pass did not replace. The
                    // shape check declines the method later; leave it alone,
                    // but keep the abstract stack honest.
                    if ins.op != Op::Stfld {
                        stack.push(Slot::Other);
                    }
                    continue;
                };
                // `ldflda` would hand out a pointer to something that is about
                // to stop existing.
                if ins.op == Op::Ldflda {
                    return None;
                }
                let Some(token) = ins.operand.as_token() else { return None };
                let Some(field) = loader.resolve_field_token(assembly, token) else {
                    return None;
                };
                let slot = loader.registry.field(field).slot as usize;
                let synthetic = *replaced.get(&local)?.get(slot)?;
                if ins.op == Op::Stfld {
                    actions.insert(ins.offset, Action::Store(synthetic));
                } else {
                    actions.insert(ins.offset, Action::Load(synthetic));
                    stack.push(Slot::Other);
                }
            }

            // `ret` is signature-dependent, so it has no static stack effect
            // to look up. It also ends the block, and the only thing that
            // matters here is that no object is being handed out of the
            // method — which the check below this `match` already makes.
            Op::Ret => {
                if stack.iter().any(|s| matches!(s, Slot::Object(_))) {
                    return None;
                }
                stack.clear();
            }

            _ => {
                // Everything else: an object reaching it escapes.
                let Some((popped, pushed)) = crate::verify::stack_effect(ins.op) else {
                    // A call, and this pass never lets an object reach one.
                    // Whether the *method* is compilable is not this pass's
                    // question, but it cannot go on tracking past an unknown
                    // stack effect.
                    return None;
                };
                for _ in 0..popped {
                    if matches!(pop(&mut stack), Slot::Object(_)) {
                        return None;
                    }
                }
                for _ in 0..pushed {
                    stack.push(Slot::Other);
                }
            }
        }

        // No object may be live across a control-flow edge.
        let leaves_the_block = ins.operand.as_target().is_some()
            || matches!(ins.operand, Operand::Targets(_))
            || matches!(ins.op, Op::Ret | Op::Throw);
        if leaves_the_block && stack.iter().any(|s| matches!(s, Slot::Object(_))) {
            return None;
        }
    }

    // ── Apply it ───────────────────────────────────────────────────────────
    //
    // Offset and length are preserved on every instruction, so each branch
    // target in the stream still names the instruction it always did and no
    // renumbering is needed.
    let rewritten = instructions
        .iter()
        .map(|ins| match actions.get(&ins.offset) {
            Some(Action::Elide) => Instruction {
                op: Op::Nop,
                operand: Operand::None,
                ..ins.clone()
            },
            Some(Action::Load(n)) => Instruction {
                op: Op::Ldloc,
                operand: Operand::Var(*n),
                ..ins.clone()
            },
            Some(Action::Store(n)) => Instruction {
                op: Op::Stloc,
                operand: Operand::Var(*n),
                ..ins.clone()
            },
            None => ins.clone(),
        })
        .collect();

    let mut dead_locals: Vec<u32> = replaced.keys().copied().collect();
    dead_locals.sort_unstable();

    Some(Scalarised {
        instructions: rewritten,
        extra_locals,
        objects: replaced.len(),
        dead_locals,
    })
}

/// Pops one slot, treating the region below this block as non-object.
fn pop(stack: &mut Vec<Slot>) -> Slot {
    stack.pop().unwrap_or(Slot::Other)
}

/// The local a `stloc` writes, if the instruction is one.
fn store_target(ins: &Instruction) -> Option<u32> {
    match ins.op {
        Op::Stloc0 => Some(0),
        Op::Stloc1 => Some(1),
        Op::Stloc2 => Some(2),
        Op::Stloc3 => Some(3),
        Op::StlocS | Op::Stloc => ins.operand.as_var(),
        _ => None,
    }
}

/// The local a `ldloc` reads, if the instruction is one.
fn load_source(ins: &Instruction) -> Option<u32> {
    match ins.op {
        Op::Ldloc0 => Some(0),
        Op::Ldloc1 => Some(1),
        Op::Ldloc2 => Some(2),
        Op::Ldloc3 => Some(3),
        Op::LdlocS | Op::Ldloc => ins.operand.as_var(),
        _ => None,
    }
}

/// Every offset something branches to.
fn branch_targets(instructions: &[Instruction]) -> BTreeSet<u32> {
    let mut targets = BTreeSet::new();
    for ins in instructions {
        match &ins.operand {
            Operand::Target(t) => {
                targets.insert(*t);
            }
            Operand::Targets(ts) => targets.extend(ts.iter().copied()),
            _ => {}
        }
    }
    targets
}

/// The instance fields of the type `ctor` constructs, when the whole object
/// can be replaced by locals — otherwise `None`.
fn scalarisable_fields(loader: &Loader, ctor: MethodId) -> Option<Vec<FieldId>> {
    if !is_trivial_ctor(loader, ctor) {
        return None;
    }
    let ty = loader.registry.ty(loader.registry.method(ctor).declaring_type);
    // A value type is copied, not referenced, and `newobj` on one means
    // something different. Only heap objects are in scope here.
    if ty.kind != TypeKind::Class {
        return None;
    }
    // Inherited fields would need the base chain walked and every constructor
    // in it proven trivial too.
    let base = loader.registry.ty(ty.base?);
    if !(base.namespace == "System" && base.name == "Object") {
        return None;
    }
    if ty.instance_fields.is_empty() {
        return None;
    }
    // Slots must be dense from zero, or the field-slot-to-local mapping would
    // not line up.
    for (n, field) in ty.instance_fields.iter().enumerate() {
        let f = loader.registry.field(*field);
        if f.slot as usize != n || f.is_static || !crate::translate::is_scalar_sig(&f.signature) {
            return None;
        }
    }
    Some(ty.instance_fields.clone())
}

/// Whether a constructor does nothing but chain to `System.Object`'s.
///
/// Lives on the loader rather than here: the interpreter asks the same
/// question on every `newobj`, and two copies of a rule this load-bearing is
/// how they start disagreeing.
pub fn constructor_does_nothing(loader: &Loader, ctor: MethodId) -> bool {
    loader.constructor_does_nothing(ctor)
}

fn is_trivial_ctor(loader: &Loader, ctor: MethodId) -> bool {
    loader.constructor_does_nothing(ctor)
}
