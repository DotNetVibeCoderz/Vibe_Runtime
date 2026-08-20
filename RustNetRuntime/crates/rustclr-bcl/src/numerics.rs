//! `System.Math`, `System.Convert` and the primitive value types.

use crate::support::*;
use rustclr_core::{ByRef, Interpreter, Value};

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

    unary_f64!("System.Math::Sqrt(double)", f64::sqrt);
    unary_f64!("System.Math::Sin(double)", f64::sin);
    unary_f64!("System.Math::Cos(double)", f64::cos);
    unary_f64!("System.Math::Tan(double)", f64::tan);
    unary_f64!("System.Math::Asin(double)", f64::asin);
    unary_f64!("System.Math::Acos(double)", f64::acos);
    unary_f64!("System.Math::Atan(double)", f64::atan);
    unary_f64!("System.Math::Exp(double)", f64::exp);
    unary_f64!("System.Math::Log(double)", f64::ln);
    unary_f64!("System.Math::Log10(double)", f64::log10);
    unary_f64!("System.Math::Log2(double)", f64::log2);
    unary_f64!("System.Math::Floor(double)", f64::floor);
    unary_f64!("System.Math::Ceiling(double)", f64::ceil);
    unary_f64!("System.Math::Truncate(double)", f64::trunc);
    unary_f64!("System.Math::Cbrt(double)", f64::cbrt);

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
        Ok(Some(Value::F(arg_f64(i, a, 0)?.abs())))
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
        Ok(Some(Value::F(arg_f64(i, a, 0)?.powf(arg_f64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Atan2(double,double)", |i, a| {
        Ok(Some(Value::F(arg_f64(i, a, 0)?.atan2(arg_f64(i, a, 1)?))))
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
    interp.register_native("System.Convert::ToInt32(object)", |i, a| {
        let v = arg(i, a, 0)?;
        let text = display(i, &v);
        parse_i32(&text).map(|v| Some(Value::I32(v)))
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
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
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

fn parse_i32(s: &str) -> rustclr_core::ExecResult<i32> {
    s.trim().parse::<i32>().map_err(|_| bad_format(s))
}

/// Banker's rounding, which is what `Math.Round` uses by default.
fn round_half_even(x: f64, digits: u32) -> f64 {
    let factor = 10f64.powi(digits as i32);
    let scaled = x * factor;
    let floor = scaled.floor();
    let diff = scaled - floor;

    let rounded = if (diff - 0.5).abs() < f64::EPSILON {
        // Exactly halfway: pick the even neighbour.
        if (floor as i64) % 2 == 0 { floor } else { floor + 1.0 }
    } else {
        scaled.round()
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
        assert!((round_half_even(3.14159, 2) - 3.14).abs() < 1e-12);
    }

    #[test]
    fn doubles_print_without_a_redundant_fraction() {
        assert_eq!(format_double(42.0), "42");
        assert_eq!(format_double(0.5), "0.5");
        assert_eq!(format_double(f64::NAN), "NaN");
    }
}
