//! Working out which IL instructions operate on floating point.
//!
//! IL's arithmetic opcodes are polymorphic: `add` is `add` whether its operands
//! are integers or doubles, and the difference lives in the *types* that
//! reached it. An interpreter can look at the values it holds; a compiler has
//! to know before it emits anything.
//!
//! So this walks the instruction stream keeping an abstract stack of kinds —
//! integer or float — in exactly the way the real stack will be shaped, and
//! records for each arithmetic instruction which one it is.
//!
//! # Why a whole pass rather than a guess
//!
//! Emitting integer `add` for a double produces a number, silently, and it is
//! the wrong number. That is the failure this runtime refuses everywhere else,
//! and a JIT is the worst place to allow it. When the walk cannot follow the
//! stream — an opcode it does not model, a branch whose two paths disagree
//! about a slot's kind — the method is declined and the interpreter runs it.
//!
//! # Storage is unchanged
//!
//! A `double` is eight bytes and so is an evaluation-stack slot, so floats live
//! in the same slots and the same general-purpose registers as integers, as bit
//! patterns. Only arithmetic and conversion need floating-point registers. That
//! is why this pass is all that was needed rather than a second register
//! allocator.

use std::collections::{BTreeMap, BTreeSet};

use rustclr_core::opcode::{Instruction, Op};
use rustclr_metadata::TypeSig;

/// What a stack slot holds, as far as the compiler needs to care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Int,
    Float,
}

/// Which instructions are floating-point, by IL offset.
#[derive(Debug, Default, Clone)]
pub struct FloatMap {
    /// Offsets whose operands are floats.
    float_ops: BTreeSet<u32>,
    /// True when anything in the method is floating-point at all.
    pub any: bool,
}

impl FloatMap {
    /// Whether the instruction at `offset` operates on floats.
    pub fn is_float_op(&self, offset: u32) -> bool {
        self.float_ops.contains(&offset)
    }
}

/// Whether a signature is `float` or `double`.
pub fn is_float_sig(sig: &TypeSig) -> bool {
    matches!(sig.unwrap_modifiers(), TypeSig::R4 | TypeSig::R8)
}

/// Infers the kind of every arithmetic instruction's operands.
///
/// `None` when the stream cannot be followed, which means the method is not
/// compilable rather than compilable-with-a-guess.
pub fn infer(
    instructions: &[Instruction],
    params: &[TypeSig],
    locals: &[TypeSig],
    return_type: &TypeSig,
) -> Option<FloatMap> {
    let kind_of = |sig: &TypeSig| if is_float_sig(sig) { Kind::Float } else { Kind::Int };
    let param_kinds: Vec<Kind> = params.iter().map(kind_of).collect();
    let local_kinds: Vec<Kind> = locals.iter().map(kind_of).collect();

    let mut map = FloatMap::default();
    map.any = param_kinds.contains(&Kind::Float)
        || local_kinds.contains(&Kind::Float)
        || is_float_sig(return_type);

    // The kind of each stack slot, as a stack. A branch target is reached with
    // whatever the fall-through path had; a stream where that disagrees is
    // declined below rather than resolved by guessing.
    let mut stack: Vec<Kind> = Vec::new();
    let mut at_leader: BTreeMap<u32, Vec<Kind>> = BTreeMap::new();

    for ins in instructions {
        // A jump target's shape is whatever reached it. If this offset was
        // recorded by an earlier branch, adopt it; a disagreement is a decline.
        if let Some(expected) = at_leader.get(&ins.offset) {
            if !stack.is_empty() && stack != *expected {
                return None;
            }
            if stack.is_empty() {
                stack = expected.clone();
            }
        }

        let pop = |n: usize, stack: &mut Vec<Kind>| -> Option<Vec<Kind>> {
            if stack.len() < n {
                return None;
            }
            Some(stack.split_off(stack.len() - n))
        };

        use Op::*;
        match ins.op {
            Nop | Break => {}

            LdcI4M1 | LdcI40 | LdcI41 | LdcI42 | LdcI43 | LdcI44 | LdcI45 | LdcI46 | LdcI47
            | LdcI48 | LdcI4S | LdcI4 | LdcI8 => stack.push(Kind::Int),
            LdcR4 | LdcR8 => {
                stack.push(Kind::Float);
                map.any = true;
            }

            Ldarg0 | Ldarg1 | Ldarg2 | Ldarg3 | LdargS | Ldarg => {
                let index = arg_index(ins)?;
                stack.push(*param_kinds.get(index)?);
            }
            StargS | Starg => {
                pop(1, &mut stack)?;
            }
            Ldloc0 | Ldloc1 | Ldloc2 | Ldloc3 | LdlocS | Ldloc => {
                let index = local_index(ins)?;
                stack.push(*local_kinds.get(index)?);
            }
            Stloc0 | Stloc1 | Stloc2 | Stloc3 | StlocS | Stloc => {
                pop(1, &mut stack)?;
            }

            Dup => {
                let top = *stack.last()?;
                stack.push(top);
            }
            Pop => {
                pop(1, &mut stack)?;
            }

            // The polymorphic ones. Both operands share a kind in valid IL;
            // mixed operands are invalid, so a disagreement is declined.
            Add | Sub | Mul | Div | Rem | AddOvf | AddOvfUn | SubOvf | SubOvfUn | MulOvf
            | MulOvfUn | DivUn | RemUn => {
                let operands = pop(2, &mut stack)?;
                let kind = match (operands[0], operands[1]) {
                    (Kind::Float, Kind::Float) => Kind::Float,
                    (Kind::Int, Kind::Int) => Kind::Int,
                    _ => return None,
                };
                if kind == Kind::Float {
                    // Overflow-checked arithmetic has no floating-point form,
                    // and `rem` on doubles is IEEE remainder — which x86 has
                    // had no single instruction for since SSE replaced x87's
                    // `fprem`. Declining both is what keeps `float_binop` from
                    // ever meeting an operation it cannot emit.
                    if !matches!(ins.op, Add | Sub | Mul | Div) {
                        return None;
                    }
                    map.float_ops.insert(ins.offset);
                    map.any = true;
                }
                stack.push(kind);
            }
            Neg => {
                let kind = *stack.last()?;
                if kind == Kind::Float {
                    map.float_ops.insert(ins.offset);
                    map.any = true;
                }
            }
            And | Or | Xor | Shl | Shr | ShrUn | Not => {
                // Bitwise operations are integer-only; a float here is invalid.
                let n = if matches!(ins.op, Not) { 1 } else { 2 };
                if pop(n, &mut stack)?.contains(&Kind::Float) {
                    return None;
                }
                stack.push(Kind::Int);
            }

            ConvR4 | ConvR8 | ConvRUn => {
                let from = pop(1, &mut stack)?;
                if from[0] == Kind::Int {
                    map.float_ops.insert(ins.offset);
                }
                stack.push(Kind::Float);
                map.any = true;
            }
            ConvI1 | ConvI2 | ConvI4 | ConvI8 | ConvU1 | ConvU2 | ConvU4 | ConvU8 | ConvI
            | ConvU => {
                let from = pop(1, &mut stack)?;
                if from[0] == Kind::Float {
                    map.float_ops.insert(ins.offset);
                    map.any = true;
                }
                stack.push(Kind::Int);
            }

            // Comparisons consume their operands and produce an integer.
            Ceq | Cgt | CgtUn | Clt | CltUn => {
                let operands = pop(2, &mut stack)?;
                match (operands[0], operands[1]) {
                    (Kind::Float, Kind::Float) => {
                        map.float_ops.insert(ins.offset);
                        map.any = true;
                    }
                    (Kind::Int, Kind::Int) => {}
                    _ => return None,
                }
                stack.push(Kind::Int);
            }
            Beq | BeqS | BneUn | BneUnS | Bge | BgeS | Bgt | BgtS | Ble | BleS | Blt | BltS
            | BgeUn | BgeUnS | BgtUn | BgtUnS | BleUn | BleUnS | BltUn | BltUnS => {
                let operands = pop(2, &mut stack)?;
                match (operands[0], operands[1]) {
                    (Kind::Float, Kind::Float) => {
                        map.float_ops.insert(ins.offset);
                        map.any = true;
                    }
                    (Kind::Int, Kind::Int) => {}
                    _ => return None,
                }
                remember(&mut at_leader, ins, &stack)?;
            }
            Brtrue | BrtrueS | Brfalse | BrfalseS => {
                if pop(1, &mut stack)?.contains(&Kind::Float) {
                    return None;
                }
                remember(&mut at_leader, ins, &stack)?;
            }
            Br | BrS => {
                remember(&mut at_leader, ins, &stack)?;
                // Nothing falls through a branch; the next instruction's shape
                // comes from whatever jumps to it.
                stack.clear();
            }

            Ret => {
                if !stack.is_empty() {
                    pop(1, &mut stack)?;
                }
                stack.clear();
            }

            // Array element access, which the integer path already compiles.
            // Its element kind is whatever the opcode names.
            LdelemI4 => {
                pop(2, &mut stack)?;
                stack.push(Kind::Int);
            }
            StelemI4 => {
                pop(3, &mut stack)?;
            }
            // A `bool[]` element is an integer on the evaluation stack.
            LdelemU1 => {
                pop(2, &mut stack)?;
                stack.push(Kind::Int);
            }
            StelemI1 => {
                pop(3, &mut stack)?;
            }
            // A `double[]` element is a double, whatever the array reference
            // itself was — the opcode names the width.
            LdelemR8 => {
                pop(2, &mut stack)?;
                stack.push(Kind::Float);
                map.any = true;
            }
            StelemR8 => {
                let operands = pop(3, &mut stack)?;
                if operands[2] != Kind::Float {
                    return None;
                }
                map.any = true;
            }
            Ldlen => {
                pop(1, &mut stack)?;
                stack.push(Kind::Int);
            }
            // An array reference is a descriptor address in compiled code — a
            // machine word, so `Int` as far as this pass is concerned. What it
            // *points at* is decided by the element opcode, not by this.
            Newarr => {
                pop(1, &mut stack)?;
                stack.push(Kind::Int);
            }

            // Anything not modelled here means the walk cannot be trusted.
            _ => return None,
        }
    }

    Some(map)
}

/// Records the stack shape a branch delivers to its target.
fn remember(
    at_leader: &mut BTreeMap<u32, Vec<Kind>>,
    ins: &Instruction,
    stack: &[Kind],
) -> Option<()> {
    let target = ins.operand.as_target()?;
    match at_leader.get(&target) {
        // Two paths reaching one target must agree about what is on the stack.
        Some(existing) if existing != stack => None,
        _ => {
            at_leader.insert(target, stack.to_vec());
            Some(())
        }
    }
}

fn arg_index(ins: &Instruction) -> Option<usize> {
    Some(match ins.op {
        Op::Ldarg0 => 0,
        Op::Ldarg1 => 1,
        Op::Ldarg2 => 2,
        Op::Ldarg3 => 3,
        _ => ins.operand.as_var()? as usize,
    })
}

fn local_index(ins: &Instruction) -> Option<usize> {
    Some(match ins.op {
        Op::Ldloc0 => 0,
        Op::Ldloc1 => 1,
        Op::Ldloc2 => 2,
        Op::Ldloc3 => 3,
        _ => ins.operand.as_var()? as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_core::opcode::decode_all;

    const INT: TypeSig = TypeSig::I4;
    const DBL: TypeSig = TypeSig::R8;

    /// `ldarg.0; ldarg.1; add; ret`
    fn add_body() -> Vec<Instruction> {
        decode_all(&[0x02, 0x03, 0x58, 0x2A]).unwrap()
    }

    #[test]
    fn integer_add_is_not_marked_as_float() {
        let map = infer(&add_body(), &[INT, INT], &[], &INT).expect("follows");
        assert!(!map.any, "nothing here is floating point");
        assert!(!map.is_float_op(2), "the add is an integer add");
    }

    #[test]
    fn the_same_add_over_doubles_is_marked() {
        let map = infer(&add_body(), &[DBL, DBL], &[], &DBL).expect("follows");
        assert!(map.any);
        assert!(map.is_float_op(2), "the add at offset 2 is a float add");
    }

    /// The whole point of the pass: one opcode, two meanings, decided by the
    /// signature rather than by the instruction.
    #[test]
    fn the_signature_is_what_decides() {
        let ints = infer(&add_body(), &[INT, INT], &[], &INT).unwrap();
        let floats = infer(&add_body(), &[DBL, DBL], &[], &DBL).unwrap();
        assert_ne!(ints.is_float_op(2), floats.is_float_op(2));
    }

    #[test]
    fn mixing_an_int_and_a_double_is_declined() {
        // Invalid IL — and a compiler that picked one kind would be inventing
        // an answer rather than reporting that it cannot tell.
        assert!(infer(&add_body(), &[INT, DBL], &[], &DBL).is_none());
    }

    #[test]
    fn a_bitwise_operation_on_a_double_is_declined() {
        // ldarg.0; ldarg.1; and; ret
        let ins = decode_all(&[0x02, 0x03, 0x5F, 0x2A]).unwrap();
        assert!(infer(&ins, &[INT, INT], &[], &INT).is_some());
        assert!(infer(&ins, &[DBL, DBL], &[], &INT).is_none());
    }

    #[test]
    fn an_unmodelled_opcode_declines_rather_than_guessing() {
        // ldarg.0; ldarg.1; newobj <token>; ret — `newobj` is not modelled, so
        // the walk cannot know what the stack looks like afterwards.
        let ins = decode_all(&[0x02, 0x03, 0x73, 0, 0, 0, 0x0A, 0x2A]).unwrap();
        assert!(infer(&ins, &[INT, INT], &[], &INT).is_none());
    }

    #[test]
    fn converting_an_int_to_a_double_is_a_float_operation() {
        // ldarg.0; conv.r8; ret
        let ins = decode_all(&[0x02, 0x6C, 0x2A]).unwrap();
        let map = infer(&ins, &[INT], &[], &DBL).expect("follows");
        assert!(map.is_float_op(1), "conv.r8 from an int converts");
        assert!(map.any);
    }

    #[test]
    fn a_float_comparison_is_marked() {
        // ldarg.0; ldarg.1; ceq; ret
        let ins = decode_all(&[0x02, 0x03, 0xFE, 0x01, 0x2A]).unwrap();
        assert!(!infer(&ins, &[INT, INT], &[], &INT).unwrap().is_float_op(2));
        assert!(infer(&ins, &[DBL, DBL], &[], &INT).unwrap().is_float_op(2));
    }

    /// `rem` on doubles is IEEE remainder, which the x86-64 backend cannot
    /// emit — so the pass has to decline it rather than leave the backend to
    /// meet an operation it has no instruction for.
    #[test]
    fn float_remainder_is_declined() {
        // ldarg.0; ldarg.1; rem; ret
        let ins = decode_all(&[0x02, 0x03, 0x5D, 0x2A]).unwrap();
        assert!(infer(&ins, &[INT, INT], &[], &INT).is_some(), "integer rem is fine");
        assert!(infer(&ins, &[DBL, DBL], &[], &DBL).is_none());
    }

    #[test]
    fn a_double_local_makes_the_method_floating_point() {
        // ldarg.0; stloc.0; ldloc.0; ret
        let ins = decode_all(&[0x02, 0x0A, 0x06, 0x2A]).unwrap();
        assert!(infer(&ins, &[DBL], &[DBL], &DBL).unwrap().any);
        assert!(!infer(&ins, &[INT], &[INT], &INT).unwrap().any);
    }
}
