//! `System.Decimal`: a 96-bit integer mantissa, a scale of 0–28 and a sign,
//! exactly as .NET defines it.
//!
//! Business code — prices, invoices, ledgers — reaches for `decimal` first, so
//! it has to be exact rather than a `double` underneath: `0.1m + 0.2m` is
//! `0.3`, trailing zeros are kept (`1.50m` prints `1.50`), and a result that
//! needs more than 28 digits is rounded half to even, as .NET rounds it.
//!
//! A decimal travels as a `Value::Struct` of the native `System.Decimal` type
//! with two 64-bit slots: the low 64 bits of the mantissa, and the high 32 bits
//! packed with the scale (bits 32–39) and the sign (bit 63). A struct with no
//! slots — what `default(decimal)` and a fresh field hold — is zero.

use rustclr_core::{ClrExceptionKind, ExecResult, ExecutionError, Interpreter, StructValue, TypeId, Value};

use crate::support::*;

#[allow(unused_imports)]
use crate::prelude::*;

// ── 256-bit unsigned arithmetic, enough for any intermediate ───────────────

type U = [u64; 4];

const ZERO_U: U = [0; 4];

fn u_from(v: u128) -> U {
    [v as u64, (v >> 64) as u64, 0, 0]
}

fn u_low(v: &U) -> u128 {
    v[0] as u128 | (v[1] as u128) << 64
}

fn u_is_zero(v: &U) -> bool {
    v.iter().all(|x| *x == 0)
}

fn u_cmp(a: &U, b: &U) -> core::cmp::Ordering {
    for i in (0..4).rev() {
        match a[i].cmp(&b[i]) {
            core::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    core::cmp::Ordering::Equal
}

fn u_add(a: &U, b: &U) -> U {
    let mut out = ZERO_U;
    let mut carry = 0u128;
    for i in 0..4 {
        let s = a[i] as u128 + b[i] as u128 + carry;
        out[i] = s as u64;
        carry = s >> 64;
    }
    out
}

/// `a - b`, for `a >= b`.
fn u_sub(a: &U, b: &U) -> U {
    let mut out = ZERO_U;
    let mut borrow = 0i128;
    for i in 0..4 {
        let d = a[i] as i128 - b[i] as i128 - borrow;
        if d < 0 {
            out[i] = (d + (1i128 << 64)) as u64;
            borrow = 1;
        } else {
            out[i] = d as u64;
            borrow = 0;
        }
    }
    out
}

fn u_mul_small(a: &U, m: u64) -> U {
    let mut out = ZERO_U;
    let mut carry = 0u128;
    for i in 0..4 {
        let p = a[i] as u128 * m as u128 + carry;
        out[i] = p as u64;
        carry = p >> 64;
    }
    out
}

fn u_mul_pow10(a: &U, mut n: u32) -> U {
    let mut out = *a;
    while n > 0 {
        let step = n.min(19);
        out = u_mul_small(&out, 10u64.pow(step));
        n -= step;
    }
    out
}

fn u_mul(a: u128, b: u128) -> U {
    let (a0, a1) = (a as u64 as u128, (a >> 64) as u64 as u128);
    let (b0, b1) = (b as u64 as u128, (b >> 64) as u64 as u128);
    let mut out = ZERO_U;
    let mut add_at = |v: u128, at: usize| {
        let mut carry = v;
        let mut i = at;
        while carry != 0 && i < 4 {
            let s = out[i] as u128 + (carry as u64) as u128;
            out[i] = s as u64;
            carry = (carry >> 64) + (s >> 64);
            i += 1;
        }
    };
    add_at(a0 * b0, 0);
    add_at(a0 * b1, 1);
    add_at(a1 * b0, 1);
    add_at(a1 * b1, 2);
    out
}

fn u_divrem_small(a: &U, d: u64) -> (U, u64) {
    let mut out = ZERO_U;
    let mut rem = 0u128;
    for i in (0..4).rev() {
        let cur = (rem << 64) | a[i] as u128;
        out[i] = (cur / d as u128) as u64;
        rem = cur % d as u128;
    }
    (out, rem as u64)
}

fn u_shl1(a: &U) -> U {
    let mut out = ZERO_U;
    for i in (0..4).rev() {
        out[i] = a[i] << 1 | if i > 0 { a[i - 1] >> 63 } else { 0 };
    }
    out
}

/// Long division, bit by bit. Decimal division is rare enough for this.
fn u_divrem(n: &U, d: &U) -> (U, U) {
    let mut q = ZERO_U;
    let mut r = ZERO_U;
    for bit in (0..256).rev() {
        r = u_shl1(&r);
        r[0] |= (n[bit / 64] >> (bit % 64)) & 1;
        if u_cmp(&r, d) != core::cmp::Ordering::Less {
            r = u_sub(&r, d);
            q[bit / 64] |= 1 << (bit % 64);
        }
    }
    (q, r)
}

fn fits96(v: &U) -> bool {
    v[3] == 0 && v[2] == 0 && v[1] >> 32 == 0
}

fn pow10(n: u32) -> u128 {
    10u128.pow(n)
}

// ── the decimal itself ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dec {
    pub neg: bool,
    pub scale: u32,
    pub mant: u128,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecError {
    Overflow,
    DivideByZero,
}

pub const MAX_MANT: u128 = (1u128 << 96) - 1;

impl Dec {
    pub const ZERO: Dec = Dec { neg: false, scale: 0, mant: 0 };

    pub fn from_i64(v: i64) -> Dec {
        Dec { neg: v < 0, scale: 0, mant: v.unsigned_abs() as u128 }
    }

    pub fn from_u64(v: u64) -> Dec {
        Dec { neg: false, scale: 0, mant: v as u128 }
    }

    pub fn is_zero(&self) -> bool {
        self.mant == 0
    }

    /// Rounds a 256-bit magnitude at `scale` into range: at most 28 decimal
    /// places and a 96-bit mantissa, rounding half to even.
    fn fit(neg: bool, mut scale: u32, mut w: U) -> Result<Dec, DecError> {
        let (mut last, mut sticky, mut removed) = (0u64, false, false);
        while scale > 28 || !fits96(&w) {
            if scale == 0 {
                return Err(DecError::Overflow);
            }
            sticky |= last != 0;
            let (q, r) = u_divrem_small(&w, 10);
            w = q;
            last = r;
            scale -= 1;
            removed = true;
        }
        if removed && (last > 5 || (last == 5 && (sticky || w[0] & 1 == 1))) {
            w = u_add(&w, &u_from(1));
            if !fits96(&w) {
                if scale == 0 {
                    return Err(DecError::Overflow);
                }
                w = u_divrem_small(&w, 10).0;
                scale -= 1;
            }
        }
        let mant = u_low(&w);
        Ok(Dec { neg: neg && mant != 0, scale, mant })
    }

    fn aligned(&self, other: &Dec) -> (U, U, u32) {
        let s = self.scale.max(other.scale);
        (
            u_mul_pow10(&u_from(self.mant), s - self.scale),
            u_mul_pow10(&u_from(other.mant), s - other.scale),
            s,
        )
    }

    pub fn add(&self, other: &Dec) -> Result<Dec, DecError> {
        let (a, b, s) = self.aligned(other);
        if self.neg == other.neg {
            return Dec::fit(self.neg, s, u_add(&a, &b));
        }
        match u_cmp(&a, &b) {
            core::cmp::Ordering::Less => Dec::fit(other.neg, s, u_sub(&b, &a)),
            _ => Dec::fit(self.neg, s, u_sub(&a, &b)),
        }
    }

    pub fn neg(&self) -> Dec {
        Dec { neg: !self.neg && self.mant != 0, ..*self }
    }

    pub fn sub(&self, other: &Dec) -> Result<Dec, DecError> {
        self.add(&other.neg())
    }

    pub fn mul(&self, other: &Dec) -> Result<Dec, DecError> {
        Dec::fit(self.neg != other.neg, self.scale + other.scale, u_mul(self.mant, other.mant))
    }

    /// As .NET: exact when it can be, otherwise as many digits as fit,
    /// rounded half to even.
    pub fn div(&self, other: &Dec) -> Result<Dec, DecError> {
        if other.mant == 0 {
            return Err(DecError::DivideByZero);
        }
        let neg = self.neg != other.neg;
        let mut scale = self.scale as i32 - other.scale as i32;
        let mut num = u_from(self.mant);
        if scale < 0 {
            num = u_mul_pow10(&num, (-scale) as u32);
            scale = 0;
        }
        let d = u_from(other.mant);
        let (mut q, mut r) = u_divrem(&num, &d);
        if !fits96(&q) {
            return Dec::fit(neg, scale as u32, q);
        }
        while !u_is_zero(&r) && scale < 28 {
            let (dq, dr) = u_divrem(&u_mul_small(&r, 10), &d);
            let candidate = u_add(&u_mul_small(&q, 10), &dq);
            if !fits96(&candidate) {
                break;
            }
            q = candidate;
            r = dr;
            scale += 1;
        }
        if !u_is_zero(&r) {
            let up = match u_cmp(&u_mul_small(&r, 2), &d) {
                core::cmp::Ordering::Greater => true,
                core::cmp::Ordering::Equal => q[0] & 1 == 1,
                core::cmp::Ordering::Less => false,
            };
            if up {
                q = u_add(&q, &u_from(1));
            }
        }
        Dec::fit(neg, scale as u32, q)
    }

    pub fn rem(&self, other: &Dec) -> Result<Dec, DecError> {
        if other.mant == 0 {
            return Err(DecError::DivideByZero);
        }
        let (a, b, s) = self.aligned(other);
        let (_, r) = u_divrem(&a, &b);
        Dec::fit(self.neg, s, r)
    }

    pub fn cmp(&self, other: &Dec) -> core::cmp::Ordering {
        use core::cmp::Ordering::*;
        match (self.is_zero(), other.is_zero()) {
            (true, true) => return Equal,
            (true, false) => return if other.neg { Greater } else { Less },
            (false, true) => return if self.neg { Less } else { Greater },
            _ => {}
        }
        if self.neg != other.neg {
            return if self.neg { Less } else { Greater };
        }
        let (a, b, _) = self.aligned(other);
        let magnitude = u_cmp(&a, &b);
        if self.neg { magnitude.reverse() } else { magnitude }
    }

    /// `Math.Round(d, decimals, mode)` with MidpointRounding's values:
    /// 0 ToEven, 1 AwayFromZero, 2 ToZero, 3 ToNegativeInfinity, 4 ToPositiveInfinity.
    pub fn round(&self, decimals: u32, mode: i32) -> Dec {
        if self.scale <= decimals {
            return *self;
        }
        let p = pow10(self.scale - decimals);
        let (q, r) = (self.mant / p, self.mant % p);
        let twice = r * 2;
        let up = match mode {
            1 => twice >= p,
            2 => false,
            3 => r != 0 && self.neg,
            4 => r != 0 && !self.neg,
            _ => twice > p || (twice == p && q & 1 == 1),
        };
        let mant = q + up as u128;
        Dec { neg: self.neg && mant != 0, scale: decimals, mant }
    }

    /// Trailing zeros removed: `1.500m` → `1.5`. For hashing and conversions.
    pub fn normalized(&self) -> Dec {
        let mut d = *self;
        while d.scale > 0 && d.mant % 10 == 0 {
            d.mant /= 10;
            d.scale -= 1;
        }
        d
    }

    pub fn to_f64(&self) -> f64 {
        self.to_string().parse::<f64>().unwrap_or(0.0)
    }

    /// Truncated toward zero, as a cast does.
    pub fn to_i128(&self) -> i128 {
        let whole = (self.mant / pow10(self.scale)) as i128;
        if self.neg { -whole } else { whole }
    }

    /// From a double, keeping 15 significant digits as .NET does: `0.1` is `0.1`.
    pub fn from_f64(v: f64, digits: usize) -> Result<Dec, DecError> {
        if !v.is_finite() || v.abs() >= 7.922_816_251_426_434e28 {
            return Err(DecError::Overflow);
        }
        if v == 0.0 {
            return Ok(Dec::ZERO);
        }
        let text = format!("{:.*e}", digits - 1, v);
        let parsed = Dec::parse(&text).ok_or(DecError::Overflow)?;
        Ok(parsed.normalized())
    }

    /// Invariant-culture parsing: sign, digits, one point, `,` group
    /// separators, and an optional exponent.
    pub fn parse(text: &str) -> Option<Dec> {
        let t = text.trim();
        let (neg, body) = match t.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let (number, exponent) = match body.find(|c| c == 'e' || c == 'E') {
            Some(at) => (&body[..at], body[at + 1..].parse::<i32>().ok()?),
            None => (body, 0),
        };
        let mut w = ZERO_U;
        let (mut scale, mut seen_point, mut digits, mut dropped) = (0i32, false, 0usize, 0i32);
        for c in number.chars() {
            match c {
                '0'..='9' => {
                    if digits < 60 {
                        w = u_add(&u_mul_small(&w, 10), &u_from(c as u128 - '0' as u128));
                        if digits > 0 || c != '0' {
                            digits += 1;
                        }
                        if seen_point {
                            scale += 1;
                        }
                    } else if !seen_point {
                        dropped += 1;
                    }
                }
                '.' if !seen_point => seen_point = true,
                ',' if !seen_point => {}
                _ => return None,
            }
        }
        if number.is_empty() || number == "." {
            return None;
        }
        scale -= exponent + dropped;
        if scale < 0 {
            w = u_mul_pow10(&w, (-scale) as u32);
            scale = 0;
        }
        Dec::fit(neg, scale as u32, w).ok()
    }

    /// The digits with the scale's places after the point, no grouping.
    pub fn plain(&self) -> String {
        let digits = self.mant.to_string();
        let s = self.scale as usize;
        let mut out = String::new();
        if self.neg && self.mant != 0 {
            out.push('-');
        }
        if s == 0 {
            out.push_str(&digits);
        } else if digits.len() > s {
            out.push_str(&digits[..digits.len() - s]);
            out.push('.');
            out.push_str(&digits[digits.len() - s..]);
        } else {
            out.push_str("0.");
            for _ in 0..s - digits.len() {
                out.push('0');
            }
            out.push_str(&digits);
        }
        out
    }

    /// `ToString(spec)`. `F`, `N` and `P` are exact; other formats go
    /// through `double`, which is exact to 15 significant digits.
    pub fn format(&self, spec: &str) -> String {
        let (kind, precision) = match spec.chars().next() {
            Some(k) if spec.len() <= 3 && spec[1..].chars().all(|c| c.is_ascii_digit()) => {
                (k.to_ascii_uppercase(), spec[1..].parse::<u32>().ok())
            }
            _ => (' ', None),
        };
        match (spec, kind) {
            ("", _) | (_, 'G') if precision.is_none() => self.plain(),
            (_, 'F') => self.round(precision.unwrap_or(2), 1).padded(precision.unwrap_or(2)).plain(),
            (_, 'N') => group(&self.round(precision.unwrap_or(2), 1).padded(precision.unwrap_or(2)).plain()),
            (_, 'P') => {
                let hundred = Dec::from_i64(100);
                let v = self.mul(&hundred).unwrap_or(*self);
                format!("{} %", group(&v.round(precision.unwrap_or(2), 1).padded(precision.unwrap_or(2)).plain()))
            }
            _ => crate::format::format_f64(self.to_f64(), spec).unwrap_or_else(|| self.plain()),
        }
    }

    /// Zeros appended so exactly `places` digits follow the point.
    fn padded(&self, places: u32) -> Dec {
        if self.scale >= places {
            return *self;
        }
        Dec { mant: self.mant * pow10(places - self.scale), scale: places, neg: self.neg }
    }

    pub fn hash(&self) -> i32 {
        let n = self.normalized();
        let mixed = (n.mant as u64) ^ ((n.mant >> 64) as u64).rotate_left(17) ^ (n.scale as u64).rotate_left(40);
        let v = if n.neg { !mixed } else { mixed };
        (v ^ (v >> 32)) as i32
    }
}

impl core::fmt::Display for Dec {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.plain())
    }
}

/// `1234567.50` → `1,234,567.50`.
fn group(plain: &str) -> String {
    let (sign, rest) = match plain.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", plain),
    };
    let (int, frac) = match rest.find('.') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let mut out = String::new();
    for (n, c) in int.chars().enumerate() {
        if n > 0 && (int.len() - n) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}{frac}")
}

// ── values ─────────────────────────────────────────────────────────────────

fn decimal_type(i: &Interpreter) -> Option<TypeId> {
    i.loader.registry.find_type_by_name("System.Decimal")
}

/// Whether a value is a decimal (unboxed).
pub fn is_decimal(i: &Interpreter, v: &Value) -> bool {
    match v {
        Value::Struct(s) => {
            let t = i.loader.registry.ty(s.type_id);
            t.name == "Decimal" && t.namespace == "System"
        }
        _ => false,
    }
}

pub fn decode(v: &Value) -> Dec {
    let Value::Struct(s) = v else { return Dec::ZERO };
    let lo = s.fields.first().and_then(|f| f.as_i64()).unwrap_or(0) as u64;
    let hi = s.fields.get(1).and_then(|f| f.as_i64()).unwrap_or(0) as u64;
    Dec { neg: hi >> 63 == 1, scale: ((hi >> 32) & 0xFF) as u32, mant: lo as u128 | ((hi & 0xFFFF_FFFF) as u128) << 64 }
}

pub fn encode(i: &Interpreter, d: Dec) -> Value {
    let type_id = decimal_type(i).unwrap_or(i.loader.core().object);
    let hi = ((d.mant >> 64) as u64 & 0xFFFF_FFFF) | (d.scale as u64) << 32 | ((d.neg && d.mant != 0) as u64) << 63;
    Value::Struct(Box::new(StructValue { type_id, fields: vec![Value::I64(d.mant as u64 as i64), Value::I64(hi as i64)] }))
}

/// A value as a decimal, converting numbers: what an operator's operand is.
pub fn to_dec(i: &mut Interpreter, v: &Value) -> ExecResult<Dec> {
    let v = match v {
        Value::Ref(r) => i.load_indirect_public(r.clone())?,
        Value::Obj(h) if !h.is_null() => crate::extras::unbox_value(i, &Value::Obj(*h)),
        other => other.clone(),
    };
    Ok(match &v {
        Value::Struct(_) => decode(&v),
        Value::I32(n) => Dec::from_i64(*n as i64),
        Value::I64(n) | Value::NativeInt(n) => Dec::from_i64(*n),
        Value::F(f) => Dec::from_f64(*f, 15).map_err(error)?,
        _ => Dec::ZERO,
    })
}

fn error(e: DecError) -> ExecutionError {
    match e {
        DecError::Overflow => ExecutionError::exception(ClrExceptionKind::Overflow, "Value was either too large or too small for a Decimal."),
        DecError::DivideByZero => ExecutionError::exception(ClrExceptionKind::DivideByZero, "Attempted to divide by zero."),
    }
}

fn dec_arg(i: &mut Interpreter, a: &[Value], n: usize) -> ExecResult<Dec> {
    let v = a.get(n).cloned().unwrap_or(Value::Null);
    to_dec(i, &v)
}

fn ret(i: &Interpreter, d: Dec) -> ExecResult<Option<Value>> {
    Ok(Some(encode(i, d)))
}

/// Writes a value-type constructor's result through `this` when it came as a
/// pointer (`ldloca; call .ctor`), and returns it for `newobj`.
fn construct(i: &mut Interpreter, a: &[Value], d: Dec) -> ExecResult<Option<Value>> {
    let value = encode(i, d);
    if let Some(Value::Ref(r)) = a.first() {
        i.store_indirect_public(r.clone(), value.clone())?;
    }
    Ok(Some(value))
}

fn param(i: &Interpreter, n: usize) -> String {
    crate::extras::param_type_name(i, n).unwrap_or_default()
}

fn returns(i: &Interpreter) -> String {
    crate::extras::return_type_name(i).unwrap_or_default()
}

/// A decimal converted to the numeric type a conversion returns.
fn convert_out(i: &Interpreter, d: Dec, target: &str) -> ExecResult<Option<Value>> {
    let whole = || d.to_i128();
    let checked = |lo: i128, hi: i128| -> ExecResult<i128> {
        let w = whole();
        if w < lo || w > hi {
            Err(ExecutionError::exception(ClrExceptionKind::Overflow, "Value was either too large or too small for the target type."))
        } else {
            Ok(w)
        }
    };
    Ok(Some(match target {
        "System.Double" | "System.Single" => Value::F(if target == "System.Single" { d.to_f64() as f32 as f64 } else { d.to_f64() }),
        "System.Int64" => Value::I64(checked(i64::MIN as i128, i64::MAX as i128)? as i64),
        "System.UInt64" => Value::I64(checked(0, u64::MAX as i128)? as u64 as i64),
        "System.UInt32" => Value::I32(checked(0, u32::MAX as i128)? as u32 as i32),
        "System.Int16" => Value::I32(checked(i16::MIN as i128, i16::MAX as i128)? as i32),
        "System.UInt16" | "System.Char" => Value::I32(checked(0, u16::MAX as i128)? as i32),
        "System.Byte" => Value::I32(checked(0, 255)? as i32),
        "System.SByte" => Value::I32(checked(-128, 127)? as i32),
        "System.Decimal" => encode(i, d),
        _ => Value::I32(checked(i32::MIN as i128, i32::MAX as i128)? as i32),
    }))
}

/// A number of the parameter's declared type, as a decimal.
fn convert_in(i: &mut Interpreter, v: &Value, from: &str) -> ExecResult<Dec> {
    let v = match v {
        Value::Ref(r) => i.load_indirect_public(r.clone())?,
        other => other.clone(),
    };
    Ok(match (from, &v) {
        ("System.UInt32", Value::I32(n)) => Dec::from_u64(*n as u32 as u64),
        ("System.UInt64", Value::I64(n)) => Dec::from_u64(*n as u64),
        ("System.Single", Value::F(f)) => Dec::from_f64(*f, 7).map_err(error)?,
        _ => to_dec(i, &v)?,
    })
}

pub fn register(interp: &mut Interpreter) {
    const D: &str = "System.Decimal";
    let key = |m: &str| -> &'static str { Box::leak(format!("{D}::{m}").into_boxed_str()) };

    // Constructors. `newobj` passes `this` as a pointer first.
    interp.register_native(key(".ctor/1"), |i, a| {
        let v = a.get(1).cloned().unwrap_or(Value::Null);
        // `new decimal(int[] bits)`: lo, mid, hi, flags.
        if let Value::Obj(h) = v {
            if !h.is_null() {
                let bits: Vec<i32> = crate::collections::elements(i, h).iter().map(|x| x.as_i32().unwrap_or(0)).collect();
                if bits.len() == 4 {
                    let mant = bits[0] as u32 as u128 | (bits[1] as u32 as u128) << 32 | (bits[2] as u32 as u128) << 64;
                    let d = Dec { mant, scale: ((bits[3] >> 16) & 0xFF) as u32, neg: bits[3] < 0 && mant != 0 };
                    return construct(i, a, d);
                }
            }
        }
        let from = param(i, 0);
        let d = convert_in(i, &v, &from)?;
        construct(i, a, d)
    });
    interp.register_native(key(".ctor/5"), |i, a| {
        let lo = arg_i32(i, a, 1)? as u32 as u128;
        let mid = arg_i32(i, a, 2)? as u32 as u128;
        let hi = arg_i32(i, a, 3)? as u32 as u128;
        let neg = arg(i, a, 4)?.is_truthy();
        let scale = arg_i32(i, a, 5)? as u32;
        if scale > 28 {
            return Err(ExecutionError::exception(ClrExceptionKind::ArgumentOutOfRange, "Decimal's scale value must be between 0 and 28, inclusive."));
        }
        let mant = lo | mid << 32 | hi << 64;
        construct(i, a, Dec { neg: neg && mant != 0, scale, mant })
    });

    // Conversions in and out.
    interp.register_native(key("op_Implicit/1"), |i, a| {
        let v = a.first().cloned().unwrap_or(Value::Null);
        let from = param(i, 0);
        let d = convert_in(i, &v, &from)?;
        ret(i, d)
    });
    for name in ["op_Explicit/1", "ToDouble/1", "ToSingle/1", "ToInt32/1", "ToInt64/1", "ToUInt32/1", "ToUInt64/1", "ToByte/1", "ToInt16/1"] {
        interp.register_native(key(name), |i, a| {
            let target = returns(i);
            let v = a.first().cloned().unwrap_or(Value::Null);
            if target == "System.Decimal" {
                let from = param(i, 0);
                let d = convert_in(i, &v, &from)?;
                return ret(i, d);
            }
            let d = to_dec(i, &v)?;
            convert_out(i, d, &target)
        });
    }

    // Arithmetic.
    macro_rules! binary {
        ($names:expr, $op:expr) => {
            for name in $names {
                interp.register_native(key(name), |i, a| {
                    let (x, y) = (dec_arg(i, a, 0)?, dec_arg(i, a, 1)?);
                    let f: fn(&Dec, &Dec) -> Result<Dec, DecError> = $op;
                    let r = f(&x, &y).map_err(error)?;
                    ret(i, r)
                });
            }
        };
    }
    binary!(["op_Addition/2", "Add/2"], |x, y| x.add(y));
    binary!(["op_Subtraction/2", "Subtract/2"], |x, y| x.sub(y));
    binary!(["op_Multiply/2", "Multiply/2"], |x, y| x.mul(y));
    binary!(["op_Division/2", "Divide/2"], |x, y| x.div(y));
    binary!(["op_Modulus/2", "Remainder/2"], |x, y| x.rem(y));
    for name in ["op_UnaryNegation/1", "Negate/1"] {
        interp.register_native(key(name), |i, a| {
            let x = dec_arg(i, a, 0)?;
            ret(i, x.neg())
        });
    }
    interp.register_native(key("op_UnaryPlus/1"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        ret(i, x)
    });
    interp.register_native(key("op_Increment/1"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        let r = x.add(&Dec::from_i64(1)).map_err(error)?;
        ret(i, r)
    });
    interp.register_native(key("op_Decrement/1"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        let r = x.sub(&Dec::from_i64(1)).map_err(error)?;
        ret(i, r)
    });

    // Comparison.
    macro_rules! compare {
        ($name:expr, $test:expr) => {
            interp.register_native(key($name), |i, a| {
                let (x, y) = (dec_arg(i, a, 0)?, dec_arg(i, a, 1)?);
                let f: fn(core::cmp::Ordering) -> bool = $test;
                Ok(Some(Value::I32(f(x.cmp(&y)) as i32)))
            });
        };
    }
    compare!("op_Equality/2", |o| o.is_eq());
    compare!("op_Inequality/2", |o| o.is_ne());
    compare!("op_LessThan/2", |o| o.is_lt());
    compare!("op_GreaterThan/2", |o| o.is_gt());
    compare!("op_LessThanOrEqual/2", |o| o.is_le());
    compare!("op_GreaterThanOrEqual/2", |o| o.is_ge());
    // Static Compare/Equals, and the instance forms on `this`.
    interp.register_native(key("Compare/2"), |i, a| {
        let (x, y) = (dec_arg(i, a, 0)?, dec_arg(i, a, 1)?);
        Ok(Some(Value::I32(x.cmp(&y) as i32)))
    });
    interp.register_native(key("CompareTo/1"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        let other = arg(i, a, 1)?;
        if other.is_null() {
            return Ok(Some(Value::I32(1)));
        }
        let y = to_dec(i, &other)?;
        Ok(Some(Value::I32(x.cmp(&y) as i32)))
    });
    interp.register_native(key("Equals/1"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        let other = arg(i, a, 1)?;
        let other = crate::extras::unbox_value(i, &other);
        Ok(Some(Value::I32((is_decimal(i, &other) && decode(&other).cmp(&x).is_eq()) as i32)))
    });
    interp.register_native(key("Equals/2"), |i, a| {
        let (x, y) = (dec_arg(i, a, 0)?, dec_arg(i, a, 1)?);
        Ok(Some(Value::I32(x.cmp(&y).is_eq() as i32)))
    });
    interp.register_native(key("GetHashCode()"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        Ok(Some(Value::I32(x.hash())))
    });
    interp.register_native(key("get_Scale()"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        Ok(Some(Value::I32(x.scale as i32)))
    });

    // Rounding, on Decimal and on Math.
    for owner in [D, "System.Math"] {
        let k = |m: &str| -> &'static str { Box::leak(format!("{owner}::{m}").into_boxed_str()) };
        interp.register_native(k("Round/1"), |i, a| {
            let v = arg(i, a, 0)?;
            if !is_decimal(i, &v) {
                return Ok(Some(Value::F(crate::extras::round_to(v.as_f64().unwrap_or(0.0), 0, false))));
            }
            let r = decode(&v).round(0, 0);
            ret(i, r)
        });
        // Round(x, int decimals) or Round(x, MidpointRounding).
        interp.register_native(k("Round/2"), |i, a| {
            let v = arg(i, a, 0)?;
            let second = arg_i32(i, a, 1)?;
            let mode_given = param(i, 1) == "System.MidpointRounding";
            if !is_decimal(i, &v) {
                let x = v.as_f64().unwrap_or(0.0);
                return Ok(Some(Value::F(if mode_given {
                    crate::extras::round_to(x, 0, second == 1)
                } else {
                    crate::extras::round_to(x, second, false)
                })));
            }
            let (places, mode) = if mode_given { (0, second) } else { (second, 0) };
            if !(0..=28).contains(&places) {
                return Err(ExecutionError::exception(ClrExceptionKind::ArgumentOutOfRange, "Rounding digits must be between 0 and 28, inclusive. (Parameter 'decimals')"));
            }
            let r = decode(&v).round(places as u32, mode);
            ret(i, r)
        });
        interp.register_native(k("Round/3"), |i, a| {
            let v = arg(i, a, 0)?;
            let (places, mode) = (arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
            if !is_decimal(i, &v) {
                return Ok(Some(Value::F(crate::extras::round_to(v.as_f64().unwrap_or(0.0), places, mode == 1))));
            }
            let r = decode(&v).round(places.clamp(0, 28) as u32, mode);
            ret(i, r)
        });
        for (name, mode) in [("Floor/1", 3), ("Ceiling/1", 4), ("Truncate/1", 2)] {
            let f: rustclr_core::NativeFn = match mode {
                3 => |i, a| directed(i, a, 3),
                4 => |i, a| directed(i, a, 4),
                _ => |i, a| directed(i, a, 2),
            };
            interp.register_native(k(name), f);
        }
    }
    interp.register_native("System.Math::Abs/1", |i, a| {
        let v = arg(i, a, 0)?;
        Ok(Some(match &v {
            Value::Struct(_) if is_decimal(i, &v) => encode(i, Dec { neg: false, ..decode(&v) }),
            Value::I32(n) => Value::I32(n.wrapping_abs()),
            Value::I64(n) => Value::I64(n.wrapping_abs()),
            Value::F(f) => Value::F(f.abs()),
            other => other.clone(),
        }))
    });
    interp.register_native("System.Math::Sign/1", |i, a| {
        let v = arg(i, a, 0)?;
        Ok(Some(Value::I32(match &v {
            Value::Struct(_) if is_decimal(i, &v) => {
                let d = decode(&v);
                if d.is_zero() { 0 } else if d.neg { -1 } else { 1 }
            }
            Value::F(f) => if *f > 0.0 { 1 } else if *f < 0.0 { -1 } else { 0 },
            other => other.as_i64().unwrap_or(0).signum() as i32,
        })))
    });
    for (name, max) in [("System.Math::Max/2", true), ("System.Math::Min/2", false)] {
        let f: rustclr_core::NativeFn = if max { |i, a| extreme(i, a, true) } else { |i, a| extreme(i, a, false) };
        interp.register_native(name, f);
    }

    // Text.
    interp.register_native(key("ToString()"), |i, a| {
        let x = dec_arg(i, a, 0)?;
        Ok(Some(string_value(i, &x.plain())))
    });
    for name in ["ToString/1", "ToString/2"] {
        interp.register_native(key(name), |i, a| {
            let x = dec_arg(i, a, 0)?;
            let spec = match arg(i, a, 1)? {
                Value::Obj(h) => i.string_value(h).unwrap_or_default(),
                _ => String::new(),
            };
            Ok(Some(string_value(i, &x.format(&spec))))
        });
    }
    for name in ["Parse/1", "Parse/2", "Parse/3"] {
        interp.register_native(key(name), |i, a| {
            let s = arg_string(i, a, 0)?;
            match Dec::parse(&s) {
                Some(d) => ret(i, d),
                None => Err(ExecutionError::exception(ClrExceptionKind::Format, &format!("The input string '{s}' was not in a correct format."))),
            }
        });
    }
    for name in ["TryParse/2", "TryParse/3", "TryParse/4"] {
        interp.register_native(key(name), |i, a| {
            let s = arg_string_or_empty(i, a, 0)?;
            let parsed = Dec::parse(&s);
            let value = encode(i, parsed.unwrap_or(Dec::ZERO));
            if let Some(Value::Ref(r)) = a.last() {
                i.store_indirect_public(r.clone(), value)?;
            }
            Ok(Some(Value::I32(parsed.is_some() as i32)))
        });
    }

    // Convert.ToDecimal, and Convert's numeric conversions given a decimal.
    for name in ["System.Convert::ToDecimal/1", "System.Convert::ToDecimal/2"] {
        interp.register_native(name, |i, a| {
            let v = arg(i, a, 0)?;
            let v = crate::extras::unbox_value(i, &v);
            if let Value::Obj(h) = &v {
                if let Some(s) = i.string_value(*h) {
                    return match Dec::parse(&s) {
                        Some(d) => ret(i, d),
                        None => Err(ExecutionError::exception(ClrExceptionKind::Format, &format!("The input string '{s}' was not in a correct format."))),
                    };
                }
            }
            let from = param(i, 0);
            let d = convert_in(i, &v, &from)?;
            ret(i, d)
        });
    }
    for (name, target) in [
        ("System.Convert::ToDouble/1", "System.Double"),
        ("System.Convert::ToInt32/1", "System.Int32"),
        ("System.Convert::ToInt64/1", "System.Int64"),
    ] {
        let f: rustclr_core::NativeFn = match target {
            "System.Double" => |i, a| convert_any(i, a, "System.Double"),
            "System.Int32" => |i, a| convert_any(i, a, "System.Int32"),
            _ => |i, a| convert_any(i, a, "System.Int64"),
        };
        // Only the decimal overloads reach an arity key; the others have
        // typed keys of their own. Registered only if nothing else claimed it.
        if !interp.has_native(name) {
            interp.register_native(name, f);
        }
    }

    fill_statics(interp);
}

fn directed(i: &mut Interpreter, a: &[Value], mode: i32) -> ExecResult<Option<Value>> {
    let v = arg(i, a, 0)?;
    if !is_decimal(i, &v) {
        let x = v.as_f64().unwrap_or(0.0);
        return Ok(Some(Value::F(match mode {
            3 => crate::fmath::floor(x),
            4 => crate::fmath::ceil(x),
            _ => crate::fmath::trunc(x),
        })));
    }
    let r = decode(&v).round(0, mode);
    ret(i, r)
}

fn extreme(i: &mut Interpreter, a: &[Value], max: bool) -> ExecResult<Option<Value>> {
    let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
    if is_decimal(i, &x) || is_decimal(i, &y) {
        let (dx, dy) = (to_dec(i, &x)?, to_dec(i, &y)?);
        let pick_x = if max { dx.cmp(&dy).is_ge() } else { dx.cmp(&dy).is_le() };
        return ret(i, if pick_x { dx } else { dy });
    }
    Ok(Some(match (&x, &y) {
        (Value::F(p), Value::F(q)) => Value::F(if max { p.max(*q) } else { p.min(*q) }),
        (Value::I32(p), Value::I32(q)) => Value::I32(if max { *p.max(q) } else { *p.min(q) }),
        _ => {
            let (p, q) = (x.as_i64().unwrap_or(0), y.as_i64().unwrap_or(0));
            Value::I64(if max { p.max(q) } else { p.min(q) })
        }
    }))
}

fn convert_any(i: &mut Interpreter, a: &[Value], target: &str) -> ExecResult<Option<Value>> {
    let v = arg(i, a, 0)?;
    let v = crate::extras::unbox_value(i, &v);
    if is_decimal(i, &v) {
        let d = decode(&v);
        // Convert rounds half to even; a cast truncates.
        let d = if target == "System.Double" { d } else { d.round(0, 0) };
        return convert_out(i, d, target);
    }
    Ok(Some(match target {
        "System.Double" => Value::F(v.as_f64().unwrap_or(0.0)),
        "System.Int32" => Value::I32(v.as_f64().map(|f| crate::extras::round_to(f, 0, false) as i32).unwrap_or(0)),
        _ => Value::I64(v.as_f64().map(|f| crate::extras::round_to(f, 0, false) as i64).unwrap_or(0)),
    }))
}

/// `decimal.Zero`, `One`, `MinusOne`, `MaxValue` and `MinValue`.
fn fill_statics(interp: &mut Interpreter) {
    let Some(ty) = decimal_type(interp) else { return };
    let max = Dec { neg: false, scale: 0, mant: MAX_MANT };
    for (name, d) in [
        ("Zero", Dec::ZERO),
        ("One", Dec::from_i64(1)),
        ("MinusOne", Dec::from_i64(-1)),
        ("MaxValue", max),
        ("MinValue", max.neg()),
    ] {
        let field = interp.loader.registry.ty(ty).static_fields.iter().copied().find(|f| interp.loader.registry.field(*f).name == name);
        if let Some(field) = field {
            let v = encode(interp, d);
            interp.loader.set_static(field, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Dec {
        Dec::parse(s).unwrap()
    }

    #[test]
    fn exact_arithmetic_keeps_scale() {
        assert_eq!(d("0.1").add(&d("0.2")).unwrap().plain(), "0.3");
        assert_eq!(d("1.50").add(&d("2.5")).unwrap().plain(), "4.00");
        assert_eq!(d("1.10").mul(&d("3")).unwrap().plain(), "3.30");
        assert_eq!(d("10").sub(&d("0.01")).unwrap().plain(), "9.99");
        assert_eq!(d("-2.5").mul(&d("4")).unwrap().plain(), "-10.0");
        assert_eq!(d("7").rem(&d("2.5")).unwrap().plain(), "2.0");
    }

    #[test]
    fn division_matches_dotnet() {
        assert_eq!(d("1").div(&d("3")).unwrap().plain(), "0.3333333333333333333333333333");
        assert_eq!(d("2").div(&d("3")).unwrap().plain(), "0.6666666666666666666666666667");
        assert_eq!(d("10").div(&d("4")).unwrap().plain(), "2.5");
        assert_eq!(d("100").div(&d("4")).unwrap().plain(), "25");
        assert_eq!(d("1.00").div(&d("1")).unwrap().plain(), "1.00");
        assert_eq!(d("250000").div(&d("3")).unwrap().plain(), "83333.33333333333333333333333");
        assert_eq!(d("1").div(&d("0")), Err(DecError::DivideByZero));
    }

    #[test]
    fn rounding_modes() {
        assert_eq!(d("2.345").round(2, 0).plain(), "2.34");
        assert_eq!(d("2.355").round(2, 0).plain(), "2.36");
        assert_eq!(d("2.345").round(2, 1).plain(), "2.35");
        assert_eq!(d("-2.5").round(0, 0).plain(), "-2");
        assert_eq!(d("-2.5").round(0, 1).plain(), "-3");
        assert_eq!(d("-2.1").round(0, 3).plain(), "-3");
        assert_eq!(d("2.1").round(0, 4).plain(), "3");
        assert_eq!(d("-2.9").round(0, 2).plain(), "-2");
    }

    #[test]
    fn conversions_and_format() {
        assert_eq!(Dec::from_f64(0.1, 15).unwrap().plain(), "0.1");
        assert_eq!(Dec::from_f64(2.5, 15).unwrap().plain(), "2.5");
        assert_eq!(Dec::from_f64(1.0 / 3.0, 15).unwrap().plain(), "0.333333333333333");
        assert_eq!(d("123.456").to_f64(), 123.456);
        assert_eq!(d("-7.9").to_i128(), -7);
        assert_eq!(d("1234567.891").format("N2"), "1,234,567.89");
        assert_eq!(d("0.125").format("P1"), "12.5 %");
        assert_eq!(d("5").format("F2"), "5.00");
        assert_eq!(d("2.5").format(""), "2.5");
        assert_eq!(d("1,234.50").plain(), "1234.50");
        assert_eq!(d("1.5e2").plain(), "150");
        assert!(Dec::parse("abc").is_none());
    }

    #[test]
    fn comparison_ignores_trailing_zeros() {
        assert!(d("1.0").cmp(&d("1.00")).is_eq());
        assert!(d("-0").cmp(&d("0")).is_eq());
        assert!(d("-1").cmp(&d("0.5")).is_lt());
        assert!(d("2.01").cmp(&d("2.1")).is_lt());
        assert_eq!(d("1.0").hash(), d("1.000").hash());
    }

    #[test]
    fn overflow_and_precision_limits() {
        let max = Dec { neg: false, scale: 0, mant: MAX_MANT };
        assert_eq!(max.add(&d("1")), Err(DecError::Overflow));
        assert_eq!(max.plain(), "79228162514264337593543950335");
        // 28 places at most; a product needing more is rounded.
        assert_eq!(d("0.0000000000000001").mul(&d("0.0000000000000001")).unwrap().plain(), "0.0000000000000000000000000000");
        assert_eq!(d("1.23456789012345678901234567891").plain(), "1.2345678901234567890123456789");
    }
}
