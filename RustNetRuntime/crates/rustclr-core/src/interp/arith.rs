//! Numeric semantics: promotion, arithmetic, conversion and comparison.
//!
//! ECMA-335 III.1.5 defines which operand pairs are legal and what the result
//! type is. Integer arithmetic wraps unless the opcode has an `.ovf` suffix;
//! division by zero throws rather than trapping.

use super::*;
use core::cmp::Ordering;

#[allow(unused_imports)]
use crate::prelude::*;

/// The common type two operands are promoted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Promoted {
    I32,
    I64,
    Native,
    Float,
    /// A managed pointer plus an integer, as in `&x + 1`.
    Pointer,
}

impl Interpreter {
    fn promote(&self, a: &Value, b: &Value) -> ExecResult<Promoted> {
        use Value::*;
        Ok(match (a, b) {
            (F(_), F(_)) => Promoted::Float,
            // Mixing F with an integer is invalid IL, but real compilers never
            // emit it; treat it as float so a mis-decode degrades gracefully.
            (F(_), _) | (_, F(_)) => Promoted::Float,
            (Ref(_), _) | (_, Ref(_)) => Promoted::Pointer,
            (I64(_), _) | (_, I64(_)) => Promoted::I64,
            (NativeInt(_), _) | (_, NativeInt(_)) => Promoted::Native,
            (I32(_), I32(_)) => Promoted::I32,
            (Null, _) | (_, Null) | (Obj(_), _) | (_, Obj(_)) => Promoted::Native,
            _ => {
                return Err(ExecutionError::InvalidProgram(format!(
                    "cannot apply an arithmetic operator to {} and {}",
                    a.kind_name(),
                    b.kind_name()
                )))
            }
        })
    }

    /// `add`, `sub`, `mul`, `div`, `rem` and their unsigned variants.
    pub(super) fn binary_numeric(&mut self, op: Op, unsigned: bool) -> ExecResult<()> {
        let (a, b) = self.pop2()?;
        let result = match self.promote(&a, &b)? {
            Promoted::Float => {
                let x = as_f(&a);
                let y = as_f(&b);
                Value::F(match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div => x / y,
                    // IEEE remainder as `rem` defines it for floats.
                    Op::Rem | Op::RemUn => x % y,
                    _ => return Err(self.unsupported_arith(op)),
                })
            }
            Promoted::I32 => {
                let x = a.as_i32().unwrap_or(0);
                let y = b.as_i32().unwrap_or(0);
                Value::I32(self.int32_op(op, x, y, unsigned)?)
            }
            Promoted::I64 | Promoted::Native | Promoted::Pointer => {
                let x = a.as_i64().unwrap_or(0);
                let y = b.as_i64().unwrap_or(0);
                let v = self.int64_op(op, x, y, unsigned)?;
                if self.promote(&a, &b)? == Promoted::I64 {
                    Value::I64(v)
                } else {
                    Value::NativeInt(v)
                }
            }
        };
        self.push(result);
        Ok(())
    }

    fn int32_op(&self, op: Op, x: i32, y: i32, unsigned: bool) -> ExecResult<i32> {
        Ok(match op {
            Op::Add => x.wrapping_add(y),
            Op::Sub => x.wrapping_sub(y),
            Op::Mul => x.wrapping_mul(y),
            Op::Div => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                // `int.MinValue / -1` overflows; the CLR throws OverflowException.
                if x == i32::MIN && y == -1 {
                    return Err(ExecutionError::overflow());
                }
                x / y
            }
            Op::DivUn => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                ((x as u32) / (y as u32)) as i32
            }
            Op::Rem => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                if x == i32::MIN && y == -1 {
                    return Ok(0);
                }
                x % y
            }
            Op::RemUn => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                ((x as u32) % (y as u32)) as i32
            }
            _ => {
                let _ = unsigned;
                return Err(self.unsupported_arith(op));
            }
        })
    }

    fn int64_op(&self, op: Op, x: i64, y: i64, unsigned: bool) -> ExecResult<i64> {
        Ok(match op {
            Op::Add => x.wrapping_add(y),
            Op::Sub => x.wrapping_sub(y),
            Op::Mul => x.wrapping_mul(y),
            Op::Div => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                if x == i64::MIN && y == -1 {
                    return Err(ExecutionError::overflow());
                }
                x / y
            }
            Op::DivUn => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                ((x as u64) / (y as u64)) as i64
            }
            Op::Rem => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                if x == i64::MIN && y == -1 {
                    return Ok(0);
                }
                x % y
            }
            Op::RemUn => {
                if y == 0 {
                    return Err(ExecutionError::divide_by_zero());
                }
                ((x as u64) % (y as u64)) as i64
            }
            _ => {
                let _ = unsigned;
                return Err(self.unsupported_arith(op));
            }
        })
    }

    /// `add.ovf`, `sub.ovf`, `mul.ovf` and their `.un` forms.
    pub(super) fn binary_checked(&mut self, op: Op, unsigned: bool) -> ExecResult<()> {
        let (a, b) = self.pop2()?;
        let wide = matches!(self.promote(&a, &b)?, Promoted::I64 | Promoted::Native);

        let result = if wide {
            let x = a.as_i64().unwrap_or(0);
            let y = b.as_i64().unwrap_or(0);
            let v = if unsigned {
                let (x, y) = (x as u64, y as u64);
                let r = match op {
                    Op::AddOvfUn => x.checked_add(y),
                    Op::SubOvfUn => x.checked_sub(y),
                    Op::MulOvfUn => x.checked_mul(y),
                    _ => return Err(self.unsupported_arith(op)),
                };
                r.ok_or_else(ExecutionError::overflow)? as i64
            } else {
                let r = match op {
                    Op::AddOvf => x.checked_add(y),
                    Op::SubOvf => x.checked_sub(y),
                    Op::MulOvf => x.checked_mul(y),
                    _ => return Err(self.unsupported_arith(op)),
                };
                r.ok_or_else(ExecutionError::overflow)?
            };
            Value::I64(v)
        } else {
            let x = a.as_i32().unwrap_or(0);
            let y = b.as_i32().unwrap_or(0);
            let v = if unsigned {
                let (x, y) = (x as u32, y as u32);
                let r = match op {
                    Op::AddOvfUn => x.checked_add(y),
                    Op::SubOvfUn => x.checked_sub(y),
                    Op::MulOvfUn => x.checked_mul(y),
                    _ => return Err(self.unsupported_arith(op)),
                };
                r.ok_or_else(ExecutionError::overflow)? as i32
            } else {
                let r = match op {
                    Op::AddOvf => x.checked_add(y),
                    Op::SubOvf => x.checked_sub(y),
                    Op::MulOvf => x.checked_mul(y),
                    _ => return Err(self.unsupported_arith(op)),
                };
                r.ok_or_else(ExecutionError::overflow)?
            };
            Value::I32(v)
        };

        self.push(result);
        Ok(())
    }

    /// `and`, `or`, `xor`.
    pub(super) fn binary_integer(&mut self, op: Op) -> ExecResult<()> {
        let (a, b) = self.pop2()?;
        let wide = matches!(self.promote(&a, &b)?, Promoted::I64 | Promoted::Native);
        let result = if wide {
            let x = a.as_i64().unwrap_or(0);
            let y = b.as_i64().unwrap_or(0);
            Value::I64(match op {
                Op::And => x & y,
                Op::Or => x | y,
                Op::Xor => x ^ y,
                _ => return Err(self.unsupported_arith(op)),
            })
        } else {
            let x = a.as_i32().unwrap_or(0);
            let y = b.as_i32().unwrap_or(0);
            Value::I32(match op {
                Op::And => x & y,
                Op::Or => x | y,
                Op::Xor => x ^ y,
                _ => return Err(self.unsupported_arith(op)),
            })
        };
        self.push(result);
        Ok(())
    }

    /// `shl`, `shr`, `shr.un`. The shift amount is masked to the operand width,
    /// matching x86 and what the CLR specifies as unspecified-but-conventional.
    pub(super) fn shift(&mut self, op: Op) -> ExecResult<()> {
        let (value, amount) = self.pop2()?;
        let n = amount.as_i32().unwrap_or(0);
        let result = match value {
            Value::I64(x) => {
                let s = (n as u32) & 63;
                Value::I64(match op {
                    Op::Shl => x.wrapping_shl(s),
                    Op::Shr => x.wrapping_shr(s),
                    Op::ShrUn => ((x as u64).wrapping_shr(s)) as i64,
                    _ => return Err(self.unsupported_arith(op)),
                })
            }
            Value::NativeInt(x) => {
                let s = (n as u32) & 63;
                Value::NativeInt(match op {
                    Op::Shl => x.wrapping_shl(s),
                    Op::Shr => x.wrapping_shr(s),
                    Op::ShrUn => ((x as u64).wrapping_shr(s)) as i64,
                    _ => return Err(self.unsupported_arith(op)),
                })
            }
            other => {
                let x = other.as_i32().unwrap_or(0);
                let s = (n as u32) & 31;
                Value::I32(match op {
                    Op::Shl => x.wrapping_shl(s),
                    Op::Shr => x.wrapping_shr(s),
                    Op::ShrUn => ((x as u32).wrapping_shr(s)) as i32,
                    _ => return Err(self.unsupported_arith(op)),
                })
            }
        };
        self.push(result);
        Ok(())
    }

    /// Unchecked `conv.*`.
    pub(super) fn convert(&mut self, op: Op, v: &Value) -> ExecResult<Value> {
        // `conv.i` / `conv.u` on a managed pointer is how C# turns a `fixed`
        // reference into a raw pointer. This runtime represents managed
        // pointers structurally — they have no address — so there is no honest
        // integer to produce. Silently yielding 0 would surface later as a
        // null dereference a long way from the cause.
        if matches!(v, Value::Ref(_)) && matches!(op, Op::ConvI | Op::ConvU) {
            return Err(ExecutionError::Unsupported(
                "converting a managed reference to a raw pointer; `unsafe` pointer arithmetic is not supported by this runtime"
                    .into(),
            ));
        }

        Ok(match op {
            Op::ConvI1 => Value::I32(to_i64(v) as i8 as i32),
            Op::ConvU1 => Value::I32(to_i64(v) as u8 as i32),
            Op::ConvI2 => Value::I32(to_i64(v) as i16 as i32),
            Op::ConvU2 => Value::I32(to_i64(v) as u16 as i32),
            Op::ConvI4 => Value::I32(to_i64(v) as i32),
            Op::ConvU4 => Value::I32(to_i64(v) as u32 as i32),
            Op::ConvI8 => Value::I64(to_i64(v)),
            Op::ConvU8 => Value::I64(to_u64(v) as i64),
            Op::ConvI => Value::NativeInt(to_i64(v)),
            Op::ConvU => Value::NativeInt(to_u64(v) as i64),
            // `conv.r4` rounds through single precision but keeps an F slot.
            Op::ConvR4 => Value::F(as_f(v) as f32 as f64),
            Op::ConvR8 => Value::F(as_f(v)),
            Op::ConvRUn => Value::F(to_u64(v) as f64),
            _ => return Err(self.unsupported_arith(op)),
        })
    }

    /// Checked `conv.ovf.*`, which throws instead of truncating.
    pub(super) fn convert_checked(&mut self, op: Op, v: &Value) -> ExecResult<Value> {
        // The `.un` forms treat the source as unsigned.
        let unsigned_source = matches!(
            op,
            Op::ConvOvfI1Un
                | Op::ConvOvfI2Un
                | Op::ConvOvfI4Un
                | Op::ConvOvfI8Un
                | Op::ConvOvfU1Un
                | Op::ConvOvfU2Un
                | Op::ConvOvfU4Un
                | Op::ConvOvfU8Un
                | Op::ConvOvfIUn
                | Op::ConvOvfUUn
        );

        if let Value::F(f) = v {
            if !f.is_finite() {
                return Err(ExecutionError::overflow());
            }
        }

        let as_i128: i128 = if unsigned_source {
            to_u64(v) as i128
        } else {
            to_i64(v) as i128
        };

        let fits = |lo: i128, hi: i128| -> ExecResult<i128> {
            if as_i128 < lo || as_i128 > hi {
                Err(ExecutionError::overflow())
            } else {
                Ok(as_i128)
            }
        };

        Ok(match op {
            Op::ConvOvfI1 | Op::ConvOvfI1Un => Value::I32(fits(-128, 127)? as i32),
            Op::ConvOvfU1 | Op::ConvOvfU1Un => Value::I32(fits(0, 255)? as i32),
            Op::ConvOvfI2 | Op::ConvOvfI2Un => Value::I32(fits(-32768, 32767)? as i32),
            Op::ConvOvfU2 | Op::ConvOvfU2Un => Value::I32(fits(0, 65535)? as i32),
            Op::ConvOvfI4 | Op::ConvOvfI4Un => {
                Value::I32(fits(i32::MIN as i128, i32::MAX as i128)? as i32)
            }
            Op::ConvOvfU4 | Op::ConvOvfU4Un => Value::I32(fits(0, u32::MAX as i128)? as u32 as i32),
            Op::ConvOvfI8 | Op::ConvOvfI8Un => {
                Value::I64(fits(i64::MIN as i128, i64::MAX as i128)? as i64)
            }
            Op::ConvOvfU8 | Op::ConvOvfU8Un => {
                Value::I64(fits(0, u64::MAX as i128)? as u64 as i64)
            }
            Op::ConvOvfI | Op::ConvOvfIUn => {
                Value::NativeInt(fits(i64::MIN as i128, i64::MAX as i128)? as i64)
            }
            Op::ConvOvfU | Op::ConvOvfUUn => {
                Value::NativeInt(fits(0, u64::MAX as i128)? as u64 as i64)
            }
            _ => return Err(self.unsupported_arith(op)),
        })
    }

    /// `ceq` and the `beq`/`bne.un` family.
    pub(super) fn compare_equal(&self, a: &Value, b: &Value) -> bool {
        use Value::*;
        match (a, b) {
            (Null, Null) => true,
            (Null, Obj(h)) | (Obj(h), Null) => h.is_null(),
            (Obj(x), Obj(y)) => x == y,
            (F(x), F(y)) => x == y,
            (F(x), other) | (other, F(x)) => *x == to_i64(other) as f64,
            (Ref(x), Ref(y)) => x == y,
            (FnPtr(x), FnPtr(y)) => x == y,
            _ => to_i64(a) == to_i64(b),
        }
    }

    /// Ordered comparison. Returns `None` when the operands are unordered,
    /// which for floats means at least one is NaN.
    pub(super) fn compare_ordered(
        &self,
        a: &Value,
        b: &Value,
        unsigned: bool,
    ) -> ExecResult<Option<Ordering>> {
        use Value::*;
        Ok(match (a, b) {
            (F(x), F(y)) => x.partial_cmp(y),
            (F(x), other) => x.partial_cmp(&(to_i64(other) as f64)),
            (other, F(y)) => (to_i64(other) as f64).partial_cmp(y),
            (Obj(x), Obj(y)) => Some(x.to_bits().cmp(&y.to_bits())),
            _ => {
                if unsigned {
                    Some(to_u64(a).cmp(&to_u64(b)))
                } else {
                    Some(to_i64(a).cmp(&to_i64(b)))
                }
            }
        })
    }

    /// Whether a two-operand conditional branch is taken.
    pub(super) fn conditional_branch_taken(
        &self,
        op: Op,
        a: &Value,
        b: &Value,
    ) -> ExecResult<bool> {
        use Op::*;
        Ok(match op {
            Beq | BeqS => self.compare_equal(a, b),
            BneUn | BneUnS => !self.compare_equal(a, b),

            Bge | BgeS => matches!(
                self.compare_ordered(a, b, false)?,
                Some(Ordering::Greater | Ordering::Equal)
            ),
            Bgt | BgtS => {
                self.compare_ordered(a, b, false)? == Some(Ordering::Greater)
            }
            Ble | BleS => matches!(
                self.compare_ordered(a, b, false)?,
                Some(Ordering::Less | Ordering::Equal)
            ),
            Blt | BltS => self.compare_ordered(a, b, false)? == Some(Ordering::Less),

            // The `.un` forms are unsigned for integers and "unordered counts
            // as true" for floats, which is how C# compiles `!(a < b)`.
            BgeUn | BgeUnS => match self.compare_ordered(a, b, true)? {
                Some(Ordering::Greater | Ordering::Equal) | None => true,
                _ => false,
            },
            BgtUn | BgtUnS => match self.compare_ordered(a, b, true)? {
                Some(Ordering::Greater) | None => true,
                _ => false,
            },
            BleUn | BleUnS => match self.compare_ordered(a, b, true)? {
                Some(Ordering::Less | Ordering::Equal) | None => true,
                _ => false,
            },
            BltUn | BltUnS => match self.compare_ordered(a, b, true)? {
                Some(Ordering::Less) | None => true,
                _ => false,
            },
            _ => return Err(self.unsupported_arith(op)),
        })
    }

    fn unsupported_arith(&self, op: Op) -> ExecutionError {
        ExecutionError::InvalidProgram(format!("`{}` used in an arithmetic position", op.name()))
    }
}

fn as_f(v: &Value) -> f64 {
    match v {
        Value::F(x) => *x,
        other => to_i64(other) as f64,
    }
}

fn to_i64(v: &Value) -> i64 {
    match v {
        Value::I32(x) => *x as i64,
        Value::I64(x) => *x,
        Value::NativeInt(x) => *x,
        Value::F(x) => *x as i64,
        Value::Null => 0,
        Value::Obj(h) => h.to_bits() as i64,
        _ => 0,
    }
}

fn to_u64(v: &Value) -> u64 {
    match v {
        Value::I32(x) => *x as u32 as u64,
        Value::I64(x) => *x as u64,
        Value::NativeInt(x) => *x as u64,
        Value::F(x) => *x as u64,
        Value::Null => 0,
        Value::Obj(h) => h.to_bits(),
        _ => 0,
    }
}
