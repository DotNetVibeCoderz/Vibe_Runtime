//! `System.Math`, `System.Convert` and the primitive value types.

use crate::support::*;
use rustclr_core::{ByRef, Interpreter, Value};

#[allow(unused_imports)]
use crate::prelude::*;

pub fn register(interp: &mut Interpreter) {
    register_math(interp);
    register_convert(interp);
    register_primitives(interp);
}

fn register_math(interp: &mut Interpreter) {
    macro_rules! unary_f64 {
        ($key:literal, $f:expr) => {
            interp.register_native($key, |i, a| {
                let x = arg_f64(i, a, 0)?;
                let f: fn(f64) -> f64 = $f;
                Ok(Some(Value::F(f(x))))
            });
        };
    }

    unary_f64!("System.Math::Sqrt(double)", crate::fmath::sqrt);
    unary_f64!("System.Math::Sin(double)", crate::fmath::sin);
    unary_f64!("System.Math::Cos(double)", crate::fmath::cos);
    unary_f64!("System.Math::Tan(double)", crate::fmath::tan);
    unary_f64!("System.Math::Asin(double)", crate::fmath::asin);
    unary_f64!("System.Math::Acos(double)", crate::fmath::acos);
    unary_f64!("System.Math::Atan(double)", crate::fmath::atan);
    unary_f64!("System.Math::Exp(double)", crate::fmath::exp);
    unary_f64!("System.Math::Log(double)", crate::fmath::ln);
    unary_f64!("System.Math::Log10(double)", crate::fmath::log10);
    unary_f64!("System.Math::Log2(double)", crate::fmath::log2);
    unary_f64!("System.Math::Floor(double)", crate::fmath::floor);
    unary_f64!("System.Math::Ceiling(double)", crate::fmath::ceil);
    unary_f64!("System.Math::Truncate(double)", crate::fmath::trunc);
    unary_f64!("System.Math::Cbrt(double)", crate::fmath::cbrt);

    interp.register_native("System.Math::Abs(int)", |i, a| {
        let x = arg_i32(i, a, 0)?;
        // `Math.Abs(int.MinValue)` throws rather than wrapping.
        match x.checked_abs() {
            Some(v) => Ok(Some(Value::I32(v))),
            None => Err(rustclr_core::ExecutionError::overflow()),
        }
    });
    interp.register_native("System.Math::Abs(long)", |i, a| {
        let x = arg_i64(i, a, 0)?;
        match x.checked_abs() {
            Some(v) => Ok(Some(Value::I64(v))),
            None => Err(rustclr_core::ExecutionError::overflow()),
        }
    });
    interp.register_native("System.Math::Abs(double)", |i, a| {
        Ok(Some(Value::F(crate::fmath::abs(arg_f64(i, a, 0)?))))
    });

    interp.register_native("System.Math::Max(int,int)", |i, a| {
        Ok(Some(Value::I32(arg_i32(i, a, 0)?.max(arg_i32(i, a, 1)?))))
    });
    interp.register_native("System.Math::Min(int,int)", |i, a| {
        Ok(Some(Value::I32(arg_i32(i, a, 0)?.min(arg_i32(i, a, 1)?))))
    });
    interp.register_native("System.Math::Max(long,long)", |i, a| {
        Ok(Some(Value::I64(arg_i64(i, a, 0)?.max(arg_i64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Min(long,long)", |i, a| {
        Ok(Some(Value::I64(arg_i64(i, a, 0)?.min(arg_i64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Max(double,double)", |i, a| {
        Ok(Some(Value::F(arg_f64(i, a, 0)?.max(arg_f64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Min(double,double)", |i, a| {
        Ok(Some(Value::F(arg_f64(i, a, 0)?.min(arg_f64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Pow(double,double)", |i, a| {
        Ok(Some(Value::F(crate::fmath::powf(arg_f64(i, a, 0)?, arg_f64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Atan2(double,double)", |i, a| {
        Ok(Some(Value::F(crate::fmath::atan2(arg_f64(i, a, 0)?, arg_f64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Sign(int)", |i, a| {
        Ok(Some(Value::I32(arg_i32(i, a, 0)?.signum())))
    });
    interp.register_native("System.Math::Sign(double)", |i, a| {
        let x = arg_f64(i, a, 0)?;
        Ok(Some(Value::I32(if x > 0.0 {
            1
        } else if x < 0.0 {
            -1
        } else {
            0
        })))
    });
    interp.register_native("System.Math::Round(double)", |i, a| {
        // .NET rounds half to even by default; Rust's `round` rounds half away
        // from zero, so this is spelled out rather than delegated.
        Ok(Some(Value::F(round_half_even(arg_f64(i, a, 0)?, 0))))
    });
    interp.register_native("System.Math::Round(double,int)", |i, a| {
        let x = arg_f64(i, a, 0)?;
        let digits = arg_i32(i, a, 1)?.clamp(0, 15) as u32;
        Ok(Some(Value::F(round_half_even(x, digits))))
    });
}

fn register_convert(interp: &mut Interpreter) {
    interp.register_native("System.Convert::ToInt32(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        parse_i32(&s).map(|v| Some(Value::I32(v)))
    });
    interp.register_native("System.Convert::ToInt32(double)", |i, a| {
        Ok(Some(Value::I32(round_half_even(arg_f64(i, a, 0)?, 0) as i32)))
    });
    // A boxed number (an enum among them) converts by value; only text is
    // parsed. Formatting first turned a boxed enum into its *name*.
    interp.register_native("System.Convert::ToInt32(object)", |i, a| {
        let v = arg(i, a, 0)?;
        let v = crate::extras::unbox_value(i, &v);
        match v {
            Value::Null => Ok(Some(Value::I32(0))),
            Value::I32(n) => Ok(Some(Value::I32(n))),
            Value::I64(n) => Ok(Some(Value::I32(n as i32))),
            Value::F(x) => Ok(Some(Value::I32(round_half_even(x, 0) as i32))),
            other => {
                let text = display(i, &other);
                parse_i32(&text).map(|v| Some(Value::I32(v)))
            }
        }
    });
    interp.register_native("System.Convert::ToInt64(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        s.trim().parse::<i64>().map(|v| Some(Value::I64(v))).map_err(|_| bad_format(&s))
    });
    interp.register_native("System.Convert::ToDouble(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        s.trim().parse::<f64>().map(|v| Some(Value::F(v))).map_err(|_| bad_format(&s))
    });
    interp.register_native("System.Convert::ToBoolean(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(Value::I32(s.trim().eq_ignore_ascii_case("true") as i32)))
    });
    interp.register_native("System.Convert::ToString(int)", |i, a| {
        let v = arg_i32(i, a, 0)?;
        Ok(Some(string_value(i, &v.to_string())))
    });
    interp.register_native("System.Convert::ToString(double)", |i, a| {
        let v = arg_f64(i, a, 0)?;
        Ok(Some(string_value(i, &format_double(v))))
    });
    interp.register_native("System.Convert::ToString(object)", |i, a| {
        let v = arg(i, a, 0)?;
        let s = display(i, &v);
        Ok(Some(string_value(i, &s)))
    });
}

fn register_primitives(interp: &mut Interpreter) {
    // -- ToString ---------------------------------------------------------
    for ty in ["Int32", "Int16", "SByte", "Byte", "UInt16"] {
        interp.register_native(
            Box::leak(format!("System.{ty}::ToString()").into_boxed_str()) as &str,
            |i, a| {
                let v = arg_i32(i, a, 0)?;
                Ok(Some(string_value(i, &v.to_string())))
            },
        );
    }
    interp.register_native("System.UInt32::ToString()", |i, a| {
        let v = arg_i32(i, a, 0)? as u32;
        Ok(Some(string_value(i, &v.to_string())))
    });
    interp.register_native("System.Int64::ToString()", |i, a| {
        let v = arg_i64(i, a, 0)?;
        Ok(Some(string_value(i, &v.to_string())))
    });
    interp.register_native("System.UInt64::ToString()", |i, a| {
        let v = arg_i64(i, a, 0)? as u64;
        Ok(Some(string_value(i, &v.to_string())))
    });
    interp.register_native("System.Double::ToString()", |i, a| {
        let v = arg_f64(i, a, 0)?;
        Ok(Some(string_value(i, &format_double(v))))
    });
    interp.register_native("System.Single::ToString()", |i, a| {
        let v = arg_f64(i, a, 0)? as f32;
        Ok(Some(string_value(i, &format_single(v))))
    });
    interp.register_native("System.Boolean::ToString()", |i, a| {
        let v = arg_bool(i, a, 0)?;
        Ok(Some(string_value(i, if v { "True" } else { "False" })))
    });

    // -- Parse / TryParse -------------------------------------------------
    interp.register_native("System.Int32::Parse(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        parse_i32(&s).map(|v| Some(Value::I32(v)))
    });
    interp.register_native("System.Int64::Parse(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        s.trim().parse::<i64>().map(|v| Some(Value::I64(v))).map_err(|_| bad_format(&s))
    });
    interp.register_native("System.Double::Parse(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        s.trim().parse::<f64>().map(|v| Some(Value::F(v))).map_err(|_| bad_format(&s))
    });

    // `TryParse` writes through an `out` parameter, which arrives as a `&`.
    interp.register_native("System.Int32::TryParse(string,int&)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let parsed = s.trim().parse::<i32>().ok();
        write_out_param(i, a, 1, Value::I32(parsed.unwrap_or(0)))?;
        Ok(Some(Value::I32(parsed.is_some() as i32)))
    });
    interp.register_native("System.Int64::TryParse(string,long&)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let parsed = s.trim().parse::<i64>().ok();
        write_out_param(i, a, 1, Value::I64(parsed.unwrap_or(0)))?;
        Ok(Some(Value::I32(parsed.is_some() as i32)))
    });
    interp.register_native("System.Double::TryParse(string,double&)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let parsed = s.trim().parse::<f64>().ok();
        write_out_param(i, a, 1, Value::F(parsed.unwrap_or(0.0)))?;
        Ok(Some(Value::I32(parsed.is_some() as i32)))
    });

    // -- Equals / GetHashCode / CompareTo ---------------------------------
    for ty in ["Int32", "Int64", "Double", "Boolean", "Char"] {
        let equals = Box::leak(format!("System.{ty}::Equals(object)").into_boxed_str()) as &str;
        interp.register_native(equals, |i, a| {
            let x = arg(i, a, 0)?;
            let y = arg(i, a, 1)?;
            Ok(Some(Value::I32(numeric_equal(&x, &y) as i32)))
        });
        let hash = Box::leak(format!("System.{ty}::GetHashCode()").into_boxed_str()) as &str;
        interp.register_native(hash, |i, a| {
            let x = arg(i, a, 0)?;
            Ok(Some(Value::I32(x.as_i64().unwrap_or(0) as i32)))
        });
    }
    interp.register_native("System.Int32::CompareTo(int)", |i, a| {
        let x = arg_i32(i, a, 0)?;
        let y = arg_i32(i, a, 1)?;
        Ok(Some(Value::I32(match x.cmp(&y) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        })))
    });
}

/// Writes to an `out`/`ref` parameter.
fn write_out_param(
    interp: &mut Interpreter,
    args: &[Value],
    index: usize,
    value: Value,
) -> rustclr_core::ExecResult<()> {
    if let Some(Value::Ref(r)) = args.get(index) {
        let target: ByRef = r.clone();
        interp.store_indirect_public(target, value)?;
    }
    Ok(())
}

fn numeric_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::F(x), Value::F(y)) => x == y,
        _ => a.as_i64() == b.as_i64(),
    }
}

/// `NumberStyles.AllowHexSpecifier` and `NumberStyles.AllowThousands`.
const HEX: i32 = 0x200;
const THOUSANDS: i32 = 0x40;
/// `NumberStyles.Integer`, what the overloads without styles use.
const INTEGER: i32 = 7;

/// A string argument, or a span of characters standing for one.
fn text_arg(i: &mut Interpreter, a: &[Value], n: usize) -> rustclr_core::ExecResult<String> {
    let v = arg(i, a, n)?;
    if let Some(h) = v.as_handle().filter(|h| !h.is_null()) {
        if let Some(s) = i.heap.with::<rustclr_core::ClrString, _>(h, |s| s.to_rust_string()) {
            return Ok(s);
        }
    }
    if let Some(chars) = crate::spans::values(i, &v) {
        let units: Vec<u16> = chars.iter().map(|c| c.as_i32().unwrap_or(0) as u16).collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    Ok(String::new())
}

/// The styles argument when argument `n` is one (an enum arrives as an int), else `Integer`.
fn styles_arg(i: &mut Interpreter, a: &[Value], n: usize) -> i32 {
    match a.get(n).map(|_| arg(i, a, n)) {
        Some(Ok(Value::I32(s))) => s,
        _ => INTEGER,
    }
}

/// Parses with the subset of `NumberStyles` an app meets: hex digits (which
/// wrap like .NET's, so `FFFFFFFF` is -1), thousands separators, white space and a sign.
fn parse_styled(text: &str, styles: i32, wide: bool, lenient: bool) -> Option<i64> {
    let t = text.trim();
    if styles & HEX != 0 {
        if t.is_empty() || t.len() > if wide { 16 } else { 8 } {
            return None;
        }
        let v = u64::from_str_radix(t, 16).ok()?;
        return Some(if wide { v as i64 } else { v as u32 as i32 as i64 });
    }
    let plain = if lenient || styles & THOUSANDS != 0 { t.replace(',', "") } else { String::from(t) };
    if wide { plain.parse::<i64>().ok() } else { plain.parse::<i32>().ok().map(i64::from) }
}

fn number(v: i64, wide: bool) -> Value {
    if wide { Value::I64(v) } else { Value::I32(v as i32) }
}

/// `Parse` in every shape; it has always let thousands separators through.
pub(crate) fn parse_styled_native(i: &mut Interpreter, a: &[Value], wide: bool) -> rustclr_core::ExecResult<Option<Value>> {
    let text = text_arg(i, a, 0)?;
    let styles = styles_arg(i, a, 1);
    parse_styled(&text, styles, wide, true).map(|v| Some(number(v, wide))).ok_or_else(|| bad_format(&text))
}

/// `TryParse` in every shape; the out parameter is always the last argument.
pub(crate) fn try_parse_styled_native(i: &mut Interpreter, a: &[Value], wide: bool) -> rustclr_core::ExecResult<Option<Value>> {
    let text = text_arg(i, a, 0)?;
    let out = a.len() - 1;
    let styles = if a.len() == 4 { styles_arg(i, a, 1) } else { INTEGER };
    let parsed = parse_styled(&text, styles, wide, false);
    write_out_param(i, a, out, number(parsed.unwrap_or(0), wide))?;
    Ok(Some(Value::I32(parsed.is_some() as i32)))
}

fn parse_i32(s: &str) -> rustclr_core::ExecResult<i32> {
    s.trim().parse::<i32>().map_err(|_| bad_format(s))
}

/// Banker's rounding, which is what `Math.Round` uses by default.
fn round_half_even(x: f64, digits: u32) -> f64 {
    let factor = crate::fmath::powi(10f64, digits as i32);
    let scaled = x * factor;
    let floor = crate::fmath::floor(scaled);
    let diff = scaled - floor;

    let rounded = if crate::fmath::abs(diff - 0.5) < f64::EPSILON {
        // Exactly halfway: pick the even neighbour.
        if (floor as i64) % 2 == 0 { floor } else { floor + 1.0 }
    } else {
        crate::fmath::round(scaled)
    };
    rounded / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_uses_bankers_rounding_like_dotnet() {
        assert_eq!(round_half_even(2.5, 0), 2.0);
        assert_eq!(round_half_even(3.5, 0), 4.0);
        assert_eq!(round_half_even(-2.5, 0), -2.0);
        assert_eq!(round_half_even(2.4, 0), 2.0);
        assert_eq!(round_half_even(2.6, 0), 3.0);
    }

    #[test]
    fn round_honours_a_digit_count() {
        assert!(crate::fmath::abs(round_half_even(3.14159, 2) - 3.14) < 1e-12);
    }

    #[test]
    fn doubles_print_without_a_redundant_fraction() {
        assert_eq!(format_double(42.0), "42");
        assert_eq!(format_double(0.5), "0.5");
        assert_eq!(format_double(f64::NAN), "NaN");
    }
}
