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

use crate::floats::FloatMap;
use std::collections::{BTreeMap, BTreeSet};
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
    /// Reads local `index` into `dst`.
    ///
    /// Index-based rather than offset-based so a backend can answer from a
    /// register instead of the frame. The default is the frame, which is what
    /// every backend did before any of them kept a local anywhere else.
    fn load_local(&mut self, dst: u8, index: usize) {
        let off = self.local_offset(index);
        self.load_frame(dst, off);
    }

    /// Writes `src` into local `index`.
    fn store_local(&mut self, index: usize, src: u8) {
        let off = self.local_offset(index);
        self.store_frame(off, src);
    }

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

    // -- arrays ---------------------------------------------------------
    //
    // An `int[]` argument does not arrive as a handle. The tier resolves it to
    // a two-word descriptor — `{ data pointer, length }` — and passes the
    // descriptor's *address* in the argument slot. `ldarg` then loads that
    // address like any other integer, so by the time `ldelem.i4` runs the
    // descriptor is already on the evaluation stack and these operations take
    // registers rather than argument indices. Compiled code reads elements
    // with ordinary loads and never calls back into the runtime.
    //
    // That is sound for a narrow, checkable reason: this backend declines any
    // method that allocates, so no collection can run while compiled code is
    // executing, and the mark-sweep collector never moves an object even when
    // it does. The pointer is stable for exactly as long as the call.

    // -- floating point -------------------------------------------------
    //
    // A `double` is eight bytes and so is a slot, so floats travel through the
    // same slots and the same general-purpose registers as integers, as bit
    // patterns. Only the arithmetic needs floating-point registers, and these
    // are the operations that move a pattern into one, work on it, and move
    // the result back.
    //
    // Which instruction is which is not visible in the opcode — IL's `add` is
    // `add` for both — so `floats::infer` works it out from the signature
    // beforehand. See that module on why a guess is not acceptable here.

    /// Whether this backend implements the floating-point operations below.
    ///
    /// Same reasoning as `supports_arrays`: only x86-64 has ever executed an
    /// instruction, and untested float encodings on an untested backend would
    /// be two unknowns stacked.
    fn supports_floats(&self) -> bool {
        false
    }

    /// `dst = lhs <op> rhs`, on the bit patterns in those registers.
    fn float_binop(&mut self, _op: BinOp, _dst: u8, _lhs: u8, _rhs: u8) {
        unreachable!("a backend without float support was asked for float arithmetic")
    }

    /// `dst = -src`.
    fn float_neg(&mut self, _dst: u8, _src: u8) {
        unreachable!("a backend without float support was asked to negate")
    }

    /// Materialise a floating-point comparison as 0 or 1 in `dst`.
    fn float_compare(&mut self, _cond: Cond, _dst: u8, _lhs: u8, _rhs: u8) {
        unreachable!("a backend without float support was asked to compare")
    }

    /// Branch on a floating-point comparison.
    fn float_branch_compare(&mut self, _cond: Cond, _lhs: u8, _rhs: u8, _target: u32) {
        unreachable!("a backend without float support was asked to branch on floats")
    }

    /// `dst = (double) src`, from a signed 64-bit integer.
    fn int_to_float(&mut self, _dst: u8, _src: u8) {
        unreachable!("a backend without float support was asked to convert")
    }

    /// `dst = (long) src`, truncating toward zero as `conv.i8` does.
    fn float_to_int(&mut self, _dst: u8, _src: u8) {
        unreachable!("a backend without float support was asked to convert")
    }

    /// Whether this backend implements the operations below.
    ///
    /// Only x86-64 does. The other two have never executed an instruction, so
    /// giving them array support would be adding untested encodings to
    /// untested backends — `shape_is_compilable_with` keeps array methods away
    /// from them instead.
    fn supports_arrays(&self) -> bool {
        false
    }

    /// `dst = descriptor.length`. `dst` may be the same register as
    /// `descriptor`.
    fn array_length(&mut self, dst: u8, descriptor: u8) {
        let _ = (dst, descriptor);
        unreachable!("this backend declines array access");
    }

    /// `dst = descriptor.data[index]`, trapping if `index` is out of range.
    fn array_load_i32(&mut self, dst: u8, descriptor: u8, index: u8) {
        let _ = (dst, descriptor, index);
        unreachable!("this backend declines array access");
    }

    /// `descriptor.data[index] = value`, trapping if `index` is out of range.
    fn array_store_i32(&mut self, descriptor: u8, index: u8, value: u8) {
        let _ = (descriptor, index, value);
        unreachable!("this backend declines array access");
    }

    /// `dst = descriptor.data[index]` for a `double[]`.
    ///
    /// Eight bytes moved as they are: a `double` travels as a bit pattern
    /// everywhere in compiled code, so there is nothing to sign-extend and
    /// nothing to convert. That is the whole difference from the `int[]` form,
    /// which has to widen 32 bits to 64.
    fn array_load_f64(&mut self, dst: u8, descriptor: u8, index: u8) {
        let _ = (dst, descriptor, index);
        unreachable!("this backend declines array access");
    }

    /// `descriptor.data[index] = value` for a `double[]`.
    fn array_store_f64(&mut self, descriptor: u8, index: u8, value: u8) {
        let _ = (descriptor, index, value);
        unreachable!("this backend declines array access");
    }

    /// `dst = descriptor.data[index]` for a `bool[]`, zero-extended.
    ///
    /// Zero extension rather than sign: `ldelem.u1` is the unsigned form, and
    /// a `bool` is 0 or 1. Sign-extending would turn a byte of 0xFF — which a
    /// `bool[]` should never hold, but a `byte[]` would — into -1.
    fn array_load_u8(&mut self, dst: u8, descriptor: u8, index: u8) {
        let _ = (dst, descriptor, index);
        unreachable!("this backend declines array access");
    }

    /// `descriptor.data[index] = value` for a `bool[]`, one byte.
    fn array_store_u8(&mut self, descriptor: u8, index: u8, value: u8) {
        let _ = (descriptor, index, value);
        unreachable!("this backend declines array access");
    }

    /// `dst = receiver.field[slot]`, converted according to `kind`.
    ///
    /// A call rather than a load: the heap's value representation is not part
    /// of this backend's ABI, and it is not going to become one.
    fn field_load(&mut self, dst: u8, receiver: u8, slot: usize, kind: i64) {
        let _ = (dst, receiver, slot, kind);
        unreachable!("this backend declines field access");
    }

    /// `receiver.field[slot] = value`, converted according to `kind`.
    fn field_store(&mut self, receiver: u8, slot: usize, kind: i64, value: u8) {
        let _ = (receiver, slot, kind, value);
        unreachable!("this backend declines field access");
    }

    /// Polls for a requested collection, parking if one is under way.
    ///
    /// Only emitted at a loop back-edge, where the evaluation stack is empty,
    /// so the frame's roots are exactly its reference-typed locals and
    /// arguments — slots whose offsets the compiler knows from declared types
    /// alone, with no inference over the stream.
    fn safepoint(&mut self) {
        unreachable!("this backend declines methods that need a safe point");
    }

    /// Whether this backend can call itself recursively.
    fn supports_self_calls(&self) -> bool {
        false
    }

    /// Writes outgoing argument `index` for a self-call.
    fn outgoing_arg(&mut self, _index: usize, _src: u8) {
        unreachable!("this backend declines calls");
    }

    /// Calls table entry `index`, leaving any result in `dst`.
    ///
    /// `arg_count` is the callee's, which is what the outgoing frame's trap and
    /// context slots sit past — they are at a different place for every callee.
    fn call_other(&mut self, _dst: u8, _index: usize, _arg_count: usize) {
        unreachable!("this backend declines calls to other methods");
    }

    /// Calls the method being compiled, recursively, leaving any result in
    /// `dst`. A trap raised inside the callee propagates to this frame.
    fn call_self(&mut self, _dst: u8) {
        unreachable!("this backend declines calls");
    }

    /// Whether this backend can allocate.
    fn supports_allocation(&self) -> bool {
        false
    }

    /// `dst = new array of `length` elements of `kind`', as a descriptor
    /// address; takes the trap path when the heap cannot satisfy it.
    ///
    /// `kind` is one of [`crate::alloc::ELEMENT_I4`] or
    /// [`crate::alloc::ELEMENT_R8`]. The call goes to
    /// [`crate::alloc::alloc_array`] through a context pointer the tier left
    /// in the argument array.
    fn alloc_array(&mut self, _dst: u8, _kind: i64, _length: u8) {
        unreachable!("this backend declines allocation");
    }

    /// `dst = descriptor.data[index]` for a reference array — one handle.
    ///
    /// A handle is eight bytes and `repr(transparent)` over `u64`, so this is
    /// the same load `array_load_f64` performs. It is named separately anyway,
    /// so a backend cannot start treating a reference as a number by accident.
    fn array_load_ref(&mut self, dst: u8, descriptor: u8, index: u8) {
        let _ = (dst, descriptor, index);
        unreachable!("this backend declines reference arrays");
    }

    /// `descriptor.data[index] = value`, storing a handle.
    fn array_store_ref(&mut self, descriptor: u8, index: u8, value: u8) {
        let _ = (descriptor, index, value);
        unreachable!("this backend declines reference arrays");
    }

    /// Allocates one instance of the class the compiler numbered `shape`,
    /// leaving its handle in `dst`; takes the trap path when the heap cannot
    /// satisfy it.
    ///
    /// The constructor is not run — only classes whose constructor does
    /// nothing but chain to `System.Object`'s reach here.
    fn alloc_object(&mut self, _dst: u8, _shape: i64) {
        unreachable!("this backend declines allocation");
    }

    /// Guarded devirtualisation: `dst` becomes the value the override for the
    /// receiver's runtime type returns.
    ///
    /// `candidates` pairs a type's registry index with the constant its
    /// override produces. The list is exhaustive by class-hierarchy analysis —
    /// the compiler only emits this when no assembly can introduce another
    /// subtype — so the fall-through is an assertion rather than a fallback.
    /// It has to be: there is no on-stack replacement here, and a compiled body
    /// that has already allocated has no sound way back to the interpreter.
    fn devirt_dispatch(&mut self, _dst: u8, _receiver: u8, _candidates: &[(i64, i64)]) {
        unreachable!("this backend declines virtual calls");
    }
}

/// Whether an opcode the walk understands is one this crate emits code for.
/// Opcodes that only compile when the backend supports arrays.
pub fn is_array_op(op: Op) -> bool {
    matches!(
        op,
        Op::Ldlen
            | Op::LdelemI4
            | Op::StelemI4
            | Op::LdelemR8
            | Op::StelemR8
            | Op::LdelemU1
            | Op::StelemI1
            | Op::Newarr
            | Op::LdelemRef
            | Op::StelemRef
    )
}

/// Opcodes that only compile when the backend can create and dispatch on
/// objects.
///
/// Separate from [`is_array_op`] because they carry a heavier obligation: a
/// `newobj` here runs no constructor, and a `callvirt` is only emitted when
/// every answer it can give is already known.
pub fn is_object_op(op: Op) -> bool {
    matches!(op, Op::Newobj | Op::Callvirt | Op::Ldfld | Op::Stfld | Op::Ldnull)
}

/// Slot and conversion kind for every `ldfld` and `stfld` in a body.
///
/// `None` when one of them names a field this backend cannot carry — a struct
/// field, or one whose declaring type it cannot lay out. Static fields are
/// excluded too: they live in the loader's static storage rather than in an
/// object, and reach compiled code by a different route or not at all.
pub fn field_accesses(
    loader: &rustclr_core::Loader,
    assembly: rustclr_core::AssemblyId,
    instructions: &[Instruction],
) -> Option<BTreeMap<u32, (usize, i64)>> {
    let mut out = BTreeMap::new();
    for ins in instructions {
        if !matches!(ins.op, Op::Ldfld | Op::Stfld) {
            continue;
        }
        let token = ins.operand.as_token()?;
        let field = loader.resolve_field_token(loader.assembly(assembly), token)?;
        let info = loader.registry.field(field);
        if info.is_static {
            return None;
        }
        let kind = field_kind(&info.signature)?;
        out.insert(ins.offset, (loader.instance_field_slot(field)?, kind));
    }
    Some(out)
}

/// How a field's declared type crosses into a register.
fn field_kind(sig: &rustclr_metadata::TypeSig) -> Option<i64> {
    use rustclr_metadata::TypeSig as T;
    Some(match sig.unwrap_modifiers() {
        T::Boolean | T::Char | T::I1 | T::U1 | T::I2 | T::U2 | T::I4 | T::U4 => {
            crate::alloc::FIELD_I4
        }
        T::I8 | T::U8 => crate::alloc::FIELD_I8,
        T::R8 | T::R4 => crate::alloc::FIELD_R8,
        T::Class(_) | T::Object => crate::alloc::FIELD_REF,
        // A struct field is stored as a `Value::Struct`, which is not one
        // word and has no register representation here.
        _ => return None,
    })
}

/// Whether a signature is an array of references.
///
/// Its elements are handles, which compiled code holds as opaque 64-bit tokens
/// — it can neither dereference one nor learn anything from it without asking
/// a helper.
pub fn is_ref_array(sig: &rustclr_metadata::TypeSig) -> bool {
    matches!(
        sig.unwrap_modifiers(),
        rustclr_metadata::TypeSig::SzArray(inner)
            if matches!(
                inner.unwrap_modifiers(),
                rustclr_metadata::TypeSig::Class(_) | rustclr_metadata::TypeSig::Object
            )
    )
}

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

/// Whether a signature type occupies one integer register.
fn integer_shaped(sig: &rustclr_metadata::TypeSig) -> bool {
    use rustclr_metadata::TypeSig as T;
    matches!(
        sig.unwrap_modifiers(),
        T::Boolean | T::Char | T::I1 | T::U1 | T::I2 | T::U2 | T::I4 | T::U4 | T::I8 | T::U8
    )
}

/// Whether scalar replacement could plausibly rescue this method.
///
/// The cheap screen, on declared types alone: a body that creates an object and
/// keeps it in a class-typed local *might* be one where that object never
/// escapes, and `escape::scalarise` asks the real question once it can see the
/// rewritten body.
///
/// Screening on the declared locals instead would reject every such method
/// before the analysis ever ran — which is precisely what happened to floats,
/// and left a differential test comparing the interpreter with itself.
pub fn scalarisation_possible(
    registry: &rustclr_core::TypeRegistry,
    method: rustclr_core::MethodId,
) -> bool {
    let info = registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else { return false };
    if !body.exception_clauses.is_empty() || info.signature.has_this {
        return false;
    }
    if !integer_shaped(&info.signature.return_type) && !info.returns_void() {
        return false;
    }
    if !info.signature.params.iter().all(|p| integer_shaped(p) || is_int_array(p)) {
        return false;
    }
    // Every local is either something the backend already takes, or a class
    // reference that scalar replacement might retire. Anything else — a
    // struct, a byref, a pointer — is beyond this pass.
    let plausible = |t: &rustclr_metadata::TypeSig| {
        integer_shaped(t)
            || crate::floats::is_float_sig(t)
            || is_int_array(t)
            || is_double_array(t)
            || is_bool_array(t)
            || matches!(t.unwrap_modifiers(), rustclr_metadata::TypeSig::Class(_))
    };
    if !body.locals.iter().all(plausible) {
        return false;
    }
    // No object to retire means nothing for the pass to do.
    matches!(rustclr_core::opcode::decode_all(&body.il),
        Ok(instructions) if instructions.iter().any(|i| i.op == Op::Newobj))
}

/// Whether a slot of this type holds a *handle* at run time.
///
/// The distinction that matters for a safe point. A class-typed slot holds a
/// handle, which the collector must be told about or it will reclaim the
/// object. An array-typed slot holds a **descriptor address** — compiled code
/// receives arrays as a data pointer and a length, not as a handle — so it is
/// not a root, and the array behind it is kept alive by a pin taken for the
/// duration of the call instead. Reading a descriptor address as a handle
/// would hand the collector a garbage slot index.
pub fn is_reference_sig(sig: &rustclr_metadata::TypeSig) -> bool {
    matches!(
        sig.unwrap_modifiers(),
        rustclr_metadata::TypeSig::Class(_) | rustclr_metadata::TypeSig::Object
    )
}

/// Whether a signature is a scalar the backend keeps in a register.
///
/// The union of the two kinds it can hold: an integer widened to 64 bits, or a
/// double as its bit pattern. A reference, a struct or an array is none of
/// these, which is what makes this the test for whether a field can become a
/// local.
pub fn is_scalar_sig(sig: &rustclr_metadata::TypeSig) -> bool {
    integer_shaped(sig) || crate::floats::is_float_sig(sig)
}

/// Whether a method is the shape every baseline backend accepts.
///
/// Shared rather than per-architecture: the backends differ in how they encode,
/// not in what they take. Duplicating this is how one of them quietly starts
/// accepting a method the others refuse.
/// Whether a signature entry is an `int[]`.
///
/// Only `int[]`, and only as a parameter. A wider set would mean more element
/// widths in the emitter for no more coverage of the loops that matter, and an
/// array *local* would have to be tracked through the frame rather than
/// arriving as a descriptor the caller built.
pub fn is_int_array(sig: &rustclr_metadata::TypeSig) -> bool {
    matches!(sig.unwrap_modifiers(), rustclr_metadata::TypeSig::SzArray(inner)
        if matches!(inner.unwrap_modifiers(), rustclr_metadata::TypeSig::I4))
}

/// Whether a signature is `bool[]`.
///
/// `byte[]` and `sbyte[]` have the same element width and are deliberately
/// left out: they would need the signed and unsigned loads told apart, and
/// nothing has needed them yet.
pub fn is_bool_array(sig: &rustclr_metadata::TypeSig) -> bool {
    matches!(sig.unwrap_modifiers(), rustclr_metadata::TypeSig::SzArray(inner)
        if matches!(inner.unwrap_modifiers(), rustclr_metadata::TypeSig::Boolean))
}

/// Whether a signature is `double[]`.
///
/// `float[]` is deliberately excluded: its elements are four bytes and the
/// evaluation stack is `F`, so every load would have to widen and every store
/// round — real work, and nothing this runtime has met needs it yet.
pub fn is_double_array(sig: &rustclr_metadata::TypeSig) -> bool {
    matches!(sig.unwrap_modifiers(), rustclr_metadata::TypeSig::SzArray(inner)
        if matches!(inner.unwrap_modifiers(), rustclr_metadata::TypeSig::R8))
}

/// The shape check, told whether the backend can take arrays.
///
/// Only x86-64 can. The AArch64 and RISC-V backends have never executed an
/// instruction, and giving them array encodings would be stacking untested
/// code on untested code — so they keep the narrower shape and this parameter
/// is how that stays explicit rather than implicit.
pub fn shape_is_compilable_with(
    registry: &rustclr_core::TypeRegistry,
    method: rustclr_core::MethodId,
    arrays: bool,
) -> bool {
    let info = registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else { return false };
    if !body.exception_clauses.is_empty() || info.signature.has_this {
        return false;
    }
    if !integer_shaped(&info.signature.return_type) && !info.returns_void() {
        return false;
    }
    let param_ok = |p: &rustclr_metadata::TypeSig| {
        integer_shaped(p) || (arrays && is_int_array(p))
    };
    if !info.signature.params.iter().all(param_ok) {
        return false;
    }
    // Locals stay integer-only even with arrays on: a descriptor stored to a
    // local and read back would work, but an array local declared as `int[]`
    // and assigned from `newarr` would not, and the two are indistinguishable
    // from the local's signature alone.
    if !body.locals.iter().all(integer_shaped) {
        return false;
    }
    let Ok(instructions) = rustclr_core::opcode::decode_all(&body.il) else { return false };
    // No `call` here. This is the screen used when the inliner is *off*, and
    // it cannot resolve a token to see whether a call targets this method
    // itself — so it declines them all, and a recursive method is compiled
    // only in the ordinary inlining path where `compile` can check properly.
    instructions
        .iter()
        .all(|i| is_supported(i.op) || (arrays && is_array_op(i.op)))
}

pub fn shape_is_compilable(
    registry: &rustclr_core::TypeRegistry,
    method: rustclr_core::MethodId,
) -> bool {
    let info = registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else { return false };
    if !body.exception_clauses.is_empty() {
        return false;
    }
    // Only integer-shaped signatures: every argument, local and the result must
    // fit one register.
    if !integer_shaped(&info.signature.return_type) && !info.returns_void() {
        return false;
    }
    if !info.signature.params.iter().all(integer_shaped) {
        return false;
    }
    if info.signature.has_this {
        return false;
    }
    if !body.locals.iter().all(integer_shaped) {
        return false;
    }
    let Ok(instructions) = rustclr_core::opcode::decode_all(&body.il) else { return false };
    instructions.iter().all(|i| is_supported(i.op))
}

/// The same check, but tolerating `call` — a method that is otherwise in shape
/// and whose only unsupported opcode is a call *might* become compilable once
/// the inliner has run.
///
/// This is deliberately a maybe. It is the cheap screen the tier uses to avoid
/// decoding twice; the binding answer is
/// [`shape_is_compilable_after_inlining`], applied to the rewritten stream.
pub fn shape_might_compile_after_inlining(
    registry: &rustclr_core::TypeRegistry,
    method: rustclr_core::MethodId,
) -> bool {
    let info = registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else { return false };
    if !body.exception_clauses.is_empty() || info.signature.has_this {
        return false;
    }
    if !integer_shaped(&info.signature.return_type) && !info.returns_void() {
        return false;
    }
    // Arrays in either place. `compile` makes the real decision once it can
    // see the rewritten body; this only has to avoid rejecting something that
    // would have compiled.
    let slot_ok = |p: &rustclr_metadata::TypeSig| {
        integer_shaped(p) || is_int_array(p) || is_bool_array(p) || is_ref_array(p) || is_reference_sig(p)
    };
    if !info.signature.params.iter().all(slot_ok) {
        return false;
    }
    if !body.locals.iter().all(slot_ok) {
        return false;
    }
    let Ok(instructions) = rustclr_core::opcode::decode_all(&body.il) else { return false };
    instructions
        .iter()
        .all(|i| is_supported(i.op) || is_array_op(i.op) || is_object_op(i.op) || i.op == Op::Call)
}

/// The shape check, judged against an already-rewritten instruction stream.
///
/// Separate from [`shape_is_compilable`] because that one decodes the method's
/// own IL, and after inlining the stream no longer matches what is in the
/// image. The signature and local checks are the same; only the opcode source
/// differs.
/// IL offsets whose `call` targets `method` itself.
///
/// `None` when the body contains a call this backend cannot emit — anything
/// that is not a self-call, since a call to another method needs an address
/// that may not exist yet. The inliner handles the small-leaf-callee case;
/// this handles the one it cannot, which is recursion.
pub fn self_calls(
    loader: &rustclr_core::Loader,
    assembly: rustclr_core::AssemblyId,
    method: rustclr_core::MethodId,
    instructions: &[Instruction],
) -> Option<BTreeSet<u32>> {
    let mut offsets = BTreeSet::new();
    for ins in instructions {
        if ins.op != Op::Call {
            continue;
        }
        let token = ins.operand.as_token()?;
        let target = loader.resolve_method_token(loader.assembly(assembly), token)?;
        if target != method {
            continue;
        }
        offsets.insert(ins.offset);
    }
    Some(offsets)
}

/// Where each `call` to *another* method goes, and what it needs.
///
/// The address of a compiled method does not exist when its caller is emitted —
/// it may not be compiled yet, and two methods may call each other — so the
/// call is indirect through a table the tier fills in before the caller runs.
/// This decides the table: one entry per distinct callee, and the index each
/// call site loads.
///
/// `None` when a callee is one this backend would not compile. Whether it
/// *will* be compiled is settled at run time; whether it *could* be has to be
/// settled here, or the caller would be emitted around a call that can never
/// be filled in.
pub struct CallTargets {
    /// Call site offset to (table index, the callee's argument count).
    pub sites: BTreeMap<u32, (usize, usize)>,
    /// The callees, in table order.
    pub table: Vec<rustclr_core::MethodId>,
    /// Arguments the widest callee takes, for sizing the outgoing frame.
    pub widest: usize,
}

pub fn call_targets(
    loader: &rustclr_core::Loader,
    assembly: rustclr_core::AssemblyId,
    method: rustclr_core::MethodId,
    instructions: &[Instruction],
    eligible: impl Fn(rustclr_core::MethodId) -> bool,
) -> Option<CallTargets> {
    let mut sites = BTreeMap::new();
    let mut table: Vec<rustclr_core::MethodId> = Vec::new();
    let mut widest = 0usize;
    for ins in instructions {
        if ins.op != Op::Call {
            continue;
        }
        let token = ins.operand.as_token()?;
        let target = loader.resolve_method_token(loader.assembly(assembly), token)?;
        if target == method {
            continue;
        }
        if !eligible(target) {
            return None;
        }
        let info = loader.registry.method(target);
        // An instance method has a receiver this convention does not carry, and
        // a void one leaves nothing where the caller expects a result.
        if info.signature.has_this {
            return None;
        }
        let arg_count = info.arg_count();
        widest = widest.max(arg_count);
        let index = match table.iter().position(|m| *m == target) {
            Some(i) => i,
            None => {
                table.push(target);
                table.len() - 1
            }
        };
        sites.insert(ins.offset, (index, arg_count));
    }
    Some(CallTargets { sites, table, widest })
}

/// The element kind each `newarr` allocates, when every one of them is
/// something this backend can allocate *and* outside any loop.
///
/// `None` declines the method. Two reasons, and both matter:
///
/// * an element type other than `int` or `double` has no allocation path here;
/// * a `newarr` inside a loop makes the number of allocations unbounded, and
///   compiled code never collects — so a loop allocating would exhaust the
///   heap where the interpreter would not. A program that runs interpreted and
///   fails compiled is the one difference this runtime will not accept.
/// `polls` says whether the body reaches a safe point at every loop back-edge.
/// When it does, an allocation inside a loop is no longer unbounded: the
/// collector can stop this method mid-body and reclaim what it has finished
/// with, exactly as it can between two interpreted instructions. When it does
/// not, the old rule stands and such a method is declined.
pub fn newarr_kinds(
    loader: &rustclr_core::Loader,
    assembly: rustclr_core::AssemblyId,
    instructions: &[Instruction],
    shapes: &mut ShapeTable,
    polls: bool,
) -> Option<BTreeMap<u32, i64>> {
    let mut kinds = BTreeMap::new();
    for ins in instructions {
        if ins.op != Op::Newarr {
            continue;
        }
        if !polls && inside_a_loop(instructions, ins.offset) {
            return None;
        }
        let token = ins.operand.as_token()?;
        let element = loader.resolve_type_token(loader.assembly(assembly), token)?;
        let kind = match loader.registry.ty(element).primitive {
            Some(rustclr_core::Primitive::Int32) => crate::alloc::ELEMENT_I4,
            Some(rustclr_core::Primitive::Double) => crate::alloc::ELEMENT_R8,
            Some(rustclr_core::Primitive::Boolean) => crate::alloc::ELEMENT_U1,
            Some(_) => return None,
            // A reference array. Its elements are handles, and the shape table
            // says which class — `ELEMENT_REF` plus that index.
            None => {
                let index = shapes.intern(loader, element)?;
                crate::alloc::ELEMENT_REF + index
            }
        };
        kinds.insert(ins.offset, kind);
    }
    Some(kinds)
}

/// Shape index for the `newobj` at each offset, when every one of them
/// constructs a class this backend can allocate.
///
/// The constructor must do nothing but chain to `System.Object`'s — the same
/// test scalar replacement applies, and for the same reason: the backend runs
/// no constructor, so one that did work would silently not happen.
pub fn newobj_shapes(
    loader: &rustclr_core::Loader,
    assembly: rustclr_core::AssemblyId,
    instructions: &[Instruction],
    shapes: &mut ShapeTable,
    polls: bool,
) -> Option<BTreeMap<u32, i64>> {
    let mut out = BTreeMap::new();
    for ins in instructions {
        if ins.op != Op::Newobj {
            continue;
        }
        // Allocation inside a loop was unbounded while nothing could collect
        // during a compiled call. A body that polls at its back-edges can be
        // stopped there, so the bound is the same one interpreted code has.
        if !polls && inside_a_loop(instructions, ins.offset) {
            return None;
        }
        let token = ins.operand.as_token()?;
        let ctor = loader.resolve_method_token(loader.assembly(assembly), token)?;
        if !crate::escape::constructor_does_nothing(loader, ctor) {
            return None;
        }
        let declaring = loader.registry.method(ctor).declaring_type;
        if loader.registry.ty(declaring).kind != rustclr_core::TypeKind::Class {
            return None;
        }
        out.insert(ins.offset, shapes.intern(loader, declaring)?);
    }
    Some(out)
}

/// The classes one compiled body refers to, numbered as its code numbers them.
///
/// Interned rather than looked up globally, because the index is baked into the
/// emitted instructions and means nothing outside that body.
#[derive(Default)]
pub struct ShapeTable {
    pub entries: Vec<(rustclr_core::TypeId, usize, rustclr_core::TypeId)>,
}

impl ShapeTable {
    /// The index for `ty`, adding it if this body has not named it yet.
    fn intern(&mut self, loader: &rustclr_core::Loader, ty: rustclr_core::TypeId) -> Option<i64> {
        if let Some(i) = self.entries.iter().position(|(t, _, _)| *t == ty) {
            return Some(i as i64);
        }
        let info = loader.registry.ty(ty);
        if info.kind != rustclr_core::TypeKind::Class {
            return None;
        }
        // Every field the type *has*, not just the ones it declares. A
        // subclass's instance is laid out base-first, so counting only the
        // declaring type's own fields would allocate an object too small for
        // the slots its own code addresses.
        let field_count = loader.registry.base_chain(ty).map(|t| {
            loader.registry.ty(t).instance_fields.len()
        }).sum::<usize>();
        // `find_sz_array` only knows constructions something has already named;
        // `System.Array` stands in, as it does for the primitive element types.
        let array = loader
            .registry
            .find_sz_array(ty)
            .unwrap_or_else(|| loader.core().array);
        self.entries.push((ty, field_count, array));
        Some((self.entries.len() - 1) as i64)
    }
}

/// Whether `offset` lies inside a loop body.
///
/// A backward branch at `b` to `t` means everything in `t..=b` runs repeatedly.
/// Conservative in the safe direction: an offset it calls looped is declined,
/// and declining compiles nothing rather than compiling something unbounded.
pub(crate) fn inside_a_loop(instructions: &[Instruction], offset: u32) -> bool {
    instructions.iter().any(|ins| {
        match ins.operand.as_target() {
            Some(target) if target <= ins.offset => target <= offset && offset <= ins.offset,
            _ => false,
        }
    })
}

/// Whether a method's *signature and locals* could be float-compilable.
///
/// The cheap screen, before the body is decoded or inlined. `float_shape` asks
/// the real question afterwards.
pub fn float_shape_possible(
    registry: &rustclr_core::TypeRegistry,
    method: rustclr_core::MethodId,
) -> bool {
    let info = registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else { return false };
    if !body.exception_clauses.is_empty() || info.signature.has_this {
        return false;
    }
    let ok = |t: &rustclr_metadata::TypeSig| {
        integer_shaped(t) || crate::floats::is_float_sig(t)
    };
    // Some float somewhere, or the integer gate should be handling it.
    let any_float = crate::floats::is_float_sig(&info.signature.return_type)
        || info.signature.params.iter().any(crate::floats::is_float_sig)
        || body.locals.iter().any(crate::floats::is_float_sig);
    let any_float = any_float
        || info.signature.params.iter().any(is_double_array)
        || body.locals.iter().any(is_double_array);
    let slot_ok = |t: &rustclr_metadata::TypeSig| {
        ok(t) || is_int_array(t) || is_double_array(t) || is_bool_array(t) || is_ref_array(t) || is_reference_sig(t)
    };
    any_float
        && (ok(&info.signature.return_type) || info.returns_void())
        && info.signature.params.iter().all(slot_ok)
        && body.locals.iter().all(slot_ok)
}

/// The float map for a method, when its shape is compilable with floats.
///
/// `None` means "run it interpreted". A method reaches here only if the
/// integer gate already declined it *because* of floats, so this is the second
/// question rather than a replacement for the first: are the only reasons it
/// was declined floating-point ones this backend can emit?
///
/// [`crate::floats::infer`] does the deciding, and declines rather than guesses
/// — an integer `add` emitted for a double gives a number, silently, and the
/// wrong one.
/// `locals` is the *effective* locals list, which is the declared one unless a
/// pass rewrote the body. Scalar replacement appends a slot per replaced field
/// and neutralises the object local it retired, and the inference below has to
/// type the `ldloc`s it produced — so reading `body.locals` here instead would
/// see a body that no longer exists.
pub fn float_shape(
    registry: &rustclr_core::TypeRegistry,
    method: rustclr_core::MethodId,
    instructions: &[Instruction],
    locals: &[rustclr_metadata::TypeSig],
) -> Option<crate::floats::FloatMap> {
    let info = registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else { return None };
    if !body.exception_clauses.is_empty() || info.signature.has_this {
        return None;
    }
    // Every argument, local and the result must be an integer *or* a float;
    // nothing else has a representation in a compiled frame.
    let ok = |t: &rustclr_metadata::TypeSig| {
        integer_shaped(t) || crate::floats::is_float_sig(t)
    };
    // An array is fine as a parameter *or* a local now. It was locals-excluded
    // while allocation was declined, because a local declared `int[]` might
    // hold either a descriptor the tier passed in or the result of a `newarr`
    // the backend could not emit, and the signature does not say which. Both
    // are descriptor addresses now, so the distinction stopped mattering.
    let slot_ok = |t: &rustclr_metadata::TypeSig| {
        ok(t) || is_int_array(t) || is_double_array(t) || is_bool_array(t) || is_ref_array(t) || is_reference_sig(t)
    };
    if !ok(&info.signature.return_type) && !info.returns_void() {
        return None;
    }
    if !info.signature.params.iter().all(slot_ok) || !locals.iter().all(slot_ok) {
        return None;
    }

    let params: Vec<_> = info.signature.params.iter().map(|p| p.clone()).collect();
    let map = crate::floats::infer(
        instructions,
        &params,
        locals,
        &info.signature.return_type,
    )?;
    // No floats at all means the integer gate should have taken it, and this
    // path adds nothing.
    map.any.then_some(map)
}

/// `locals` is the effective locals list; see [`float_shape`] for why the
/// declared one is not enough once a pass has rewritten the body.
pub fn shape_is_compilable_after_inlining(
    registry: &rustclr_core::TypeRegistry,
    method: rustclr_core::MethodId,
    instructions: &[Instruction],
    arrays: bool,
    locals: &[rustclr_metadata::TypeSig],
) -> bool {
    let info = registry.method(method);
    let rustclr_core::MethodKind::Il(body) = &info.kind else { return false };
    if !body.exception_clauses.is_empty() || info.signature.has_this {
        return false;
    }
    if !integer_shaped(&info.signature.return_type) && !info.returns_void() {
        return false;
    }
    // An array is fine as a parameter *or* a local. Locals were excluded while
    // allocation was declined, because a local declared `int[]` might hold
    // either a descriptor the tier passed in or the result of a `newarr` the
    // backend could not emit, and the signature does not say which. Both are
    // descriptor addresses now, so the distinction stopped mattering.
    let slot_ok = |p: &rustclr_metadata::TypeSig| {
        integer_shaped(p) || (arrays && (is_int_array(p) || is_bool_array(p) || is_ref_array(p) || is_reference_sig(p)))
    };
    if !info.signature.params.iter().all(slot_ok) {
        return false;
    }
    if !locals.iter().all(slot_ok) {
        return false;
    }
    // `call` passes here and is checked properly in `compile`, which is the
    // only place that can resolve a token and see whether the target is this
    // method itself. Anything else is declined there.
    instructions
        .iter()
        .all(|i| is_supported(i.op) || (arrays && (is_array_op(i.op) || is_object_op(i.op))) || i.op == Op::Call)
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
    translate_with_plan(b, instructions, analysis, locals, returns_value, &Plan::default())
}

/// What the walk cannot work out from the instruction stream alone.
///
/// Both of these need something the walk does not have: the float map needs
/// the method's signature, and the element kinds need the loader to resolve a
/// `newarr` token. They are computed once, before emission starts.
#[derive(Default, Clone)]
pub struct Plan {
    pub floats: FloatMap,
    /// Element kind for the `newarr` at each IL offset.
    pub newarr_kinds: BTreeMap<u32, i64>,
    /// IL offsets whose `call` targets the method being compiled.
    ///
    /// Recursion is the one call shape the inliner cannot remove — splicing a
    /// method into itself does not terminate — so it is the one that needed a
    /// calling convention of its own.
    pub self_calls: BTreeSet<u32>,
    /// Arguments the method being compiled takes, for laying out a self-call.
    pub arg_count: usize,
    /// Whether a self-call leaves a value on the stack.
    pub returns_value: bool,
    /// Shape index for the `newobj` at each IL offset.
    pub newobj_shapes: BTreeMap<u32, i64>,
    /// Guarded dispatch cases for the `callvirt` at each IL offset.
    pub devirt: BTreeMap<u32, crate::devirt::Cases>,
    /// Slot and kind for the `ldfld`/`stfld` at each IL offset.
    pub fields: BTreeMap<u32, (usize, i64)>,
    /// Table index and argument count for each call to another method.
    pub call_sites: BTreeMap<u32, (usize, usize)>,
    /// Whether a call to another method leaves a value on the stack, by site.
    pub call_returns: BTreeMap<u32, bool>,
    /// IL offsets that begin a loop body and must poll for a collection.
    ///
    /// Empty when the method has no loop, which is most of them — a body that
    /// runs a bounded number of instructions reaches its own return sooner than
    /// any collection could need it to stop.
    pub safepoints: BTreeSet<u32>,
}

/// [`translate`], told which instructions are floating-point.
///
/// The map comes from [`crate::floats::infer`], because an IL arithmetic
/// opcode does not say whether its operands are integers or doubles — the
/// types that reached it do, and only a walk over the stream can recover that.
pub fn translate_with_plan<B: Backend>(
    b: &mut B,
    instructions: &[Instruction],
    analysis: &MethodAnalysis,
    locals: usize,
    returns_value: bool,
    plan: &Plan,
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
        // A safe point, emitted *before* the instruction: this offset is the
        // target of a backward branch, so every path arriving here is about to
        // run a loop body again. Placing it here rather than at the branch
        // means the poll sits where the evaluation stack is known empty.
        if plan.safepoints.contains(&ins.offset) {
            b.safepoint();
        }
        one(b, ins, depth as usize, returns_value, plan)?;
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
    plan: &Plan,
) -> Result<(), CompileError> {
    use Op::*;
    let floats = &plan.floats;

    let t0 = b.temp(0);
    let t1 = b.temp(1);
    let t2 = b.temp(2);

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
            b.load_local(t0, index);
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
            b.store_local(index, t0);
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
            if floats.is_float_op(ins.offset) {
                b.float_binop(op, t0, t0, t1);
            } else {
                b.binop(op, t0, t0, t1);
            }
            write_slot(b, depth - 2, t0);
        }

        Neg | Not | ConvI4 => {
            let op = match ins.op {
                Neg => UnOp::Neg,
                Not => UnOp::Not,
                _ => UnOp::SignExtend32,
            };
            read_slot(b, t0, depth - 1);
            match (ins.op, floats.is_float_op(ins.offset)) {
                (Neg, true) => b.float_neg(t0, t0),
                // `conv.i4` on a double is a real conversion followed by the
                // narrowing. Doing only the narrowing sign-extends the *bit
                // pattern* — `(int)3.5` came out 0, because the low 32 bits of
                // 3.5's representation are zero.
                (ConvI4, true) => {
                    b.float_to_int(t0, t0);
                    b.unop(UnOp::SignExtend32, t0, t0);
                }
                _ => b.unop(op, t0, t0),
            }
            write_slot(b, depth - 1, t0);
        }

        // Already 64-bit on the evaluation stack — unless a double is being
        // truncated to one, which is a real conversion rather than a no-op.
        ConvI8 | ConvI => {
            if floats.is_float_op(ins.offset) {
                read_slot(b, t0, depth - 1);
                b.float_to_int(t0, t0);
                write_slot(b, depth - 1, t0);
            }
        }

        // A float constant needs no floating-point instruction: the value is
        // loaded as its bit pattern, which is exactly how doubles travel
        // through slots and registers everywhere else here.
        LdcR4 | LdcR8 => {
            let Operand::F64(value) = ins.operand else {
                return Err(CompileError::Unsupported("a float constant with no value".into()));
            };
            // `ldc.r4` pushes the value widened to the stack's `F` type, so
            // rounding through `f32` first is what makes the compiled result
            // match the interpreted one bit for bit.
            let widened = if ins.op == LdcR4 { value as f32 as f64 } else { value };
            b.load_imm(t0, widened.to_bits() as i64);
            write_slot(b, depth, t0);
        }

        // A call to another compiled method. Indirect through a table the tier
        // filled in, because the callee's address did not exist when this was
        // emitted — it may not have been compiled yet, and two methods may call
        // each other.
        Call if plan.call_sites.contains_key(&ins.offset) => {
            let (index, arg_count) = plan.call_sites[&ins.offset];
            if depth < arg_count {
                return Err(CompileError::Unsupported(
                    "a call with fewer arguments on the stack than it takes".into(),
                ));
            }
            for i in 0..arg_count {
                read_slot(b, t0, depth - arg_count + i);
                b.outgoing_arg(i, t0);
            }
            b.call_other(t0, index, arg_count);
            if plan.call_returns.get(&ins.offset).copied().unwrap_or(false) {
                write_slot(b, depth - arg_count, t0);
            }
        }

        // A recursive call. The arguments are copied into an outgoing frame the
        // prologue reserved, and the callee is entered exactly as the tier
        // enters this method — same argument-array shape, same trap slot.
        Call if plan.self_calls.contains(&ins.offset) => {
            let n = plan.arg_count;
            if depth < n {
                return Err(CompileError::Unsupported(
                    "a self-call with fewer arguments on the stack than it takes".into(),
                ));
            }
            for i in 0..n {
                read_slot(b, t0, depth - n + i);
                b.outgoing_arg(i, t0);
            }
            b.call_self(t0);
            if plan.returns_value {
                write_slot(b, depth - n, t0);
            }
        }

        // `newarr`. The element type was resolved at compile time, so only the
        // length is dynamic; a failed allocation takes the same trap path a
        // failed bounds check does.
        Newarr => {
            let Some(kind) = plan.newarr_kinds.get(&ins.offset).copied() else {
                return Err(CompileError::Unsupported(
                    "newarr of an element type this backend does not allocate".into(),
                ));
            };
            read_slot(b, t0, depth - 1);
            b.alloc_array(t0, kind, t0);
            write_slot(b, depth - 1, t0);
        }

        // A reference array element: one handle, eight bytes. The handle is an
        // opaque token rather than an address, so nothing here can dereference
        // it — the only thing compiled code does with one is hand it back to a
        // helper.
        LdelemRef => {
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.array_load_ref(t0, t0, t1);
            write_slot(b, depth - 2, t0);
        }

        StelemRef => {
            read_slot(b, t0, depth - 3);
            read_slot(b, t1, depth - 2);
            read_slot(b, t2, depth - 1);
            b.array_store_ref(t0, t1, t2);
        }

        // `newobj` of a class whose constructor does nothing. The allocation
        // itself takes the same trap path a failed array allocation does.
        Newobj => {
            let Some(shape) = plan.newobj_shapes.get(&ins.offset).copied() else {
                return Err(CompileError::Unsupported(
                    "newobj of a class this backend does not allocate".into(),
                ));
            };
            b.alloc_object(t0, shape);
            write_slot(b, depth, t0);
        }

        // The null reference. `Handle::NULL` is the all-zero value, so this is
        // the same instruction as pushing the integer zero — but it is spelled
        // separately because the two mean different things and a backend that
        // conflated them would be relying on a coincidence.
        Ldnull => {
            b.load_imm(t0, 0);
            write_slot(b, depth, t0);
        }

        // Instance field access on a heap object. The receiver is a handle —
        // an opaque token — and the value crosses as one 64-bit word, so no
        // part of the heap's layout is assumed here.
        Ldfld => {
            let Some((slot, kind)) = plan.fields.get(&ins.offset).copied() else {
                return Err(CompileError::Unsupported(
                    "a field this backend cannot resolve".into(),
                ));
            };
            read_slot(b, t0, depth - 1);
            b.field_load(t0, t0, slot, kind);
            write_slot(b, depth - 1, t0);
        }

        Stfld => {
            let Some((slot, kind)) = plan.fields.get(&ins.offset).copied() else {
                return Err(CompileError::Unsupported(
                    "a field this backend cannot resolve".into(),
                ));
            };
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.field_store(t0, slot, kind, t1);
        }

        // A virtual call every one of whose answers is known. The receiver is
        // consumed and the constant its type produces takes its place, so the
        // stack behaves exactly as the call would have.
        Callvirt => {
            let Some(cases) = plan.devirt.get(&ins.offset) else {
                return Err(CompileError::Unsupported(
                    "a virtual call whose targets are not all known".into(),
                ));
            };
            read_slot(b, t0, depth - 1);
            b.devirt_dispatch(t0, t0, cases);
            write_slot(b, depth - 1, t0);
        }

        // `bool[]` element access — one byte, zero-extended on the way in.
        LdelemU1 => {
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.array_load_u8(t0, t0, t1);
            write_slot(b, depth - 2, t0);
        }

        StelemI1 => {
            read_slot(b, t0, depth - 3);
            read_slot(b, t1, depth - 2);
            read_slot(b, t2, depth - 1);
            b.array_store_u8(t0, t1, t2);
        }

        // `double[]` element access. The index is bounds-checked exactly as the
        // `int[]` form is; only the element width and the absence of sign
        // extension differ.
        LdelemR8 => {
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.array_load_f64(t0, t0, t1);
            write_slot(b, depth - 2, t0);
        }

        StelemR8 => {
            read_slot(b, t0, depth - 3);
            read_slot(b, t1, depth - 2);
            read_slot(b, t2, depth - 1);
            b.array_store_f64(t0, t1, t2);
        }

        // `conv.r8` from an integer converts; from a double it is already one.
        ConvR8 | ConvR4 => {
            if floats.is_float_op(ins.offset) {
                read_slot(b, t0, depth - 1);
                b.int_to_float(t0, t0);
                write_slot(b, depth - 1, t0);
            }
        }

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
            if floats.is_float_op(ins.offset) {
                b.float_compare(cond, t0, t0, t1);
            } else {
                b.compare(cond, t0, t0, t1);
            }
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
            if floats.is_float_op(ins.offset) {
                b.float_branch_compare(cond, t0, t1, target);
            } else {
                b.branch_compare(cond, t0, t1, target);
            }
        }

        // -- arrays ---------------------------------------------------
        //
        // The array operand is always an argument here, because
        // `shape_is_compilable_with` only admits arrays that arrive as
        // parameters. That is what lets the descriptor live in an argument
        // slot and the index arithmetic stay in registers.
        Ldlen => {
            read_slot(b, t0, depth - 1);
            b.array_length(t0, t0);
            write_slot(b, depth - 1, t0);
        }

        LdelemI4 => {
            read_slot(b, t0, depth - 2);
            read_slot(b, t1, depth - 1);
            b.array_load_i32(t0, t0, t1);
            write_slot(b, depth - 2, t0);
        }

        StelemI4 => {
            read_slot(b, t0, depth - 3);
            read_slot(b, t1, depth - 2);
            read_slot(b, t2, depth - 1);
            b.array_store_i32(t0, t1, t2);
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

/// Where a method must poll for a collection, and `None` if it cannot.
///
/// A safe point goes at the target of every backward branch — the top of each
/// loop body — because a loop is the only way a compiled method can run long
/// enough for another thread to be waiting on it, and the only way it can
/// allocate without bound.
///
/// The requirement that makes the frame describable is that the **evaluation
/// stack is empty** at each of those offsets. A handle sitting in a spill slot
/// would have to be found by inference over the instruction stream; a handle in
/// a local or an argument is found from its declared type. Roslyn emits loop
/// back-edges at depth zero, so the restriction costs nothing in practice and
/// removes a whole analysis that could be wrong.
///
/// `None` means some back-edge lands where the stack is not empty. The method is
/// then compiled without safe points — and therefore may not allocate in a loop,
/// which is the rule that was there before any of this.
pub fn safepoints(
    instructions: &[Instruction],
    analysis: &MethodAnalysis,
) -> Option<BTreeSet<u32>> {
    let mut sites = BTreeSet::new();
    for ins in instructions {
        let Some(target) = ins.operand.as_target() else { continue };
        if target > ins.offset {
            continue;
        }
        if *analysis.depth_at.get(&target).unwrap_or(&0) != 0 {
            return None;
        }
        sites.insert(target);
    }
    Some(sites)
}

/// Byte offsets, from the frame pointer, of every slot that holds a handle.
///
/// Derived from declared types alone: a class-typed local or argument holds a
/// handle, and everything else does not. See [`is_reference_sig`] for why an
/// array slot is deliberately absent.
///
/// `local_offset` and `arg_offset` come from the backend, because where a slot
/// lives is the backend's business and this pass only decides *which* slots
/// matter.
pub fn reference_slots(
    params: &[rustclr_metadata::TypeSig],
    locals: &[rustclr_metadata::TypeSig],
    mut local_offset: impl FnMut(usize) -> i32,
    mut arg_offset: impl FnMut(usize) -> Option<i32>,
) -> Vec<i32> {
    let mut out = Vec::new();
    for (i, sig) in params.iter().enumerate() {
        if is_reference_sig(sig) {
            if let Some(offset) = arg_offset(i) {
                out.push(offset);
            }
        }
    }
    for (i, sig) in locals.iter().enumerate() {
        if is_reference_sig(sig) {
            out.push(local_offset(i));
        }
    }
    out
}
