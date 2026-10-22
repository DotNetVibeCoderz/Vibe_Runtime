//! The base class library a business application expects.
//!
//! Found by running a probe of what dashboards, CRMs and point-of-sale apps
//! call (Rustzor's `tests/BclProbe`) on .NET and on RustCLR and diffing the
//! two. Everything here closed a line of that diff:
//!
//! * **Formatting and parsing**: `ToString("N2")`, `TryParse`, culture
//!   overloads (always invariant; see [`crate::format`]).
//! * **`DateTime` and `TimeSpan`** as .NET ticks, with calendar arithmetic and
//!   format strings. `DateTime` used to be Unix milliseconds, which cannot
//!   represent a date built with `new DateTime(2026, 3, 9)`.
//! * **String and `StringBuilder` overloads**: comparisons, `char` and
//!   `startIndex` variants, padding, trimming, splitting with options.
//! * **Enums by name**: `ToString`, `Parse`, `TryParse`, `GetValues`.
//! * **Collections and LINQ**: `Find`/`RemoveAll`/`GetRange`, `Array.Sort`,
//!   `MaxBy`, `Chunk`, `DistinctBy`, `OfType`, `Join`, `ToLookup`.
//! * **Events**: `Delegate.Combine`/`Remove`, `EventArgs`, `EventHandler<T>`.
//! * Small types: `Lazy<T>`, `Guid.NewGuid`, `HashCode`, `Uri` escaping,
//!   `CultureInfo`, `StringComparer`.
//!
//! Registered after the rest of RustBCL, so an entry here replaces an older
//! binding with the same key where the older one was wrong.

use crate::collections::{
    elements, field, field_handle, list_items, new_list, receiver, sequence_values, set_field, values_equal,
    value_hash, with_values, HASHSET_BUCKETS, HASHSET_ITEMS, HASHSET_NEXT, LIST_ITEMS,
};
use crate::format::{self, TICKS_PER_DAY, TICKS_PER_HOUR, TICKS_PER_MINUTE, TICKS_PER_MS, TICKS_PER_SECOND};
use crate::support::*;
use rustclr_core::{
    ByRef, ClrArray, ClrBox, ClrDelegate, ClrException, ClrExceptionKind, ClrObject, ClrString, ExecResult,
    ExecutionError, Interpreter, MethodId, TypeId, TypeKind, Value,
};
use rustclr_core::Primitive;
use rustclr_gc::Handle;

#[allow(unused_imports)]
use crate::prelude::*;

pub fn register(interp: &mut Interpreter) {
    register_collection_interfaces(interp);
    // The types these bindings belong to; registered here, not in the
    // loader, so the minimal tier a small board runs does not pay for them.
    interp.loader.register_business_types();
    register_numbers(interp);
    register_datetime(interp);
    register_timespan(interp);
    register_datetimeoffset(interp);
    fill_value_statics(interp);
    register_strings(interp);
    register_string_builder(interp);
    register_framework_gaps(interp);
    register_chars(interp);
    register_enums(interp);
    register_lists(interp);
    register_arrays(interp);
    register_sets_and_dictionaries(interp);
    register_linq(interp);
    register_delegates(interp);
    register_misc(interp);
}

/// Leaks a stable key; the native table holds it for the life of the process.
fn k(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn bad_format(what: &str) -> ExecutionError {
    ExecutionError::exception(ClrExceptionKind::Format, &format!("The input string '{what}' was not in a correct format."))
}

fn argument(message: &str) -> ExecutionError {
    ExecutionError::exception(ClrExceptionKind::Argument, message)
}

fn out_range(message: &str) -> ExecutionError {
    ExecutionError::exception(ClrExceptionKind::ArgumentOutOfRange, message)
}

/// Writes through an `out` parameter.
fn store_out(interp: &mut Interpreter, args: &[Value], index: usize, value: Value) -> ExecResult<()> {
    match args.get(index) {
        Some(Value::Ref(r)) => {
            let r: ByRef = r.clone();
            interp.store_indirect_public(r, value)
        }
        _ => Ok(()),
    }
}

/// The type argument of the generic method being run natively, or of the
/// generic type it was called on.
fn type_arg(interp: &Interpreter, index: usize) -> Option<TypeId> {
    if let Some(m) = interp.current_native_method() {
        if let Some(t) = interp.loader.registry.method(m).generic_args.get(index) {
            return Some(*t);
        }
    }
    interp.current_type_arguments().get(index).copied()
}

/// The full name of parameter `index` of the method being run natively, so a
/// binding can tell `op_Subtraction(DateTime, DateTime)` from
/// `op_Subtraction(DateTime, TimeSpan)`: both are two 64-bit slots.
pub(crate) fn param_type_name(interp: &Interpreter, index: usize) -> Option<String> {
    let method = interp.current_native_method()?;
    let info = interp.loader.registry.method(method);
    let sig = info.signature.params.get(index)?.unwrap_modifiers().clone();
    let assembly = interp.current_assembly()?;
    let ty = interp.loader.resolve_type_sig(interp.loader.assembly(assembly), &sig)?;
    Some(interp.loader.registry.ty(ty).full_name())
}

pub(crate) fn return_type_name(interp: &Interpreter) -> Option<String> {
    let method = interp.current_native_method()?;
    let info = interp.loader.registry.method(method);
    let sig = info.signature.return_type.unwrap_modifiers().clone();
    let assembly = interp.current_assembly()?;
    let ty = interp.loader.resolve_type_sig(interp.loader.assembly(assembly), &sig)?;
    Some(interp.loader.registry.ty(ty).full_name())
}

fn units(interp: &mut Interpreter, args: &[Value], index: usize) -> ExecResult<Vec<u16>> {
    let v = arg(interp, args, index)?;
    match v {
        Value::Obj(h) if !h.is_null() => interp
            .heap
            .with::<ClrString, _>(h, |s| s.units.clone())
            .ok_or_else(ExecutionError::null_reference),
        _ => Err(ExecutionError::null_reference()),
    }
}

fn string_from_units(interp: &mut Interpreter, units: Vec<u16>) -> Value {
    Value::Obj(interp.alloc_clr_string(ClrString { units }))
}

fn fold(unit: u16) -> u16 {
    if unit < 128 {
        return (unit as u8).to_ascii_lowercase() as u16;
    }
    match char::from_u32(unit as u32) {
        Some(c) => {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) if (l as u32) < 0x10000 => l as u16,
                _ => unit,
            }
        }
        None => unit,
    }
}

fn find_units(haystack: &[u16], needle: &[u16], from: usize, ignore_case: bool) -> i32 {
    if needle.is_empty() {
        return from.min(haystack.len()) as i32;
    }
    if needle.len() > haystack.len() {
        return -1;
    }
    for start in from..=haystack.len() - needle.len() {
        let matched = if ignore_case {
            haystack[start..start + needle.len()].iter().zip(needle).all(|(a, b)| fold(*a) == fold(*b))
        } else {
            &haystack[start..start + needle.len()] == needle
        };
        if matched {
            return start as i32;
        }
    }
    -1
}

/// `StringComparison`: 1, 3 and 5 are the case-insensitive members.
fn ignores_case(comparison: i32) -> bool {
    comparison % 2 == 1
}

fn compare_units(a: &[u16], b: &[u16], ignore_case: bool) -> i32 {
    let ordering = if ignore_case {
        a.iter().map(|u| fold(*u)).cmp(b.iter().map(|u| fold(*u)))
    } else {
        a.cmp(b)
    };
    match ordering {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

// ── numbers ──────────────────────────────────────────────────────────────────

/// Renders any value with a format string. Used by `ToString(format)`,
/// interpolation holes and `string.Format`.
pub fn format_value(interp: &mut Interpreter, value: &Value, type_hint: Option<TypeId>, spec: &str) -> String {
    let hint = type_hint.map(|t| interp.loader.registry.ty(t).full_name()).unwrap_or_default();
    match hint.as_str() {
        "System.DateTime" => return format::format_date(value.as_i64().unwrap_or(0), spec),
        "System.TimeSpan" => return format::format_timespan(value.as_i64().unwrap_or(0), spec),
        "System.DateTimeOffset" => return format_datetimeoffset(interp, value.as_i64().unwrap_or(0), spec),
        "System.Decimal" => return crate::decimal::decode(value).format(spec),
        "System.Guid" => {
            let text = match value {
                Value::Obj(h) if !h.is_null() => interp.string_value(*h).unwrap_or_default(),
                _ => "00000000-0000-0000-0000-000000000000".into(),
            };
            return guid_format(&text, spec).unwrap_or(text);
        }
        "System.Boolean" => return if value.is_truthy() { "True".into() } else { "False".into() },
        "System.Char" => {
            return char::from_u32(value.as_i32().unwrap_or(0) as u32).map(String::from).unwrap_or_default()
        }
        _ => {}
    }
    if let Some(t) = type_hint {
        if interp.loader.registry.ty(t).kind == TypeKind::Enum {
            return enum_name(interp, t, value.as_i64().unwrap_or(0));
        }
    }
    match value {
        Value::Obj(h) if !h.is_null() => {
            let boxed = interp.heap.with::<ClrBox, _>(*h, |b| (b.value.clone(), b.type_id));
            if let Some((inner, ty)) = boxed {
                return format_value(interp, &inner, Some(ty), spec);
            }
            display(interp, value)
        }
        Value::F(f) => {
            let single = hint == "System.Single";
            let shown = if single && spec.is_empty() { (*f as f32) as f64 } else { *f };
            if single && spec.is_empty() {
                return crate::support::format_single(*f as f32);
            }
            format::format_f64(shown, spec).unwrap_or_else(|| display(interp, value))
        }
        Value::I32(n) => {
            let bits = match hint.as_str() {
                "System.Byte" | "System.SByte" => Some(8),
                "System.Int16" | "System.UInt16" => Some(16),
                _ => Some(32),
            };
            format::format_i64(*n as i64, spec, bits).unwrap_or_else(|| n.to_string())
        }
        Value::I64(n) | Value::NativeInt(n) => format::format_i64(*n, spec, Some(64)).unwrap_or_else(|| n.to_string()),
        _ => display(interp, value),
    }
}

fn register_numbers(interp: &mut Interpreter) {
    for (ty, prim) in [
        ("System.Int32", "int"),
        ("System.Int64", "long"),
        ("System.Int16", "short"),
        ("System.Byte", "byte"),
        ("System.UInt32", "uint"),
        ("System.UInt64", "ulong"),
        ("System.Double", "double"),
        ("System.Single", "float"),
    ] {
        let _ = prim;
        let type_name: &'static str = k(ty.to_string());
        let with_format = |i: &mut Interpreter, a: &[Value], type_name: &str| -> ExecResult<Option<Value>> {
            let v = arg(i, a, 0)?;
            let spec = match a.get(1).map(|_| arg(i, a, 1)).transpose()? {
                Some(Value::Obj(h)) if i.string_value(h).is_some() => i.string_value(h).unwrap_or_default(),
                _ => String::new(),
            };
            let hint = i.loader.registry.find_type_by_name(type_name);
            let text = format_value(i, &v, hint, &spec);
            Ok(Some(string_value(i, &text)))
        };
        // ToString(string), ToString(IFormatProvider), ToString(string, IFormatProvider).
        // A provider arrives as an object that is not a string, which
        // `with_format` treats as "no format": the invariant default.
        match ty {
            "System.Int32" => {
                interp.register_native("System.Int32::ToString(string)", |i, a| fmt_num(i, a, "System.Int32"));
                interp.register_native("System.Int32::ToString/1", |i, a| fmt_num(i, a, "System.Int32"));
                interp.register_native("System.Int32::ToString/2", |i, a| fmt_num(i, a, "System.Int32"));
            }
            "System.Int64" => {
                interp.register_native("System.Int64::ToString(string)", |i, a| fmt_num(i, a, "System.Int64"));
                interp.register_native("System.Int64::ToString/1", |i, a| fmt_num(i, a, "System.Int64"));
                interp.register_native("System.Int64::ToString/2", |i, a| fmt_num(i, a, "System.Int64"));
            }
            "System.Int16" => {
                interp.register_native("System.Int16::ToString(string)", |i, a| fmt_num(i, a, "System.Int16"));
                interp.register_native("System.Int16::ToString()", |i, a| fmt_num(i, a, "System.Int16"));
            }
            "System.Byte" => {
                interp.register_native("System.Byte::ToString(string)", |i, a| fmt_num(i, a, "System.Byte"));
                interp.register_native("System.Byte::ToString()", |i, a| fmt_num(i, a, "System.Byte"));
            }
            "System.UInt32" => {
                interp.register_native("System.UInt32::ToString(string)", |i, a| fmt_num(i, a, "System.UInt32"));
            }
            "System.UInt64" => {
                interp.register_native("System.UInt64::ToString(string)", |i, a| fmt_num(i, a, "System.UInt64"));
            }
            "System.Double" => {
                interp.register_native("System.Double::ToString(string)", |i, a| fmt_num(i, a, "System.Double"));
                interp.register_native("System.Double::ToString/1", |i, a| fmt_num(i, a, "System.Double"));
                interp.register_native("System.Double::ToString/2", |i, a| fmt_num(i, a, "System.Double"));
            }
            _ => {
                interp.register_native("System.Single::ToString(string)", |i, a| fmt_num(i, a, "System.Single"));
                interp.register_native("System.Single::ToString/1", |i, a| fmt_num(i, a, "System.Single"));
                interp.register_native("System.Single::ToString/2", |i, a| fmt_num(i, a, "System.Single"));
            }
        }
        let _ = (type_name, with_format);
    }

    // Parse and TryParse, for every shape: (s), (s, provider), (s, styles,
    // provider), and the out-parameter forms.
    for ty in ["Int32", "Int64", "Int16", "Byte"] {
        let full: &'static str = k(format!("System.{ty}"));
        let wide = ty == "Int64";
        for member in ["Parse/2", "Parse/3"] {
            interp.register_native(k(format!("{full}::{member}")), if wide { |i, a| crate::numerics::parse_styled_native(i, a, true) } else { |i, a| crate::numerics::parse_styled_native(i, a, false) });
        }
        if ty != "Int32" && ty != "Int64" {
            interp.register_native(k(format!("{full}::Parse(string)")), parse_int);
        }
        for member in ["TryParse/2", "TryParse/3", "TryParse/4"] {
            interp.register_native(k(format!("{full}::{member}")), if wide { |i, a| crate::numerics::try_parse_styled_native(i, a, true) } else { |i, a| crate::numerics::try_parse_styled_native(i, a, false) });
        }
    }
    for ty in ["Double", "Single"] {
        let full: &'static str = k(format!("System.{ty}"));
        for member in ["Parse/2", "Parse/3"] {
            interp.register_native(k(format!("{full}::{member}")), parse_double);
        }
        if ty == "Single" {
            interp.register_native("System.Single::Parse(string)", parse_double);
        }
        for member in ["TryParse/2", "TryParse/3", "TryParse/4"] {
            interp.register_native(k(format!("{full}::{member}")), try_parse_double);
        }
        interp.register_native(k(format!("{full}::IsNaN/1")), |i, a| Ok(Some(Value::I32(arg_f64(i, a, 0)?.is_nan() as i32))));
        interp.register_native(k(format!("{full}::IsInfinity/1")), |i, a| {
            Ok(Some(Value::I32(arg_f64(i, a, 0)?.is_infinite() as i32)))
        });
        interp.register_native(k(format!("{full}::IsFinite/1")), |i, a| Ok(Some(Value::I32(arg_f64(i, a, 0)?.is_finite() as i32))));
        interp.register_native(k(format!("{full}::IsPositiveInfinity/1")), |i, a| {
            Ok(Some(Value::I32((arg_f64(i, a, 0)? == f64::INFINITY) as i32)))
        });
        interp.register_native(k(format!("{full}::IsNegativeInfinity/1")), |i, a| {
            Ok(Some(Value::I32((arg_f64(i, a, 0)? == f64::NEG_INFINITY) as i32)))
        });
    }
    interp.register_native("System.Boolean::Parse(string)", |i, a| {
        let s = arg_string(i, a, 0)?;
        match s.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(Some(Value::I32(1))),
            "false" => Ok(Some(Value::I32(0))),
            _ => Err(bad_format(&s)),
        }
    });
    interp.register_native("System.Boolean::TryParse(string,bool&)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let parsed = match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(1),
            "false" => Some(0),
            _ => None,
        };
        store_out(i, a, 1, Value::I32(parsed.unwrap_or(0)))?;
        Ok(Some(Value::I32(parsed.is_some() as i32)))
    });

    // Math: what the older table lacked.
    interp.register_native("System.Math::Clamp(int,int,int)", |i, a| {
        let (v, lo, hi) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        if lo > hi {
            return Err(argument("'min' cannot be greater than max."));
        }
        Ok(Some(Value::I32(v.clamp(lo, hi))))
    });
    interp.register_native("System.Math::Clamp(long,long,long)", |i, a| {
        let (v, lo, hi) = (arg_i64(i, a, 0)?, arg_i64(i, a, 1)?, arg_i64(i, a, 2)?);
        Ok(Some(Value::I64(v.max(lo).min(hi))))
    });
    interp.register_native("System.Math::Clamp(double,double,double)", |i, a| {
        let (v, lo, hi) = (arg_f64(i, a, 0)?, arg_f64(i, a, 1)?, arg_f64(i, a, 2)?);
        Ok(Some(Value::F(v.max(lo).min(hi))))
    });
    interp.register_native("System.Math::Round(double,int)", |i, a| {
        let (v, digits) = (arg_f64(i, a, 0)?, arg_i32(i, a, 1)?);
        Ok(Some(Value::F(round_to(v, digits, false))))
    });
    // Round(double, MidpointRounding): 0 = ToEven, 1 = AwayFromZero.
    interp.register_native("System.Math::Round/2", |i, a| {
        let (v, mode) = (arg_f64(i, a, 0)?, arg_i32(i, a, 1)?);
        Ok(Some(Value::F(round_to(v, 0, mode == 1))))
    });
    interp.register_native("System.Math::Round/3", |i, a| {
        let (v, digits, mode) = (arg_f64(i, a, 0)?, arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        Ok(Some(Value::F(round_to(v, digits, mode == 1))))
    });
    interp.register_native("System.Math::Truncate(double)", |i, a| Ok(Some(Value::F(crate::fmath::trunc(arg_f64(i, a, 0)?)))));
    interp.register_native("System.Math::Log(double,double)", |i, a| {
        Ok(Some(Value::F(crate::fmath::ln(arg_f64(i, a, 0)?) / crate::fmath::ln(arg_f64(i, a, 1)?))))
    });
    interp.register_native("System.Math::Sign(long)", |i, a| Ok(Some(Value::I32(arg_i64(i, a, 0)?.signum() as i32))));
    interp.register_native("System.Math::Max(long,long)", |i, a| Ok(Some(Value::I64(arg_i64(i, a, 0)?.max(arg_i64(i, a, 1)?)))));
    interp.register_native("System.Math::Min(long,long)", |i, a| Ok(Some(Value::I64(arg_i64(i, a, 0)?.min(arg_i64(i, a, 1)?)))));
    interp.register_native("System.Convert::ToInt32(string,int)", |i, a| {
        let s = arg_string(i, a, 0)?;
        let base = arg_i32(i, a, 1)? as u32;
        let digits = s.trim().trim_start_matches("0x").trim_start_matches("0X");
        i64::from_str_radix(digits, base)
            .map(|v| Some(Value::I32(v as i32)))
            .map_err(|_| bad_format(&s))
    });
    interp.register_native("System.Convert::ToString(int,int)", |i, a| {
        let (v, base) = (arg_i32(i, a, 0)?, arg_i32(i, a, 1)?);
        let text = match base {
            2 => format!("{:b}", v),
            8 => format!("{:o}", v),
            16 => format!("{:x}", v),
            _ => v.to_string(),
        };
        Ok(Some(string_value(i, &text)))
    });
    interp.register_native("System.Convert::ToDouble(object)", |i, a| {
        let v = arg(i, a, 0)?;
        let v = unbox_value(i, &v);
        Ok(Some(Value::F(match v {
            Value::F(f) => f,
            Value::Obj(h) => format::parse_number(&i.string_value(h).unwrap_or_default()).unwrap_or(0.0),
            other => other.as_i64().unwrap_or(0) as f64,
        })))
    });
    interp.register_native("System.Convert::ToString(double,#0)", |i, a| {
        let text = crate::support::format_double(arg_f64(i, a, 0)?);
        Ok(Some(string_value(i, &text)))
    });
}

pub(crate) fn unbox_value(interp: &Interpreter, v: &Value) -> Value {
    match v {
        Value::Obj(h) if !h.is_null() => interp.heap.with::<ClrBox, _>(*h, |b| b.value.clone()).unwrap_or_else(|| v.clone()),
        other => other.clone(),
    }
}

fn fmt_num(i: &mut Interpreter, a: &[Value], type_name: &str) -> ExecResult<Option<Value>> {
    let v = arg(i, a, 0)?;
    let spec = match a.get(1) {
        Some(_) => match arg(i, a, 1)? {
            Value::Obj(h) => i.string_value(h).unwrap_or_default(),
            _ => String::new(),
        },
        None => String::new(),
    };
    let hint = i.loader.registry.find_type_by_name(type_name);
    let text = format_value(i, &v, hint, &spec);
    Ok(Some(string_value(i, &text)))
}

pub(crate) fn round_to(v: f64, digits: i32, away: bool) -> f64 {
    let scale = crate::fmath::powi(10.0, digits.clamp(0, 15));
    let x = v * scale;
    let rounded = if away {
        crate::fmath::round(x)
    } else {
        // Banker's rounding, .NET's default.
        let r = crate::fmath::round(x);
        if crate::fmath::abs(x - crate::fmath::trunc(x)) == 0.5 { 2.0 * crate::fmath::round(x / 2.0) } else { r }
    };
    rounded / scale
}

fn parse_int(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let s = arg_string(i, a, 0)?;
    let cleaned: String = s.trim().chars().filter(|c| *c != ',').collect();
    cleaned.parse::<i32>().map(|v| Some(Value::I32(v))).map_err(|_| bad_format(&s))
}

fn parse_double(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let s = arg_string(i, a, 0)?;
    format::parse_number(&s).map(|v| Some(Value::F(v))).ok_or_else(|| bad_format(&s))
}

fn try_parse_double(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let s = arg_string_or_empty(i, a, 0)?;
    let parsed = format::parse_number(&s);
    store_out(i, a, a.len() - 1, Value::F(parsed.unwrap_or(0.0)))?;
    Ok(Some(Value::I32(parsed.is_some() as i32)))
}

// ── DateTime ─────────────────────────────────────────────────────────────────
//
// A `DateTime` is its tick count, as a 64-bit slot: 100 ns units since
// 0001-01-01. `Kind` is not tracked; `Now` and `UtcNow` differ by the host's
// UTC offset.

fn ticks(i: &mut Interpreter, a: &[Value], index: usize) -> ExecResult<i64> {
    Ok(arg(i, a, index)?.as_i64().unwrap_or(0))
}

fn now_ticks(i: &mut Interpreter, local: bool) -> i64 {
    let ms = i.host.wall_clock_millis();
    let offset = if local { i.host.utc_offset_minutes() as i64 * TICKS_PER_MINUTE } else { 0 };
    ms * TICKS_PER_MS + format::UNIX_EPOCH_TICKS + offset
}

/// The ticks for `text` read with argument 1 of `ParseExact`: one format, or
/// the first of several that matches. A time-only format falls on today.
fn parse_exact_any(i: &mut Interpreter, a: &[Value], text: &str) -> ExecResult<Option<i64>> {
    let formats: Vec<String> = match arg(i, a, 1)? {
        Value::Obj(h) => match i.string_value(h) {
            Some(single) => vec![single],
            None => crate::support::array_values(i, h)
                .into_iter()
                .filter_map(|v| match v {
                    Value::Obj(s) => i.string_value(s),
                    _ => None,
                })
                .collect(),
        },
        _ => {
            return Err(ExecutionError::exception(ClrExceptionKind::ArgumentNull, "Value cannot be null. (Parameter 'format')"));
        }
    };
    for format in &formats {
        if let Some(found) = format::parse_exact(text, format) {
            let today = if found.has_date {
                0
            } else {
                let now = now_ticks(i, true);
                now - now.rem_euclid(TICKS_PER_DAY)
            };
            return Ok(Some(found.ticks + today));
        }
    }
    Ok(None)
}

fn date_value(t: i64) -> ExecResult<Option<Value>> {
    if !(0..=3_155_378_975_999_999_999).contains(&t) {
        return Err(out_range("The added or subtracted value results in an un-representable DateTime."));
    }
    Ok(Some(Value::I64(t)))
}

fn register_datetime(interp: &mut Interpreter) {
    const DT: &str = "System.DateTime";
    let key = |m: &str| k(format!("{DT}::{m}"));

    interp.register_native(key(".ctor(long)"), |i, a| construct_with(i, a, |i, a| date_value(arg_i64(i, a, 1)?)));
    interp.register_native(key(".ctor(int,int,int)"), |i, a| construct_with(i, a, |i, a| {
        let (y, m, d) = (arg_i32(i, a, 1)? as i64, arg_i32(i, a, 2)? as i64, arg_i32(i, a, 3)? as i64);
        checked_date(y, m, d, 0, 0, 0, 0)
    }));
    interp.register_native(key(".ctor(int,int,int,int,int,int)"), |i, a| construct_with(i, a, |i, a| {
        let v: Vec<i64> = (1..=6).map(|n| arg_i32(i, a, n).map(|x| x as i64)).collect::<ExecResult<_>>()?;
        checked_date(v[0], v[1], v[2], v[3], v[4], v[5], 0)
    }));
    interp.register_native(key(".ctor(int,int,int,int,int,int,int)"), |i, a| construct_with(i, a, |i, a| {
        let v: Vec<i64> = (1..=7).map(|n| arg_i32(i, a, n).map(|x| x as i64)).collect::<ExecResult<_>>()?;
        checked_date(v[0], v[1], v[2], v[3], v[4], v[5], v[6])
    }));
    // With a DateTimeKind: the kind is not tracked.
    interp.register_native(key(".ctor/7"), |i, a| construct_with(i, a, |i, a| {
        let v: Vec<i64> = (1..=6).map(|n| arg_i32(i, a, n).map(|x| x as i64)).collect::<ExecResult<_>>()?;
        checked_date(v[0], v[1], v[2], v[3], v[4], v[5], 0)
    }));

    interp.register_native(key("get_Now()"), |i, _| Ok(Some(Value::I64(now_ticks(i, true)))));
    interp.register_native(key("get_UtcNow()"), |i, _| Ok(Some(Value::I64(now_ticks(i, false)))));
    interp.register_native(key("get_Today()"), |i, _| {
        let t = now_ticks(i, true);
        Ok(Some(Value::I64(t - t.rem_euclid(TICKS_PER_DAY))))
    });
    interp.register_native(key("get_Ticks()"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)?))));
    interp.register_native(key("get_Year()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).year as i32))));
    interp.register_native(key("get_Month()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).month as i32))));
    interp.register_native(key("get_Day()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).day as i32))));
    interp.register_native(key("get_Hour()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).hour as i32))));
    interp.register_native(key("get_Minute()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).minute as i32))));
    interp.register_native(key("get_Second()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).second as i32))));
    interp.register_native(key("get_Millisecond()"), |i, a| {
        Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).millisecond as i32)))
    });
    interp.register_native(key("get_DayOfWeek()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).day_of_week as i32))));
    interp.register_native(key("get_DayOfYear()"), |i, a| Ok(Some(Value::I32(format::parts(ticks(i, a, 0)?).day_of_year as i32))));
    interp.register_native(key("get_Date()"), |i, a| {
        let t = ticks(i, a, 0)?;
        Ok(Some(Value::I64(t - t.rem_euclid(TICKS_PER_DAY))))
    });
    interp.register_native(key("get_TimeOfDay()"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)?.rem_euclid(TICKS_PER_DAY)))));
    interp.register_native(key("get_Kind()"), |_, _| Ok(Some(Value::I32(0))));

    for (name, unit) in [
        ("AddDays(double)", TICKS_PER_DAY),
        ("AddHours(double)", TICKS_PER_HOUR),
        ("AddMinutes(double)", TICKS_PER_MINUTE),
        ("AddSeconds(double)", TICKS_PER_SECOND),
        ("AddMilliseconds(double)", TICKS_PER_MS),
    ] {
        let unit_f = unit as f64;
        let f: rustclr_core::NativeFn = match unit {
            TICKS_PER_DAY => |i, a| add_scaled(i, a, TICKS_PER_DAY as f64),
            TICKS_PER_HOUR => |i, a| add_scaled(i, a, TICKS_PER_HOUR as f64),
            TICKS_PER_MINUTE => |i, a| add_scaled(i, a, TICKS_PER_MINUTE as f64),
            TICKS_PER_SECOND => |i, a| add_scaled(i, a, TICKS_PER_SECOND as f64),
            _ => |i, a| add_scaled(i, a, TICKS_PER_MS as f64),
        };
        let _ = unit_f;
        interp.register_native(key(name), f);
    }
    interp.register_native(key("AddTicks(long)"), |i, a| date_value(ticks(i, a, 0)? + arg_i64(i, a, 1)?));
    interp.register_native(key("AddMonths(int)"), |i, a| date_value(format::add_months(ticks(i, a, 0)?, arg_i32(i, a, 1)? as i64)));
    interp.register_native(key("AddYears(int)"), |i, a| date_value(format::add_months(ticks(i, a, 0)?, arg_i32(i, a, 1)? as i64 * 12)));
    interp.register_native(key("Add/1"), |i, a| date_value(ticks(i, a, 0)? + arg_i64(i, a, 1)?));
    interp.register_native(key("op_Addition/2"), |i, a| date_value(ticks(i, a, 0)? + arg_i64(i, a, 1)?));
    // Subtract a DateTime → TimeSpan; subtract a TimeSpan → DateTime. Both are
    // a 64-bit slot, so the declared parameter type decides.
    for name in ["Subtract/1", "op_Subtraction/2"] {
        interp.register_native(key(name), |i, a| {
            let index = if a.len() == 2 { 1 } else { 1 };
            let param = param_type_name(i, if i.loader.registry.method(i.current_native_method().unwrap()).signature.has_this { 0 } else { 1 });
            let (x, y) = (ticks(i, a, 0)?, ticks(i, a, index)?);
            if param.as_deref() == Some("System.TimeSpan") {
                date_value(x - y)
            } else {
                Ok(Some(Value::I64(x - y)))
            }
        });
    }
    for (name, op) in [
        ("op_Equality/2", 0),
        ("op_Inequality/2", 1),
        ("op_LessThan/2", 2),
        ("op_GreaterThan/2", 3),
        ("op_LessThanOrEqual/2", 4),
        ("op_GreaterThanOrEqual/2", 5),
    ] {
        let f: rustclr_core::NativeFn = match op {
            0 => |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? == ticks(i, a, 1)?) as i32))),
            1 => |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? != ticks(i, a, 1)?) as i32))),
            2 => |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? < ticks(i, a, 1)?) as i32))),
            3 => |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? > ticks(i, a, 1)?) as i32))),
            4 => |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? <= ticks(i, a, 1)?) as i32))),
            _ => |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? >= ticks(i, a, 1)?) as i32))),
        };
        interp.register_native(key(name), f);
        interp.register_native(k(format!("System.TimeSpan::{name}")), f);
    }
    for name in ["CompareTo/1", "Compare/2"] {
        interp.register_native(key(name), |i, a| Ok(Some(Value::I32(ticks(i, a, 0)?.cmp(&ticks(i, a, 1)?) as i32))));
        interp.register_native(k(format!("System.TimeSpan::{name}")), |i, a| {
            Ok(Some(Value::I32(ticks(i, a, 0)?.cmp(&ticks(i, a, 1)?) as i32)))
        });
    }
    interp.register_native(key("Equals/1"), |i, a| {
        let other = { let v__ = arg(i, a, 1)?; unbox_value(i, &v__) };
        Ok(Some(Value::I32((other.as_i64() == Some(ticks(i, a, 0)?)) as i32)))
    });
    interp.register_native(key("GetHashCode()"), |i, a| Ok(Some(Value::I32(ticks(i, a, 0)? as i32))));

    interp.register_native(key("ToString()"), |i, a| {
        let text = format::format_date(ticks(i, a, 0)?, "");
        Ok(Some(string_value(i, &text)))
    });
    for name in ["ToString(string)", "ToString/1", "ToString/2"] {
        interp.register_native(key(name), |i, a| {
            let spec = match arg(i, a, 1)? {
                Value::Obj(h) => i.string_value(h).unwrap_or_default(),
                _ => String::new(),
            };
            let text = format::format_date(ticks(i, a, 0)?, &spec);
            Ok(Some(string_value(i, &text)))
        });
    }
    for (name, pattern) in [
        ("ToShortDateString()", "d"),
        ("ToLongDateString()", "D"),
        ("ToShortTimeString()", "t"),
        ("ToLongTimeString()", "T"),
    ] {
        let f: rustclr_core::NativeFn = match pattern {
            "d" => |i, a| date_string(i, a, "d"),
            "D" => |i, a| date_string(i, a, "D"),
            "t" => |i, a| date_string(i, a, "t"),
            _ => |i, a| date_string(i, a, "T"),
        };
        interp.register_native(key(name), f);
    }
    for name in ["Parse(string)", "Parse/2", "Parse/3"] {
        interp.register_native(key(name), |i, a| {
            let s = arg_string(i, a, 0)?;
            format::parse_date(&s).map(|t| Some(Value::I64(t))).ok_or_else(|| bad_format(&s))
        });
    }
    // ParseExact(string, string | string[], IFormatProvider[, DateTimeStyles]):
    // the invariant culture, and styles are not applied.
    for name in ["ParseExact/3", "ParseExact/4"] {
        interp.register_native(key(name), |i, a| {
            let s = arg_string(i, a, 0)?;
            parse_exact_any(i, a, &s)?.map(|t| Some(Value::I64(t))).ok_or_else(|| bad_format(&s))
        });
    }
    interp.register_native(key("TryParseExact/5"), |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let parsed = parse_exact_any(i, a, &s)?;
        store_out(i, a, 4, Value::I64(parsed.unwrap_or(0)))?;
        Ok(Some(Value::I32(parsed.is_some() as i32)))
    });
    for name in ["TryParse/2", "TryParse/3", "TryParse/4"] {
        interp.register_native(key(name), |i, a| {
            let s = arg_string_or_empty(i, a, 0)?;
            let parsed = format::parse_date(&s);
            store_out(i, a, a.len() - 1, Value::I64(parsed.unwrap_or(0)))?;
            Ok(Some(Value::I32(parsed.is_some() as i32)))
        });
    }
    interp.register_native(key("DaysInMonth(int,int)"), |i, a| {
        Ok(Some(Value::I32(format::days_in_month(arg_i32(i, a, 0)? as i64, arg_i32(i, a, 1)? as i64) as i32)))
    });
    interp.register_native(key("IsLeapYear(int)"), |i, a| Ok(Some(Value::I32(format::is_leap(arg_i32(i, a, 0)? as i64) as i32))));
    interp.register_native(key("SpecifyKind/2"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)?))));
    interp.register_native(key("ToUniversalTime()"), |i, a| {
        let offset = i.host.utc_offset_minutes() as i64 * TICKS_PER_MINUTE;
        Ok(Some(Value::I64(ticks(i, a, 0)? - offset)))
    });
    interp.register_native(key("ToLocalTime()"), |i, a| {
        let offset = i.host.utc_offset_minutes() as i64 * TICKS_PER_MINUTE;
        Ok(Some(Value::I64(ticks(i, a, 0)? + offset)))
    });
}

// ── DateTimeOffset ───────────────────────────────────────────────────────────
//
// One 64-bit slot, like `DateTime`: the instant as UTC ticks in the low 62
// bits, and bit 62 set when it reads in the host's offset (`Now`, `ToLocalTime`).
// Any other offset keeps the instant exactly and reads as UTC; see
// `docs/limitations.md`. `default(DateTimeOffset)` is 0001-01-01 +00:00, as in .NET.

const DTO_LOCAL: i64 = 1 << 62;
const DTO_TICKS: i64 = DTO_LOCAL - 1;

fn host_offset_ticks(i: &mut Interpreter) -> i64 {
    i.host.utc_offset_minutes() as i64 * TICKS_PER_MINUTE
}

/// `(utc ticks, offset ticks)` of the offset in argument `index`.
fn dto(i: &mut Interpreter, a: &[Value], index: usize) -> ExecResult<(i64, i64)> {
    let raw = arg(i, a, index)?.as_i64().unwrap_or(0);
    let offset = if raw & DTO_LOCAL != 0 { host_offset_ticks(i) } else { 0 };
    Ok((raw & DTO_TICKS, offset))
}

/// Encodes an instant. An offset that is neither UTC nor the host's cannot be
/// stored, so the value reads as UTC; the instant itself is exact.
fn dto_value(i: &mut Interpreter, utc: i64, offset: i64) -> ExecResult<Option<Value>> {
    if !(0..=3_155_378_975_999_999_999).contains(&utc) {
        return Err(out_range("The UTC time represented when the offset is applied must be between year 0 and 10,000."));
    }
    let local = offset != 0 && offset == host_offset_ticks(i);
    Ok(Some(Value::I64(if local { utc | DTO_LOCAL } else { utc })))
}

fn dto_clock(i: &mut Interpreter, a: &[Value], index: usize) -> ExecResult<i64> {
    let (utc, offset) = dto(i, a, index)?;
    Ok(utc + offset)
}

/// Clock ticks in a given offset → the stored value.
fn dto_from_clock(i: &mut Interpreter, clock: i64, offset: i64) -> ExecResult<Option<Value>> {
    if offset % TICKS_PER_MINUTE != 0 || offset.abs() > 14 * TICKS_PER_HOUR {
        return Err(argument("Offset must be within plus or minus 14 hours and specified in whole minutes."));
    }
    dto_value(i, clock - offset, offset)
}

fn dto_with_utc(i: &mut Interpreter, a: &[Value], utc: i64) -> ExecResult<Option<Value>> {
    let (_, offset) = dto(i, a, 0)?;
    dto_value(i, utc, offset)
}

fn dto_part(i: &mut Interpreter, a: &[Value]) -> ExecResult<format::Parts> {
    Ok(format::parts(dto_clock(i, a, 0)?))
}

fn dto_add_scaled(i: &mut Interpreter, a: &[Value], unit: f64) -> ExecResult<Option<Value>> {
    let (utc, _) = dto(i, a, 0)?;
    let ms = crate::fmath::round(arg_f64(i, a, 1)? * unit / TICKS_PER_MS as f64) as i64;
    dto_with_utc(i, a, utc + ms * TICKS_PER_MS)
}

fn dto_string(i: &mut Interpreter, a: &[Value], spec: &str) -> ExecResult<Option<Value>> {
    let (utc, offset) = dto(i, a, 0)?;
    let text = format::format_date_offset(utc + offset, offset / TICKS_PER_MINUTE, spec);
    Ok(Some(string_value(i, &text)))
}

/// Text → stored value. Without an offset in the text it is local, as in .NET.
fn dto_parse(i: &mut Interpreter, text: &str) -> Option<i64> {
    let (clock_text, minutes) = format::split_offset(text);
    let clock = format::parse_date(clock_text)?;
    let offset = match minutes {
        Some(m) => m * TICKS_PER_MINUTE,
        None => host_offset_ticks(i),
    };
    dto_from_clock(i, clock, offset).ok().flatten().and_then(|v| v.as_i64())
}

fn register_datetimeoffset(interp: &mut Interpreter) {
    const DTO: &str = "System.DateTimeOffset";
    let key = |m: &str| k(format!("{DTO}::{m}"));

    interp.register_native(key("get_UtcNow()"), |i, _| {
        let utc = now_ticks(i, false);
        dto_value(i, utc, 0)
    });
    interp.register_native(key("get_Now()"), |i, _| {
        let (utc, offset) = (now_ticks(i, false), host_offset_ticks(i));
        dto_value(i, utc, offset)
    });
    // From a DateTime: `Kind` is not tracked, so its clock is taken as UTC.
    for name in [".ctor/1", "op_Implicit/1"] {
        interp.register_native(key(name), |i, a| construct_with(i, a, |i, a| {
            let first = if matches!(a.first(), Some(Value::Ref(_))) { 1 } else { 0 };
            let clock = arg_i64(i, a, first)?;
            dto_value(i, clock, 0)
        }));
    }
    // (DateTime, TimeSpan) and (long ticks, TimeSpan): both clock ticks.
    interp.register_native(key(".ctor/2"), |i, a| construct_with(i, a, |i, a| {
        let (clock, offset) = (arg_i64(i, a, 1)?, arg_i64(i, a, 2)?);
        dto_from_clock(i, clock, offset)
    }));
    interp.register_native(key(".ctor/7"), |i, a| construct_with(i, a, |i, a| {
        let v: Vec<i64> = (1..=6).map(|n| arg_i32(i, a, n).map(|x| x as i64)).collect::<ExecResult<_>>()?;
        let clock = checked_date(v[0], v[1], v[2], v[3], v[4], v[5], 0)?.and_then(|x| x.as_i64()).unwrap_or(0);
        let offset = arg_i64(i, a, 7)?;
        dto_from_clock(i, clock, offset)
    }));
    interp.register_native(key(".ctor/8"), |i, a| construct_with(i, a, |i, a| {
        let v: Vec<i64> = (1..=7).map(|n| arg_i32(i, a, n).map(|x| x as i64)).collect::<ExecResult<_>>()?;
        let clock = checked_date(v[0], v[1], v[2], v[3], v[4], v[5], v[6])?.and_then(|x| x.as_i64()).unwrap_or(0);
        let offset = arg_i64(i, a, 8)?;
        dto_from_clock(i, clock, offset)
    }));

    interp.register_native(key("FromUnixTimeMilliseconds(long)"), |i, a| {
        let utc = arg_i64(i, a, 0)? * TICKS_PER_MS + format::UNIX_EPOCH_TICKS;
        dto_value(i, utc, 0)
    });
    interp.register_native(key("FromUnixTimeSeconds(long)"), |i, a| {
        let utc = arg_i64(i, a, 0)? * TICKS_PER_SECOND + format::UNIX_EPOCH_TICKS;
        dto_value(i, utc, 0)
    });
    interp.register_native(key("ToUnixTimeMilliseconds()"), |i, a| {
        let (utc, _) = dto(i, a, 0)?;
        Ok(Some(Value::I64((utc - format::UNIX_EPOCH_TICKS).div_euclid(TICKS_PER_MS))))
    });
    interp.register_native(key("ToUnixTimeSeconds()"), |i, a| {
        let (utc, _) = dto(i, a, 0)?;
        Ok(Some(Value::I64((utc - format::UNIX_EPOCH_TICKS).div_euclid(TICKS_PER_SECOND))))
    });

    interp.register_native(key("get_UtcTicks()"), |i, a| Ok(Some(Value::I64(dto(i, a, 0)?.0))));
    interp.register_native(key("get_UtcDateTime()"), |i, a| Ok(Some(Value::I64(dto(i, a, 0)?.0))));
    for name in ["get_Ticks()", "get_DateTime()", "get_ClockDateTime()"] {
        interp.register_native(key(name), |i, a| Ok(Some(Value::I64(dto_clock(i, a, 0)?))));
    }
    interp.register_native(key("get_LocalDateTime()"), |i, a| {
        let (utc, _) = dto(i, a, 0)?;
        let offset = host_offset_ticks(i);
        Ok(Some(Value::I64(utc + offset)))
    });
    interp.register_native(key("get_Offset()"), |i, a| Ok(Some(Value::I64(dto(i, a, 0)?.1))));
    interp.register_native(key("get_TotalOffsetMinutes()"), |i, a| Ok(Some(Value::I32((dto(i, a, 0)?.1 / TICKS_PER_MINUTE) as i32))));
    interp.register_native(key("get_Year()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.year as i32))));
    interp.register_native(key("get_Month()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.month as i32))));
    interp.register_native(key("get_Day()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.day as i32))));
    interp.register_native(key("get_Hour()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.hour as i32))));
    interp.register_native(key("get_Minute()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.minute as i32))));
    interp.register_native(key("get_Second()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.second as i32))));
    interp.register_native(key("get_Millisecond()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.millisecond as i32))));
    interp.register_native(key("get_DayOfWeek()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.day_of_week as i32))));
    interp.register_native(key("get_DayOfYear()"), |i, a| Ok(Some(Value::I32(dto_part(i, a)?.day_of_year as i32))));
    interp.register_native(key("get_Date()"), |i, a| {
        let clock = dto_clock(i, a, 0)?;
        Ok(Some(Value::I64(clock - clock.rem_euclid(TICKS_PER_DAY))))
    });
    interp.register_native(key("get_TimeOfDay()"), |i, a| Ok(Some(Value::I64(dto_clock(i, a, 0)?.rem_euclid(TICKS_PER_DAY)))));

    for (name, unit) in [
        ("AddDays(double)", 0),
        ("AddHours(double)", 1),
        ("AddMinutes(double)", 2),
        ("AddSeconds(double)", 3),
        ("AddMilliseconds(double)", 4),
    ] {
        let f: rustclr_core::NativeFn = match unit {
            0 => |i, a| dto_add_scaled(i, a, TICKS_PER_DAY as f64),
            1 => |i, a| dto_add_scaled(i, a, TICKS_PER_HOUR as f64),
            2 => |i, a| dto_add_scaled(i, a, TICKS_PER_MINUTE as f64),
            3 => |i, a| dto_add_scaled(i, a, TICKS_PER_SECOND as f64),
            _ => |i, a| dto_add_scaled(i, a, TICKS_PER_MS as f64),
        };
        interp.register_native(key(name), f);
    }
    for name in ["AddTicks(long)", "Add/1", "op_Addition/2"] {
        interp.register_native(key(name), |i, a| {
            let (utc, _) = dto(i, a, 0)?;
            let delta = arg_i64(i, a, 1)?;
            dto_with_utc(i, a, utc + delta)
        });
    }
    // Calendar arithmetic happens on the clock, then maps back to an instant.
    for (name, scale) in [("AddMonths(int)", 1i64), ("AddYears(int)", 12)] {
        let f: rustclr_core::NativeFn = if scale == 1 {
            |i, a| {
                let (utc, offset) = dto(i, a, 0)?;
                let clock = format::add_months(utc + offset, arg_i32(i, a, 1)? as i64);
                dto_value(i, clock - offset, offset)
            }
        } else {
            |i, a| {
                let (utc, offset) = dto(i, a, 0)?;
                let clock = format::add_months(utc + offset, arg_i32(i, a, 1)? as i64 * 12);
                dto_value(i, clock - offset, offset)
            }
        };
        interp.register_native(key(name), f);
    }
    // Subtract an offset → TimeSpan; subtract a TimeSpan → DateTimeOffset.
    for name in ["Subtract/1", "op_Subtraction/2"] {
        interp.register_native(key(name), |i, a| {
            let has_this = i.loader.registry.method(i.current_native_method().unwrap()).signature.has_this;
            let param = param_type_name(i, if has_this { 0 } else { 1 });
            let (x, _) = dto(i, a, 0)?;
            if param.as_deref() == Some("System.TimeSpan") {
                let span = arg_i64(i, a, 1)?;
                dto_with_utc(i, a, x - span)
            } else {
                let (y, _) = dto(i, a, 1)?;
                Ok(Some(Value::I64(x - y)))
            }
        });
    }
    // Comparisons are on the instant, whatever the offsets.
    for (name, op) in [
        ("op_Equality/2", 0),
        ("op_Inequality/2", 1),
        ("op_LessThan/2", 2),
        ("op_GreaterThan/2", 3),
        ("op_LessThanOrEqual/2", 4),
        ("op_GreaterThanOrEqual/2", 5),
    ] {
        let f: rustclr_core::NativeFn = match op {
            0 => |i, a| Ok(Some(Value::I32((dto(i, a, 0)?.0 == dto(i, a, 1)?.0) as i32))),
            1 => |i, a| Ok(Some(Value::I32((dto(i, a, 0)?.0 != dto(i, a, 1)?.0) as i32))),
            2 => |i, a| Ok(Some(Value::I32((dto(i, a, 0)?.0 < dto(i, a, 1)?.0) as i32))),
            3 => |i, a| Ok(Some(Value::I32((dto(i, a, 0)?.0 > dto(i, a, 1)?.0) as i32))),
            4 => |i, a| Ok(Some(Value::I32((dto(i, a, 0)?.0 <= dto(i, a, 1)?.0) as i32))),
            _ => |i, a| Ok(Some(Value::I32((dto(i, a, 0)?.0 >= dto(i, a, 1)?.0) as i32))),
        };
        interp.register_native(key(name), f);
    }
    for name in ["CompareTo/1", "Compare/2"] {
        interp.register_native(key(name), |i, a| Ok(Some(Value::I32(dto(i, a, 0)?.0.cmp(&dto(i, a, 1)?.0) as i32))));
    }
    interp.register_native(key("Equals/1"), |i, a| {
        let other = { let v__ = arg(i, a, 1)?; unbox_value(i, &v__) };
        let (x, _) = dto(i, a, 0)?;
        Ok(Some(Value::I32((other.as_i64().map(|o| o & DTO_TICKS) == Some(x)) as i32)))
    });
    interp.register_native(key("EqualsExact/1"), |i, a| {
        let same = arg(i, a, 0)?.as_i64() == arg(i, a, 1)?.as_i64();
        Ok(Some(Value::I32(same as i32)))
    });
    interp.register_native(key("GetHashCode()"), |i, a| {
        let (x, _) = dto(i, a, 0)?;
        Ok(Some(Value::I32((x ^ (x >> 32)) as i32)))
    });

    interp.register_native(key("ToUniversalTime()"), |i, a| {
        let (utc, _) = dto(i, a, 0)?;
        dto_value(i, utc, 0)
    });
    interp.register_native(key("ToLocalTime()"), |i, a| {
        let (utc, _) = dto(i, a, 0)?;
        let offset = host_offset_ticks(i);
        dto_value(i, utc, offset)
    });
    interp.register_native(key("ToOffset/1"), |i, a| {
        let (utc, _) = dto(i, a, 0)?;
        let offset = arg_i64(i, a, 1)?;
        dto_value(i, utc, offset)
    });

    interp.register_native(key("ToString()"), |i, a| dto_string(i, a, ""));
    for name in ["ToString(string)", "ToString/1", "ToString/2"] {
        interp.register_native(key(name), |i, a| {
            let spec = match arg(i, a, 1)? {
                Value::Obj(h) => i.string_value(h).unwrap_or_default(),
                _ => String::new(),
            };
            dto_string(i, a, &spec)
        });
    }
    for name in ["Parse(string)", "Parse/2", "Parse/3"] {
        interp.register_native(key(name), |i, a| {
            let s = arg_string(i, a, 0)?;
            dto_parse(i, &s).map(|t| Some(Value::I64(t))).ok_or_else(|| bad_format(&s))
        });
    }
    for name in ["TryParse/2", "TryParse/3", "TryParse/4"] {
        interp.register_native(key(name), |i, a| {
            let s = arg_string_or_empty(i, a, 0)?;
            let parsed = dto_parse(i, &s);
            store_out(i, a, a.len() - 1, Value::I64(parsed.unwrap_or(0)))?;
            Ok(Some(Value::I32(parsed.is_some() as i32)))
        });
    }
}

/// The values of the static fields the loader declares on `TimeSpan`,
/// `DateTime`, `DateTimeOffset` and `Guid`. They are constants, so they are
/// set once here rather than by a type initializer.
fn fill_value_statics(interp: &mut Interpreter) {
    const MAX_TICKS: i64 = 3_155_378_975_999_999_999;
    for (ty, name, value) in [
        ("System.TimeSpan", "Zero", Value::I64(0)),
        ("System.TimeSpan", "MinValue", Value::I64(i64::MIN)),
        ("System.TimeSpan", "MaxValue", Value::I64(i64::MAX)),
        ("System.DateTime", "MinValue", Value::I64(0)),
        ("System.DateTime", "MaxValue", Value::I64(MAX_TICKS)),
        ("System.DateTime", "UnixEpoch", Value::I64(format::UNIX_EPOCH_TICKS)),
        ("System.DateTimeOffset", "MinValue", Value::I64(0)),
        ("System.DateTimeOffset", "MaxValue", Value::I64(MAX_TICKS)),
        ("System.DateTimeOffset", "UnixEpoch", Value::I64(format::UNIX_EPOCH_TICKS)),
        // A null Guid reads as the empty one everywhere, and installing must
        // not allocate: the profiler charges every allocation to a program.
        ("System.Guid", "Empty", Value::Null),
    ] {
        let Some(type_id) = interp.loader.registry.find_type_by_name(ty) else { continue };
        let field = interp
            .loader
            .registry
            .ty(type_id)
            .static_fields
            .iter()
            .copied()
            .find(|f| interp.loader.registry.field(*f).name == name);
        if let Some(field) = field {
            interp.loader.set_static(field, value);
        }
    }
}

/// How a boxed or formatted `DateTimeOffset` prints.
pub(crate) fn format_datetimeoffset(interp: &mut Interpreter, raw: i64, spec: &str) -> String {
    let offset = if raw & DTO_LOCAL != 0 { host_offset_ticks(interp) } else { 0 };
    let utc = raw & DTO_TICKS;
    format::format_date_offset(utc + offset, offset / TICKS_PER_MINUTE, spec)
}

/// A value-type constructor's result. `newobj` takes it as the return value;
/// `ldloca x; call .ctor` (how C# initialises a local in place) passes `this`
/// as a reference, and the value has to be written through it.
fn construct(i: &mut Interpreter, a: &[Value], result: ExecResult<Option<Value>>) -> ExecResult<Option<Value>> {
    let value = result?.unwrap_or(Value::I64(0));
    if let Some(Value::Ref(r)) = a.first() {
        let r: ByRef = r.clone();
        i.store_indirect_public(r, value.clone())?;
    }
    Ok(Some(value))
}

fn construct_with(
    i: &mut Interpreter,
    a: &[Value],
    body: fn(&mut Interpreter, &[Value]) -> ExecResult<Option<Value>>,
) -> ExecResult<Option<Value>> {
    let result = body(i, a);
    construct(i, a, result)
}

fn date_string(i: &mut Interpreter, a: &[Value], pattern: &str) -> ExecResult<Option<Value>> {
    let text = format::format_date(ticks(i, a, 0)?, pattern);
    Ok(Some(string_value(i, &text)))
}

fn checked_date(y: i64, m: i64, d: i64, h: i64, min: i64, s: i64, ms: i64) -> ExecResult<Option<Value>> {
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > format::days_in_month(y, m) {
        return Err(out_range("Year, Month, and Day parameters describe an un-representable DateTime."));
    }
    if !(0..24).contains(&h) || !(0..60).contains(&min) || !(0..60).contains(&s) || !(0..1000).contains(&ms) {
        return Err(out_range("Hour, Minute, and Second parameters describe an un-representable DateTime."));
    }
    Ok(Some(Value::I64(format::ticks_from(y, m, d, h, min, s, ms))))
}

fn add_scaled(i: &mut Interpreter, a: &[Value], unit: f64) -> ExecResult<Option<Value>> {
    let t = ticks(i, a, 0)?;
    let amount = arg_f64(i, a, 1)?;
    // .NET rounds the scaled value to whole milliseconds.
    let ms = crate::fmath::round(amount * unit / TICKS_PER_MS as f64) as i64;
    date_value(t + ms * TICKS_PER_MS)
}

// ── TimeSpan ─────────────────────────────────────────────────────────────────

fn register_timespan(interp: &mut Interpreter) {
    const TS: &str = "System.TimeSpan";
    for name in ["Parse(string)", "Parse/2"] {
        interp.register_native(k(format!("{TS}::{name}")), |i, a| {
            let s = arg_string(i, a, 0)?;
            format::parse_timespan(&s).map(|t| Some(Value::I64(t))).ok_or_else(|| bad_format(&s))
        });
    }
    for name in ["TryParse/2", "TryParse/3"] {
        interp.register_native(k(format!("{TS}::{name}")), |i, a| {
            let s = arg_string_or_empty(i, a, 0)?;
            let parsed = format::parse_timespan(&s);
            store_out(i, a, a.len() - 1, Value::I64(parsed.unwrap_or(0)))?;
            Ok(Some(Value::I32(parsed.is_some() as i32)))
        });
    }
    let key = |m: &str| k(format!("{TS}::{m}"));
    interp.register_native(key(".ctor(long)"), |i, a| construct_with(i, a, |i, a| Ok(Some(Value::I64(arg_i64(i, a, 1)?)))));
    interp.register_native(key(".ctor(int,int,int)"), |i, a| construct_with(i, a, |i, a| {
        let (h, m, s) = (arg_i32(i, a, 1)? as i64, arg_i32(i, a, 2)? as i64, arg_i32(i, a, 3)? as i64);
        Ok(Some(Value::I64(h * TICKS_PER_HOUR + m * TICKS_PER_MINUTE + s * TICKS_PER_SECOND)))
    }));
    interp.register_native(key(".ctor(int,int,int,int)"), |i, a| construct_with(i, a, |i, a| {
        let v: Vec<i64> = (1..=4).map(|n| arg_i32(i, a, n).map(|x| x as i64)).collect::<ExecResult<_>>()?;
        Ok(Some(Value::I64(v[0] * TICKS_PER_DAY + v[1] * TICKS_PER_HOUR + v[2] * TICKS_PER_MINUTE + v[3] * TICKS_PER_SECOND)))
    }));
    interp.register_native(key(".ctor(int,int,int,int,int)"), |i, a| construct_with(i, a, |i, a| {
        let v: Vec<i64> = (1..=5).map(|n| arg_i32(i, a, n).map(|x| x as i64)).collect::<ExecResult<_>>()?;
        Ok(Some(Value::I64(
            v[0] * TICKS_PER_DAY + v[1] * TICKS_PER_HOUR + v[2] * TICKS_PER_MINUTE + v[3] * TICKS_PER_SECOND + v[4] * TICKS_PER_MS,
        )))
    }));
    // FromX(double) and .NET 9's FromX(long) / FromX(int).
    for (name, unit) in [
        ("FromDays", TICKS_PER_DAY),
        ("FromHours", TICKS_PER_HOUR),
        ("FromMinutes", TICKS_PER_MINUTE),
        ("FromSeconds", TICKS_PER_SECOND),
        ("FromMilliseconds", TICKS_PER_MS),
        ("FromTicks", 1),
    ] {
        let f: rustclr_core::NativeFn = match unit {
            TICKS_PER_DAY => |i, a| from_unit(i, a, TICKS_PER_DAY),
            TICKS_PER_HOUR => |i, a| from_unit(i, a, TICKS_PER_HOUR),
            TICKS_PER_MINUTE => |i, a| from_unit(i, a, TICKS_PER_MINUTE),
            TICKS_PER_SECOND => |i, a| from_unit(i, a, TICKS_PER_SECOND),
            TICKS_PER_MS => |i, a| from_unit(i, a, TICKS_PER_MS),
            _ => |i, a| from_unit(i, a, 1),
        };
        for shape in ["(double)", "(long)", "(int)", "/1"] {
            interp.register_native(k(format!("{TS}::{name}{shape}")), f);
        }
    }
    let part: [(&str, rustclr_core::NativeFn); 11] = [
        ("get_Ticks()", |i, a| Ok(Some(Value::I64(ticks(i, a, 0)?)))),
        ("get_Days()", |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? / TICKS_PER_DAY) as i32)))),
        ("get_Hours()", |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? % TICKS_PER_DAY / TICKS_PER_HOUR) as i32)))),
        ("get_Minutes()", |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? % TICKS_PER_HOUR / TICKS_PER_MINUTE) as i32)))),
        ("get_Seconds()", |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? % TICKS_PER_MINUTE / TICKS_PER_SECOND) as i32)))),
        ("get_Milliseconds()", |i, a| Ok(Some(Value::I32((ticks(i, a, 0)? % TICKS_PER_SECOND / TICKS_PER_MS) as i32)))),
        ("get_TotalDays()", |i, a| Ok(Some(Value::F(ticks(i, a, 0)? as f64 / TICKS_PER_DAY as f64)))),
        ("get_TotalHours()", |i, a| Ok(Some(Value::F(ticks(i, a, 0)? as f64 / TICKS_PER_HOUR as f64)))),
        ("get_TotalMinutes()", |i, a| Ok(Some(Value::F(ticks(i, a, 0)? as f64 / TICKS_PER_MINUTE as f64)))),
        ("get_TotalSeconds()", |i, a| Ok(Some(Value::F(ticks(i, a, 0)? as f64 / TICKS_PER_SECOND as f64)))),
        ("get_TotalMilliseconds()", |i, a| Ok(Some(Value::F(ticks(i, a, 0)? as f64 / TICKS_PER_MS as f64)))),
    ];
    for (name, f) in part {
        interp.register_native(key(name), f);
    }
    interp.register_native(key("Add/1"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)? + ticks(i, a, 1)?))));
    interp.register_native(key("Subtract/1"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)? - ticks(i, a, 1)?))));
    interp.register_native(key("op_Addition/2"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)? + ticks(i, a, 1)?))));
    interp.register_native(key("op_Subtraction/2"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)? - ticks(i, a, 1)?))));
    interp.register_native(key("op_UnaryNegation/1"), |i, a| Ok(Some(Value::I64(-ticks(i, a, 0)?))));
    interp.register_native(key("Negate()"), |i, a| Ok(Some(Value::I64(-ticks(i, a, 0)?))));
    interp.register_native(key("Duration()"), |i, a| Ok(Some(Value::I64(ticks(i, a, 0)?.abs()))));
    interp.register_native(key("Equals/1"), |i, a| {
        let other = { let v__ = arg(i, a, 1)?; unbox_value(i, &v__) };
        Ok(Some(Value::I32((other.as_i64() == Some(ticks(i, a, 0)?)) as i32)))
    });
    interp.register_native(key("ToString()"), |i, a| {
        let text = format::format_timespan(ticks(i, a, 0)?, "");
        Ok(Some(string_value(i, &text)))
    });
    for name in ["ToString(string)", "ToString/2"] {
        interp.register_native(key(name), |i, a| {
            let spec = arg_string_or_empty(i, a, 1)?;
            let text = format::format_timespan(ticks(i, a, 0)?, &spec);
            Ok(Some(string_value(i, &text)))
        });
    }
}

fn from_unit(i: &mut Interpreter, a: &[Value], unit: i64) -> ExecResult<Option<Value>> {
    let v = arg(i, a, 0)?;
    Ok(Some(Value::I64(match v {
        Value::F(f) => crate::fmath::round(f * unit as f64) as i64,
        other => other.as_i64().unwrap_or(0) * unit,
    })))
}

// ── strings ──────────────────────────────────────────────────────────────────

fn register_strings(interp: &mut Interpreter) {
    const S: &str = "System.String";
    let key = |m: &str| k(format!("{S}::{m}"));

    interp.register_native(key("IndexOf(char,int)"), |i, a| {
        let s = units(i, a, 0)?;
        let c = arg_i32(i, a, 1)? as u16;
        let from = arg_i32(i, a, 2)?.max(0) as usize;
        Ok(Some(Value::I32(s.iter().skip(from).position(|u| *u == c).map(|p| (p + from) as i32).unwrap_or(-1))))
    });
    interp.register_native(key("IndexOf(string,int)"), |i, a| {
        let (s, n) = (units(i, a, 0)?, units(i, a, 1)?);
        let from = arg_i32(i, a, 2)?.max(0) as usize;
        Ok(Some(Value::I32(find_units(&s, &n, from, false))))
    });
    interp.register_native(key("IndexOf(string,int,int)"), |i, a| {
        let (s, n) = (units(i, a, 0)?, units(i, a, 1)?);
        let from = arg_i32(i, a, 2)?.max(0) as usize;
        let count = arg_i32(i, a, 3)?.max(0) as usize;
        let end = (from + count).min(s.len());
        Ok(Some(Value::I32(find_units(&s[..end], &n, from, false))))
    });
    // IndexOf(string, StringComparison) and IndexOf(char, StringComparison).
    interp.register_native(key("IndexOf/2"), |i, a| {
        let s = units(i, a, 0)?;
        let needle = match arg(i, a, 1)? {
            Value::I32(c) => vec![c as u16],
            _ => units(i, a, 1)?,
        };
        let comparison = arg_i32(i, a, 2)?;
        Ok(Some(Value::I32(find_units(&s, &needle, 0, ignores_case(comparison)))))
    });
    interp.register_native(key("IndexOfAny(char[])"), |i, a| {
        let s = units(i, a, 0)?;
        let set: Vec<u16> = { let h__ = arg_handle(i, a, 1)?; array_values(i, h__) }.iter().map(|v| v.as_i32().unwrap_or(0) as u16).collect();
        Ok(Some(Value::I32(s.iter().position(|u| set.contains(u)).map(|p| p as i32).unwrap_or(-1))))
    });
    interp.register_native(key("LastIndexOf(char)"), |i, a| {
        let s = units(i, a, 0)?;
        let c = arg_i32(i, a, 1)? as u16;
        Ok(Some(Value::I32(s.iter().rposition(|u| *u == c).map(|p| p as i32).unwrap_or(-1))))
    });
    interp.register_native(key("LastIndexOf/2"), |i, a| {
        let s = units(i, a, 0)?;
        let n = units(i, a, 1)?;
        let ignore = ignores_case(arg_i32(i, a, 2)?);
        let mut last = -1;
        let mut from = 0;
        loop {
            let at = find_units(&s, &n, from, ignore);
            if at < 0 {
                break;
            }
            last = at;
            from = at as usize + 1;
            if from > s.len() {
                break;
            }
        }
        Ok(Some(Value::I32(last)))
    });
    interp.register_native(key("Contains(char)"), |i, a| {
        let s = units(i, a, 0)?;
        let c = arg_i32(i, a, 1)? as u16;
        Ok(Some(Value::I32(s.contains(&c) as i32)))
    });
    interp.register_native(key("Contains/2"), |i, a| {
        let (s, n) = (units(i, a, 0)?, units(i, a, 1)?);
        Ok(Some(Value::I32((find_units(&s, &n, 0, ignores_case(arg_i32(i, a, 2)?)) >= 0) as i32)))
    });
    interp.register_native(key("StartsWith(char)"), |i, a| {
        let s = units(i, a, 0)?;
        Ok(Some(Value::I32((s.first() == Some(&(arg_i32(i, a, 1)? as u16))) as i32)))
    });
    interp.register_native(key("EndsWith(char)"), |i, a| {
        let s = units(i, a, 0)?;
        Ok(Some(Value::I32((s.last() == Some(&(arg_i32(i, a, 1)? as u16))) as i32)))
    });
    for name in ["StartsWith/2", "StartsWith/3"] {
        interp.register_native(key(name), |i, a| {
            let (s, p) = (units(i, a, 0)?, units(i, a, 1)?);
            let ignore = match arg(i, a, 2)? {
                Value::I32(c) if a.len() == 3 => ignores_case(c),
                other => other.is_truthy(),
            };
            Ok(Some(Value::I32((p.len() <= s.len() && compare_units(&s[..p.len()], &p, ignore) == 0) as i32)))
        });
    }
    for name in ["EndsWith/2", "EndsWith/3"] {
        interp.register_native(key(name), |i, a| {
            let (s, p) = (units(i, a, 0)?, units(i, a, 1)?);
            let ignore = match arg(i, a, 2)? {
                Value::I32(c) if a.len() == 3 => ignores_case(c),
                other => other.is_truthy(),
            };
            Ok(Some(Value::I32((p.len() <= s.len() && compare_units(&s[s.len() - p.len()..], &p, ignore) == 0) as i32)))
        });
    }
    // Instance Equals(string, StringComparison) and static Equals(a, b, comparison).
    interp.register_native(key("Equals/2"), |i, a| string_equals(i, a));
    interp.register_native(key("Equals/3"), |i, a| string_equals(i, a));
    interp.register_native(key("Compare/3"), |i, a| {
        let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
        if x.is_null() || y.is_null() {
            return Ok(Some(Value::I32(match (x.is_null(), y.is_null()) {
                (true, true) => 0,
                (true, false) => -1,
                _ => 1,
            })));
        }
        let (sx, sy) = (units(i, a, 0)?, units(i, a, 1)?);
        let ignore = match arg(i, a, 2)? {
            Value::I32(c) => ignores_case(c),
            other => other.is_truthy(),
        };
        Ok(Some(Value::I32(compare_units(&sx, &sy, ignore))))
    });
    interp.register_native(key("Compare(string,string,bool)"), |i, a| {
        let (sx, sy) = (units(i, a, 0)?, units(i, a, 1)?);
        Ok(Some(Value::I32(compare_units(&sx, &sy, arg_bool(i, a, 2)?))))
    });
    interp.register_native(key("PadLeft(int,char)"), |i, a| pad(i, a, true));
    interp.register_native(key("PadRight(int,char)"), |i, a| pad(i, a, false));
    for (name, front, back) in [
        ("Trim(char)", true, true),
        ("Trim(char[])", true, true),
        ("TrimStart(char)", true, false),
        ("TrimStart(char[])", true, false),
        ("TrimEnd(char)", false, true),
        ("TrimEnd(char[])", false, true),
    ] {
        let f: rustclr_core::NativeFn = match (front, back) {
            (true, true) => |i, a| trim_chars(i, a, true, true),
            (true, false) => |i, a| trim_chars(i, a, true, false),
            _ => |i, a| trim_chars(i, a, false, true),
        };
        interp.register_native(key(name), f);
    }
    interp.register_native(key("ToUpperInvariant()"), |i, a| {
        let s = arg_string(i, a, 0)?.to_uppercase();
        Ok(Some(string_value(i, &s)))
    });
    interp.register_native(key("ToLowerInvariant()"), |i, a| {
        let s = arg_string(i, a, 0)?.to_lowercase();
        Ok(Some(string_value(i, &s)))
    });
    interp.register_native(key("ToUpper/1"), |i, a| {
        let s = arg_string(i, a, 0)?.to_uppercase();
        Ok(Some(string_value(i, &s)))
    });
    interp.register_native(key("ToLower/1"), |i, a| {
        let s = arg_string(i, a, 0)?.to_lowercase();
        Ok(Some(string_value(i, &s)))
    });
    interp.register_native(key("Insert(int,string)"), |i, a| {
        let mut s = units(i, a, 0)?;
        let at = arg_i32(i, a, 1)?;
        let add = units(i, a, 2)?;
        if at < 0 || at as usize > s.len() {
            return Err(out_range("startIndex"));
        }
        s.splice(at as usize..at as usize, add);
        Ok(Some(string_from_units(i, s)))
    });
    interp.register_native(key("Remove(int)"), |i, a| {
        let mut s = units(i, a, 0)?;
        let at = arg_i32(i, a, 1)?;
        if at < 0 || at as usize > s.len() {
            return Err(out_range("startIndex"));
        }
        s.truncate(at as usize);
        Ok(Some(string_from_units(i, s)))
    });
    interp.register_native(key("Remove(int,int)"), |i, a| {
        let mut s = units(i, a, 0)?;
        let (at, count) = (arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        if at < 0 || count < 0 || (at + count) as usize > s.len() {
            return Err(out_range("count"));
        }
        s.drain(at as usize..(at + count) as usize);
        Ok(Some(string_from_units(i, s)))
    });
    interp.register_native(key("Substring(int,int)"), |i, a| {
        let s = units(i, a, 0)?;
        let (at, len) = (arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        if at < 0 || len < 0 || (at + len) as usize > s.len() {
            return Err(out_range("length"));
        }
        Ok(Some(string_from_units(i, s[at as usize..(at + len) as usize].to_vec())))
    });
    // `new string(chars)`, `new string('*', 4)`, `new string(chars, start, length)`.
    interp.register_native(key(".ctor(char[])"), |i, a| {
        let chars: Vec<u16> = { let h__ = arg_handle(i, a, 1)?; array_values(i, h__) }.iter().map(|v| v.as_i32().unwrap_or(0) as u16).collect();
        Ok(Some(string_from_units(i, chars)))
    });
    interp.register_native(key(".ctor(char,int)"), |i, a| {
        let c = arg_i32(i, a, 1)? as u16;
        let n = arg_i32(i, a, 2)?.max(0) as usize;
        Ok(Some(string_from_units(i, vec![c; n])))
    });
    interp.register_native(key(".ctor(char[],int,int)"), |i, a| {
        let chars: Vec<u16> = { let h__ = arg_handle(i, a, 1)?; array_values(i, h__) }.iter().map(|v| v.as_i32().unwrap_or(0) as u16).collect();
        let (at, len) = (arg_i32(i, a, 2)?.max(0) as usize, arg_i32(i, a, 3)?.max(0) as usize);
        let end = (at + len).min(chars.len());
        Ok(Some(string_from_units(i, chars[at.min(end)..end].to_vec())))
    });
    interp.register_native(key("Replace/3"), |i, a| {
        let s = arg_string(i, a, 0)?;
        let from = arg_string(i, a, 1)?;
        let to = arg_string_or_empty(i, a, 2)?;
        Ok(Some(string_value(i, &s.replace(&from, &to))))
    });
    // Split with every separator shape: char, char[], string, string[],
    // plus a count and/or StringSplitOptions (1 = remove empty, 2 = trim).
    for name in ["Split(char[])", "Split/1", "Split/2", "Split/3", "Split(string)"] {
        interp.register_native(key(name), split);
    }
    // Join(char, string[]) and Join(char/string, IEnumerable<T>).
    interp.register_native(key("Join/2"), |i, a| {
        let separator = match arg(i, a, 0)? {
            Value::I32(c) => char::from_u32(c as u32).map(String::from).unwrap_or_default(),
            _ => arg_string_or_empty(i, a, 0)?,
        };
        let source = arg(i, a, 1)?;
        let items = sequence_values(i, &source);
        // Join<T> knows its T: an enum prints names, a DateTime a date.
        let hint = type_arg(i, 0);
        let parts: Vec<String> = items.iter().map(|v| format_value(i, v, hint, "")).collect();
        let joined = parts.join(&separator);
        Ok(Some(string_value(i, &joined)))
    });
    // string.Format with format specifiers and alignment applied.
    for name in ["Format(string,object)", "Format(string,object,object)", "Format(string,object,object,object)"] {
        interp.register_native(key(name), |i, a| {
            let format_text = arg_string(i, a, 0)?;
            let values: Vec<Value> = (1..a.len()).map(|n| arg(i, a, n)).collect::<ExecResult<_>>()?;
            let text = composite(i, &format_text, &values);
            Ok(Some(string_value(i, &text)))
        });
    }
    interp.register_native(key("Format(string,object[])"), |i, a| {
        let format_text = arg_string(i, a, 0)?;
        let values = { let h__ = arg_handle(i, a, 1)?; array_values(i, h__) };
        let text = composite(i, &format_text, &values);
        Ok(Some(string_value(i, &text)))
    });
    // Format(IFormatProvider, string, args...).
    for name in ["Format/3", "Format/4", "Format/5"] {
        interp.register_native(key(name), |i, a| {
            let format_text = arg_string(i, a, 1)?;
            let mut values: Vec<Value> = (2..a.len()).map(|n| arg(i, a, n)).collect::<ExecResult<_>>()?;
            // Format(provider, format, object[]).
            if values.len() == 1 {
                if let Value::Obj(h) = values[0] {
                    if i.heap.with::<ClrArray, _>(h, |_| ()).is_some() {
                        values = array_values(i, h);
                    }
                }
            }
            let text = composite(i, &format_text, &values);
            Ok(Some(string_value(i, &text)))
        });
    }
    interp.register_native(key("Concat/2"), |i, a| {
        let x = arg(i, a, 0)?;
        let y = arg(i, a, 1)?;
        let text = display(i, &x) + &display(i, &y);
        Ok(Some(string_value(i, &text)))
    });
    interp.register_native(key("op_Inequality/2"), |i, a| {
        let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
        Ok(Some(Value::I32(!values_equal(i, &x, &y) as i32)))
    });
    interp.register_native(key("get_Empty()"), |i, _| Ok(Some(string_value(i, ""))));
    interp.register_native(key("IsNullOrWhiteSpace(string)"), |i, a| {
        let v = arg(i, a, 0)?;
        let blank = match v {
            Value::Obj(h) if !h.is_null() => i.string_value(h).map(|s| s.trim().is_empty()).unwrap_or(true),
            _ => true,
        };
        Ok(Some(Value::I32(blank as i32)))
    });
}

fn string_equals(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
    if x.is_null() || y.is_null() {
        return Ok(Some(Value::I32((x.is_null() && y.is_null()) as i32)));
    }
    let (sx, sy) = (units(i, a, 0)?, units(i, a, 1)?);
    let ignore = ignores_case(arg_i32(i, a, 2)?);
    Ok(Some(Value::I32((sx.len() == sy.len() && compare_units(&sx, &sy, ignore) == 0) as i32)))
}

fn pad(i: &mut Interpreter, a: &[Value], left: bool) -> ExecResult<Option<Value>> {
    let mut s = units(i, a, 0)?;
    let width = arg_i32(i, a, 1)?.max(0) as usize;
    let c = arg_i32(i, a, 2)? as u16;
    if s.len() < width {
        let fill = vec![c; width - s.len()];
        if left {
            s.splice(0..0, fill);
        } else {
            s.extend(fill);
        }
    }
    Ok(Some(string_from_units(i, s)))
}

fn trim_chars(i: &mut Interpreter, a: &[Value], front: bool, back: bool) -> ExecResult<Option<Value>> {
    let s = units(i, a, 0)?;
    let set: Vec<u16> = match arg(i, a, 1)? {
        Value::I32(c) => vec![c as u16],
        Value::Obj(h) if !h.is_null() => array_values(i, h).iter().map(|v| v.as_i32().unwrap_or(0) as u16).collect(),
        _ => Vec::new(),
    };
    let is_trimmed = |u: &u16| {
        if set.is_empty() {
            char::from_u32(*u as u32).map(|c| c.is_whitespace()).unwrap_or(false)
        } else {
            set.contains(u)
        }
    };
    let mut start = 0;
    let mut end = s.len();
    if front {
        while start < end && is_trimmed(&s[start]) {
            start += 1;
        }
    }
    if back {
        while end > start && is_trimmed(&s[end - 1]) {
            end -= 1;
        }
    }
    Ok(Some(string_from_units(i, s[start..end].to_vec())))
}

fn split(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let s = arg_string(i, a, 0)?;
    // Separators.
    let mut separators: Vec<String> = Vec::new();
    match arg(i, a, 1)? {
        Value::I32(c) => separators.push(char::from_u32(c as u32).map(String::from).unwrap_or_default()),
        Value::Obj(h) if !h.is_null() => {
            if let Some(text) = i.string_value(h) {
                separators.push(text);
            } else {
                for v in array_values(i, h) {
                    match v {
                        Value::I32(c) => separators.push(char::from_u32(c as u32).map(String::from).unwrap_or_default()),
                        Value::Obj(sh) => separators.push(i.string_value(sh).unwrap_or_default()),
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    separators.retain(|s| !s.is_empty());
    // Count and options: Split(sep, options), Split(sep, count), Split(sep, count, options).
    let (mut count, mut options) = (i32::MAX, 0);
    match a.len() {
        3 => {
            let v = arg_i32(i, a, 2)?;
            // A char[] separator with an int is (separator, count); with
            // any other separator the int is options. .NET's overloads make
            // the distinction by type; StringSplitOptions values are 0..3.
            let is_count = matches!(param_type_name(i, 1).as_deref(), Some("System.Int32"));
            if is_count { count = v } else { options = v }
        }
        4 => {
            count = arg_i32(i, a, 2)?;
            options = arg_i32(i, a, 3)?;
        }
        _ => {}
    }
    let mut parts: Vec<String> = Vec::new();
    if separators.is_empty() {
        parts = s.split(char::is_whitespace).map(String::from).collect();
    } else {
        let mut rest = s.as_str();
        loop {
            if parts.len() as i32 == count - 1 {
                parts.push(rest.to_string());
                break;
            }
            let next = separators
                .iter()
                .filter_map(|sep| rest.find(sep.as_str()).map(|at| (at, sep.len())))
                .min_by_key(|(at, _)| *at);
            match next {
                Some((at, len)) => {
                    parts.push(rest[..at].to_string());
                    rest = &rest[at + len..];
                }
                None => {
                    parts.push(rest.to_string());
                    break;
                }
            }
        }
    }
    if options & 2 != 0 {
        parts = parts.into_iter().map(|p| p.trim().to_string()).collect();
    }
    if options & 1 != 0 {
        parts.retain(|p| !p.is_empty());
    }
    Ok(Some(Value::Obj(string_array(i, &parts))))
}

/// `string.Format` semantics: `{index[,alignment][:format]}`.
pub fn composite(i: &mut Interpreter, format_text: &str, values: &[Value]) -> String {
    let mut out = String::with_capacity(format_text.len());
    let mut chars = format_text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut spec = String::new();
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    spec.push(inner);
                }
                let (head, fmt) = match spec.split_once(':') {
                    Some((h, f)) => (h, f),
                    None => (spec.as_str(), ""),
                };
                let (index, alignment) = match head.split_once(',') {
                    Some((ix, al)) => (ix.trim(), al.trim().parse::<i32>().unwrap_or(0)),
                    None => (head.trim(), 0),
                };
                let Ok(index) = index.parse::<usize>() else { continue };
                let value = values.get(index).cloned().unwrap_or(Value::Null);
                let text = format_value(i, &value, None, fmt);
                out.push_str(&align(&text, alignment));
            }
            other => out.push(other),
        }
    }
    out
}

pub fn align(text: &str, alignment: i32) -> String {
    let width = alignment.unsigned_abs() as usize;
    let len = text.encode_utf16().count();
    if len >= width {
        return text.to_string();
    }
    let padding = " ".repeat(width - len);
    if alignment > 0 { format!("{padding}{text}") } else { format!("{text}{padding}") }
}

// ── StringBuilder ────────────────────────────────────────────────────────────

fn sb_get(i: &mut Interpreter, this: Handle) -> String {
    match i.heap.with::<ClrObject, _>(this, |o| o.fields.first().cloned()).flatten() {
        Some(Value::Obj(h)) => i.string_value(h).unwrap_or_default(),
        _ => String::new(),
    }
}

fn sb_set(i: &mut Interpreter, this: Handle, text: &str) {
    let h = i.alloc_string(text);
    i.heap.with_mut::<ClrObject, _>(this, |o| {
        if o.fields.is_empty() {
            o.fields.push(Value::Obj(h));
        } else {
            o.fields[0] = Value::Obj(h);
        }
    });
}

fn sb_append(i: &mut Interpreter, a: &[Value], text: &str) -> ExecResult<Option<Value>> {
    let this = arg_handle(i, a, 0)?;
    crate::strings::builder_append(i, this, text);
    Ok(Some(Value::Obj(this)))
}

fn register_string_builder(interp: &mut Interpreter) {
    const SB: &str = "System.Text.StringBuilder";
    let key = |m: &str| k(format!("{SB}::{m}"));
    interp.register_native(key("Append(long)"), |i, a| {
        let v = arg_i64(i, a, 1)?;
        sb_append(i, a, &v.to_string())
    });
    interp.register_native(key("Append(bool)"), |i, a| {
        let v = arg_bool(i, a, 1)?;
        sb_append(i, a, if v { "True" } else { "False" })
    });
    interp.register_native(key("Append(double)"), |i, a| {
        let v = arg_f64(i, a, 1)?;
        sb_append(i, a, &crate::support::format_double(v))
    });
    interp.register_native(key("Append(float)"), |i, a| {
        let v = arg_f64(i, a, 1)?;
        sb_append(i, a, &crate::support::format_single(v as f32))
    });
    interp.register_native(key("Append(char,int)"), |i, a| {
        let c = char::from_u32(arg_i32(i, a, 1)? as u32).unwrap_or(' ');
        let n = arg_i32(i, a, 2)?.max(0) as usize;
        sb_append(i, a, &c.to_string().repeat(n))
    });
    interp.register_native(key("Append(string,int,int)"), |i, a| {
        let s = units(i, a, 1)?;
        let (at, len) = (arg_i32(i, a, 2)?.max(0) as usize, arg_i32(i, a, 3)?.max(0) as usize);
        if at + len > s.len() {
            return Err(out_range("count"));
        }
        let piece = String::from_utf16_lossy(&s[at..at + len]);
        sb_append(i, a, &piece)
    });
    interp.register_native(key("Append(char[])"), |i, a| {
        let chars: Vec<u16> = { let h__ = arg_handle(i, a, 1)?; array_values(i, h__) }.iter().map(|v| v.as_i32().unwrap_or(0) as u16).collect();
        sb_append(i, a, &String::from_utf16_lossy(&chars))
    });
    for name in ["AppendFormat/2", "AppendFormat/3", "AppendFormat/4"] {
        interp.register_native(key(name), |i, a| {
            let format_text = arg_string(i, a, 1)?;
            let mut values: Vec<Value> = (2..a.len()).map(|n| arg(i, a, n)).collect::<ExecResult<_>>()?;
            if values.len() == 1 {
                if let Value::Obj(h) = values[0] {
                    if i.heap.with::<ClrArray, _>(h, |_| ()).is_some() && i.string_value(h).is_none() {
                        values = array_values(i, h);
                    }
                }
            }
            let text = composite(i, &format_text, &values);
            sb_append(i, a, &text)
        });
    }
    interp.register_native(key("AppendLine(object)"), |i, a| {
        let v = arg(i, a, 1)?;
        let text = display(i, &v) + NEWLINE;
        sb_append(i, a, &text)
    });
    interp.register_native(key("Insert(int,string)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let at = arg_i32(i, a, 1)?.max(0) as usize;
        let add = arg_string_or_empty(i, a, 2)?;
        let mut s: Vec<u16> = sb_get(i, this).encode_utf16().collect();
        let at = at.min(s.len());
        s.splice(at..at, add.encode_utf16());
        sb_set(i, this, &String::from_utf16_lossy(&s));
        Ok(Some(Value::Obj(this)))
    });
    interp.register_native(key("Insert/2"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let at = arg_i32(i, a, 1)?.max(0) as usize;
        let v = arg(i, a, 2)?;
        let add = display(i, &v);
        let mut s: Vec<u16> = sb_get(i, this).encode_utf16().collect();
        let at = at.min(s.len());
        s.splice(at..at, add.encode_utf16());
        sb_set(i, this, &String::from_utf16_lossy(&s));
        Ok(Some(Value::Obj(this)))
    });
    interp.register_native(key("Remove(int,int)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let (at, len) = (arg_i32(i, a, 1)?.max(0) as usize, arg_i32(i, a, 2)?.max(0) as usize);
        let mut s: Vec<u16> = sb_get(i, this).encode_utf16().collect();
        if at + len > s.len() {
            return Err(out_range("length"));
        }
        s.drain(at..at + len);
        sb_set(i, this, &String::from_utf16_lossy(&s));
        Ok(Some(Value::Obj(this)))
    });
    interp.register_native(key("Replace(string,string)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let (from, to) = (arg_string(i, a, 1)?, arg_string_or_empty(i, a, 2)?);
        let s = sb_get(i, this).replace(&from, &to);
        sb_set(i, this, &s);
        Ok(Some(Value::Obj(this)))
    });
    interp.register_native(key("Replace(char,char)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let (from, to) = (arg_i32(i, a, 1)? as u16, arg_i32(i, a, 2)? as u16);
        let s: Vec<u16> = sb_get(i, this).encode_utf16().map(|u| if u == from { to } else { u }).collect();
        sb_set(i, this, &String::from_utf16_lossy(&s));
        Ok(Some(Value::Obj(this)))
    });
    // `sb[i]` reads one unit in place; copying the builder per read made a
    // loop over it quadratic.
    interp.register_native(key("get_Chars(int)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let at = arg_i32(i, a, 1)?;
        let text = i.heap.with::<ClrObject, _>(this, |o| o.fields.first().cloned()).flatten();
        let unit = match text {
            Some(Value::Obj(h)) if !h.is_null() && at >= 0 => i.heap.with::<ClrString, _>(h, |s| s.units.get(at as usize).copied()).flatten(),
            _ => None,
        };
        unit.map(|u| Some(Value::I32(u as i32))).ok_or_else(|| out_range("index"))
    });
    interp.register_native(key("set_Length(int)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let len = arg_i32(i, a, 1)?.max(0) as usize;
        let mut s: Vec<u16> = sb_get(i, this).encode_utf16().collect();
        s.resize(len, 0);
        sb_set(i, this, &String::from_utf16_lossy(&s));
        Ok(None)
    });
    interp.register_native(key("ToString(int,int)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let (at, len) = (arg_i32(i, a, 1)?.max(0) as usize, arg_i32(i, a, 2)?.max(0) as usize);
        let s: Vec<u16> = sb_get(i, this).encode_utf16().collect();
        let end = (at + len).min(s.len());
        let text = String::from_utf16_lossy(&s[at.min(end)..end]);
        Ok(Some(string_value(i, &text)))
    });
}

// ── chars ────────────────────────────────────────────────────────────────────

fn register_chars(interp: &mut Interpreter) {
    let tests: [(&str, fn(char) -> bool); 9] = [
        ("IsLower", |c| c.is_lowercase()),
        ("IsUpper", |c| c.is_uppercase()),
        ("IsLetterOrDigit", |c| c.is_alphanumeric()),
        ("IsPunctuation", |c| c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c)),
        ("IsSymbol", |c| "$+<=>^`|~".contains(c)),
        ("IsControl", |c| c.is_control()),
        ("IsSeparator", |c| c == ' ' || c == '\u{a0}'),
        ("IsAsciiDigit", |c| c.is_ascii_digit()),
        ("IsAsciiLetter", |c| c.is_ascii_alphabetic()),
    ];
    for (name, test) in tests {
        let f: rustclr_core::NativeFn = match name {
            "IsLower" => |i, a| char_test(i, a, |c| c.is_lowercase()),
            "IsUpper" => |i, a| char_test(i, a, |c| c.is_uppercase()),
            "IsLetterOrDigit" => |i, a| char_test(i, a, |c| c.is_alphanumeric()),
            "IsPunctuation" => |i, a| char_test(i, a, |c| c.is_ascii_punctuation() && !"$+<=>^`|~".contains(c)),
            "IsSymbol" => |i, a| char_test(i, a, |c| "$+<=>^`|~".contains(c)),
            "IsControl" => |i, a| char_test(i, a, |c| c.is_control()),
            "IsSeparator" => |i, a| char_test(i, a, |c| c == ' ' || c == '\u{a0}'),
            "IsAsciiDigit" => |i, a| char_test(i, a, |c| c.is_ascii_digit()),
            _ => |i, a| char_test(i, a, |c| c.is_ascii_alphabetic()),
        };
        let _ = test;
        interp.register_native(k(format!("System.Char::{name}(char)")), f);
    }
    interp.register_native("System.Char::IsAsciiHexDigit(char)", |i, a| char_test(i, a, |c| c.is_ascii_hexdigit()));
    interp.register_native("System.Char::IsAsciiHexDigitLower(char)", |i, a| char_test(i, a, |c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    interp.register_native("System.Char::IsAsciiHexDigitUpper(char)", |i, a| char_test(i, a, |c| c.is_ascii_digit() || ('A'..='F').contains(&c)));
    interp.register_native("System.Char::IsAsciiLetterOrDigit(char)", |i, a| char_test(i, a, |c| c.is_ascii_alphanumeric()));
    interp.register_native("System.Span`1::ToString()", crate::spans::span_to_string);
    interp.register_native("System.ReadOnlySpan`1::ToString()", crate::spans::span_to_string);
    interp.register_native("System.Uri::IsHexDigit(char)", |i, a| char_test(i, a, |c| c.is_ascii_hexdigit()));
    interp.register_native("System.Char::GetNumericValue(char)", |i, a| {
        let c = char::from_u32(arg_i32(i, a, 0)? as u32).unwrap_or('\0');
        Ok(Some(Value::F(c.to_digit(10).map(|d| d as f64).unwrap_or(-1.0))))
    });
    interp.register_native("System.Char::IsLetter(string,int)", |i, a| {
        let s = units(i, a, 0)?;
        let at = arg_i32(i, a, 1)?.max(0) as usize;
        Ok(Some(Value::I32(s.get(at).and_then(|u| char::from_u32(*u as u32)).map(|c| c.is_alphabetic()).unwrap_or(false) as i32)))
    });
}

fn char_test(i: &mut Interpreter, a: &[Value], test: fn(char) -> bool) -> ExecResult<Option<Value>> {
    let c = char::from_u32(arg_i32(i, a, 0)? as u32).unwrap_or('\0');
    Ok(Some(Value::I32(test(c) as i32)))
}

// ── enums ────────────────────────────────────────────────────────────────────

/// Names of the framework enums RustBCL defines natively.
fn native_enum_names(type_name: &str) -> Option<&'static [&'static str]> {
    Some(match type_name {
        "System.DayOfWeek" => &format::DAYS,
        "System.StringComparison" => &[
            "CurrentCulture", "CurrentCultureIgnoreCase", "InvariantCulture", "InvariantCultureIgnoreCase",
            "Ordinal", "OrdinalIgnoreCase",
        ],
        "System.MidpointRounding" => &["ToEven", "AwayFromZero", "ToZero", "ToNegativeInfinity", "ToPositiveInfinity"],
        "System.DateTimeKind" => &["Unspecified", "Utc", "Local"],
        _ => return None,
    })
}

/// (name, value) for every member of an enum type, in declaration order.
pub fn enum_members(interp: &Interpreter, ty: TypeId) -> Vec<(String, i64)> {
    let name = interp.loader.registry.ty(ty).full_name();
    if let Some(names) = native_enum_names(&name) {
        return names.iter().enumerate().map(|(n, s)| (s.to_string(), n as i64)).collect();
    }
    let mut out = Vec::new();
    for f in interp.loader.registry.ty(ty).static_fields.clone() {
        let field = interp.loader.registry.field(f);
        if !field.is_literal {
            continue;
        }
        if let Some(v) = field.constant.as_ref().and_then(|c| c.as_i64()) {
            out.push((field.name.clone(), v));
        }
    }
    out
}

/// What `ToString()` on an enum value prints: its name, a `[Flags]`
/// combination joined with `, `, or the number when nothing matches.
pub fn enum_name(interp: &Interpreter, ty: TypeId, value: i64) -> String {
    let members = enum_members(interp, ty);
    if let Some((name, _)) = members.iter().find(|(_, v)| *v == value) {
        return name.clone();
    }
    // A flags combination: every set bit covered by a named member.
    if value > 0 {
        let mut remaining = value;
        let mut names = Vec::new();
        for (name, v) in members.iter().rev() {
            if *v != 0 && remaining & *v == *v {
                names.push(name.clone());
                remaining &= !*v;
            }
        }
        if remaining == 0 && !names.is_empty() {
            names.reverse();
            return names.join(", ");
        }
    }
    value.to_string()
}

fn parse_enum(interp: &Interpreter, ty: TypeId, text: &str, ignore_case: bool) -> Option<i64> {
    let members = enum_members(interp, ty);
    let mut total = 0i64;
    for part in text.split(',') {
        let part = part.trim();
        if let Ok(n) = part.parse::<i64>() {
            total |= n;
            continue;
        }
        let found = members.iter().find(|(name, _)| {
            if ignore_case { name.eq_ignore_ascii_case(part) } else { name == part }
        })?;
        total |= found.1;
    }
    Some(total)
}

fn enum_value(interp: &Interpreter, ty: TypeId, v: i64) -> Value {
    let underlying = interp.loader.registry.ty(ty).underlying;
    match underlying {
        Some(Primitive::Int64) | Some(Primitive::UInt64) => Value::I64(v),
        _ => Value::I32(v as i32),
    }
}

fn register_enums(interp: &mut Interpreter) {
    // Enum.Parse<T>(string), Enum.Parse<T>(string, bool ignoreCase).
    for name in ["Parse(string)", "Parse/2"] {
        interp.register_native(k(format!("System.Enum::{name}")), |i, a| {
            // Parse(Type, string) also has arity 2; it is told apart by its
            // first argument being a type object.
            let first = arg(i, a, 0)?;
            if let Value::Obj(h) = first {
                if let Some(ty) = i.type_from_object(h) {
                    let text = arg_string(i, a, 1)?;
                    let v = parse_enum(i, ty, &text, false).ok_or_else(|| argument(&format!("Requested value '{text}' was not found.")))?;
                    let value = enum_value(i, ty, v);
                    return Ok(Some(Value::Obj(i.box_value(ty, value))));
                }
            }
            let ty = type_arg(i, 0).ok_or_else(|| argument("Enum.Parse needs a type."))?;
            let text = arg_string(i, a, 0)?;
            let ignore = a.len() > 1 && arg_bool(i, a, 1)?;
            let v = parse_enum(i, ty, &text, ignore).ok_or_else(|| argument(&format!("Requested value '{text}' was not found.")))?;
            Ok(Some(enum_value(i, ty, v)))
        });
    }
    interp.register_native("System.Enum::Parse/3", |i, a| {
        let ty = { let h__ = arg_handle(i, a, 0)?; i.type_from_object(h__) }.ok_or_else(|| argument("enumType"))?;
        let text = arg_string(i, a, 1)?;
        let ignore = arg_bool(i, a, 2)?;
        let v = parse_enum(i, ty, &text, ignore).ok_or_else(|| argument(&format!("Requested value '{text}' was not found.")))?;
        let value = enum_value(i, ty, v);
        Ok(Some(Value::Obj(i.box_value(ty, value))))
    });
    // TryParse<T>(string, out T) and TryParse<T>(string, bool, out T).
    for name in ["TryParse/2", "TryParse/3"] {
        interp.register_native(k(format!("System.Enum::{name}")), |i, a| {
            let Some(ty) = type_arg(i, 0) else { return Ok(Some(Value::I32(0))) };
            let text = arg_string_or_empty(i, a, 0)?;
            let ignore = a.len() == 3 && arg_bool(i, a, 1)?;
            let parsed = parse_enum(i, ty, &text, ignore);
            let value = enum_value(i, ty, parsed.unwrap_or(0));
            store_out(i, a, a.len() - 1, value)?;
            Ok(Some(Value::I32(parsed.is_some() as i32)))
        });
    }
    // GetValues<T>() and GetValues(Type); GetNames likewise.
    for name in ["GetValues()", "GetValues/1", "GetNames()", "GetNames/1"] {
        interp.register_native(k(format!("System.Enum::{name}")), |i, a| {
            let ty = if a.is_empty() { type_arg(i, 0) } else { { let h__ = arg_handle(i, a, 0)?; i.type_from_object(h__) } };
            let ty = ty.ok_or_else(|| argument("enumType"))?;
            let names_only = i
                .current_native_method()
                .map(|m| i.loader.registry.method(m).name == "GetNames")
                .unwrap_or(false);
            let members = enum_members(i, ty);
            if names_only {
                let names: Vec<String> = members.into_iter().map(|(n, _)| n).collect();
                return Ok(Some(Value::Obj(string_array(i, &names))));
            }
            let array = i.alloc_array(ty, members.len());
            for (n, (_, v)) in members.iter().enumerate() {
                let value = enum_value(i, ty, *v);
                i.heap.with_mut::<ClrArray, _>(array, |arr| {
                    arr.storage.set(n, &value);
                });
            }
            Ok(Some(Value::Obj(array)))
        });
    }
    interp.register_native("System.Enum::IsDefined/1", |i, a| {
        let ty = type_arg(i, 0).ok_or_else(|| argument("enumType"))?;
        let v = arg_i64(i, a, 0)?;
        Ok(Some(Value::I32(enum_members(i, ty).iter().any(|(_, m)| *m == v) as i32)))
    });
    interp.register_native("System.Enum::IsDefined/2", |i, a| {
        let ty = { let h__ = arg_handle(i, a, 0)?; i.type_from_object(h__) }.ok_or_else(|| argument("enumType"))?;
        let v = { let v__ = arg(i, a, 1)?; unbox_value(i, &v__) };
        let defined = match v {
            Value::Obj(h) => {
                let name = i.string_value(h).unwrap_or_default();
                enum_members(i, ty).iter().any(|(n, _)| *n == name)
            }
            other => {
                let n = other.as_i64().unwrap_or(-1);
                enum_members(i, ty).iter().any(|(_, m)| *m == n)
            }
        };
        Ok(Some(Value::I32(defined as i32)))
    });
    interp.register_native("System.Enum::GetName/1", |i, a| {
        let ty = type_arg(i, 0).ok_or_else(|| argument("enumType"))?;
        let v = arg_i64(i, a, 0)?;
        let name = enum_members(i, ty).into_iter().find(|(_, m)| *m == v).map(|(n, _)| n);
        Ok(Some(match name {
            Some(n) => string_value(i, &n),
            None => Value::Null,
        }))
    });
    // Enums order by their underlying value; null sorts first.
    interp.register_native("System.Enum::CompareTo/1", |i, a| {
        let this = { let v__ = arg(i, a, 0)?; unbox_value(i, &v__) }.as_i64().unwrap_or(0);
        let other = { let v__ = arg(i, a, 1)?; unbox_value(i, &v__) };
        Ok(Some(Value::I32(match other {
            Value::Null => 1,
            v => this.cmp(&v.as_i64().unwrap_or(0)) as i32,
        })))
    });
    interp.register_native("System.Enum::HasFlag/1", |i, a| {
        let this = { let v__ = arg(i, a, 0)?; unbox_value(i, &v__) }.as_i64().unwrap_or(0);
        let flag = { let v__ = arg(i, a, 1)?; unbox_value(i, &v__) }.as_i64().unwrap_or(0);
        Ok(Some(Value::I32((this & flag == flag) as i32)))
    });
}

// ── List<T> ──────────────────────────────────────────────────────────────────

fn list_values(i: &mut Interpreter, this: Handle) -> Vec<Value> {
    let items = list_items(i, this);
    elements(i, items)
}

fn list_replace(i: &mut Interpreter, this: Handle, values: Vec<Value>) {
    let items = crate::collections::ensure_list(i, this);
    with_values(i, items, |v| *v = values);
}

fn predicate(i: &mut Interpreter, delegate: &Value, item: &Value) -> ExecResult<bool> {
    Ok(crate::linq::call(i, delegate, &[item.clone()])?.is_truthy())
}

fn default_of_arg(i: &mut Interpreter, index: usize) -> Value {
    match type_arg(i, index) {
        Some(t) => i.zero_of(t),
        None => Value::Null,
    }
}

fn register_lists(interp: &mut Interpreter) {
    const L: &str = "System.Collections.Generic.List`1";
    let key = |m: &str| k(format!("{L}::{m}"));
    interp.register_native(key("Find/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        for v in list_values(i, this) {
            if predicate(i, &pred, &v)? {
                return Ok(Some(v));
            }
        }
        Ok(Some(default_of_arg(i, 0)))
    });
    interp.register_native(key("FindLast/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        for v in list_values(i, this).into_iter().rev() {
            if predicate(i, &pred, &v)? {
                return Ok(Some(v));
            }
        }
        Ok(Some(default_of_arg(i, 0)))
    });
    interp.register_native(key("FindIndex/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        for (n, v) in list_values(i, this).into_iter().enumerate() {
            if predicate(i, &pred, &v)? {
                return Ok(Some(Value::I32(n as i32)));
            }
        }
        Ok(Some(Value::I32(-1)))
    });
    interp.register_native(key("FindLastIndex/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        let values = list_values(i, this);
        for n in (0..values.len()).rev() {
            if predicate(i, &pred, &values[n])? {
                return Ok(Some(Value::I32(n as i32)));
            }
        }
        Ok(Some(Value::I32(-1)))
    });
    interp.register_native(key("FindAll/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        let mut kept = Vec::new();
        for v in list_values(i, this) {
            if predicate(i, &pred, &v)? {
                kept.push(v);
            }
        }
        Ok(Some(new_list(i, kept)))
    });
    interp.register_native(key("TrueForAll/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        for v in list_values(i, this) {
            if !predicate(i, &pred, &v)? {
                return Ok(Some(Value::I32(0)));
            }
        }
        Ok(Some(Value::I32(1)))
    });
    interp.register_native(key("RemoveAll/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        let values = list_values(i, this);
        let before = values.len();
        let mut kept = Vec::with_capacity(before);
        for v in values {
            if !predicate(i, &pred, &v)? {
                kept.push(v);
            }
        }
        let removed = before - kept.len();
        list_replace(i, this, kept);
        Ok(Some(Value::I32(removed as i32)))
    });
    interp.register_native(key("ForEach/1"), |i, a| {
        let this = receiver(i, a)?;
        let action = arg(i, a, 1)?;
        for v in list_values(i, this) {
            crate::linq::call(i, &action, &[v])?;
        }
        Ok(None)
    });
    interp.register_native(key("ConvertAll/1"), |i, a| {
        let this = receiver(i, a)?;
        let converter = arg(i, a, 1)?;
        let mut out = Vec::new();
        for v in list_values(i, this) {
            out.push(crate::linq::call(i, &converter, &[v])?);
        }
        Ok(Some(new_list(i, out)))
    });
    interp.register_native(key("GetRange(int,int)"), |i, a| {
        let this = receiver(i, a)?;
        let (at, count) = (arg_i32(i, a, 1)?, arg_i32(i, a, 2)?);
        let values = list_values(i, this);
        if at < 0 || count < 0 || (at + count) as usize > values.len() {
            return Err(argument("Offset and length were out of bounds for the array or count is greater than the number of elements from index to the end of the source collection."));
        }
        Ok(Some(new_list(i, values[at as usize..(at + count) as usize].to_vec())))
    });
    interp.register_native(key("RemoveRange(int,int)"), |i, a| {
        let this = receiver(i, a)?;
        let (at, count) = (arg_i32(i, a, 1)?.max(0) as usize, arg_i32(i, a, 2)?.max(0) as usize);
        let mut values = list_values(i, this);
        if at + count > values.len() {
            return Err(argument("Offset and length were out of bounds."));
        }
        values.drain(at..at + count);
        list_replace(i, this, values);
        Ok(None)
    });
    interp.register_native(key("InsertRange/2"), |i, a| {
        let this = receiver(i, a)?;
        let at = arg_i32(i, a, 1)?.max(0) as usize;
        let source = arg(i, a, 2)?;
        let added = sequence_values(i, &source);
        let mut values = list_values(i, this);
        let at = at.min(values.len());
        values.splice(at..at, added);
        list_replace(i, this, values);
        Ok(None)
    });
    interp.register_native(key("LastIndexOf(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let item = arg(i, a, 1)?;
        let values = list_values(i, this);
        Ok(Some(Value::I32(values.iter().rposition(|v| values_equal(i, v, &item)).map(|p| p as i32).unwrap_or(-1))))
    });
    interp.register_native(key("Exists/1"), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        for v in list_values(i, this) {
            if predicate(i, &pred, &v)? {
                return Ok(Some(Value::I32(1)));
            }
        }
        Ok(Some(Value::I32(0)))
    });
    interp.register_native(key("AsReadOnly()"), |i, a| Ok(Some(Value::Obj(receiver(i, a)?))));
    interp.register_native(key("get_Capacity()"), |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(Value::I32(list_values(i, this).len() as i32)))
    });
    interp.register_native(key("set_Capacity(int)"), |_, _| Ok(None));
    interp.register_native(key("EnsureCapacity(int)"), |i, a| Ok(Some(Value::I32(arg_i32(i, a, 1)?))));
    interp.register_native(key("TrimExcess()"), |_, _| Ok(None));
    let _ = (LIST_ITEMS, field, set_field);
}

// ── arrays ───────────────────────────────────────────────────────────────────

fn array_handle(i: &mut Interpreter, a: &[Value], index: usize) -> ExecResult<Handle> {
    let h = arg_handle(i, a, index)?;
    if h.is_null() {
        return Err(ExecutionError::exception(ClrExceptionKind::ArgumentNull, "Value cannot be null. (Parameter 'array')"));
    }
    Ok(h)
}

fn is_array(i: &Interpreter, v: &Value) -> bool {
    v.as_handle().filter(|h| !h.is_null()).is_some_and(|h| i.heap.with::<ClrArray, _>(h, |_| ()).is_some())
}

/// Sorts `keys[start..start+len]` (stable), moving `items` the same way when given:
/// `Array.Sort(keys, items)`, `Array.Sort(keys, items, comparer)` and
/// `Array.Sort(array, index, length)`.
fn sort_keys_items(
    i: &mut Interpreter,
    keys: Handle,
    items: Option<Handle>,
    comparer: Option<Value>,
    start: usize,
    len: usize,
) -> ExecResult<Option<Value>> {
    let key_values = elements(i, keys);
    let item_values = items.map(|h| elements(i, h));
    if let Some(iv) = &item_values {
        if iv.len() < start + len {
            return Err(ExecutionError::exception(
                ClrExceptionKind::Argument,
                "The keys and items arrays have different lengths.",
            ));
        }
    }
    let mut compare: Box<dyn FnMut(&mut Interpreter, &Value, &Value) -> ExecResult<core::cmp::Ordering>> = match comparer {
        Some(c) => crate::collections::comparison_from(i, c)?,
        None => Box::new(|i: &mut Interpreter, x: &Value, y: &Value| {
            Ok(match crate::collections::compare_values(i, x, y)? {
                n if n < 0 => core::cmp::Ordering::Less,
                0 => core::cmp::Ordering::Equal,
                _ => core::cmp::Ordering::Greater,
            })
        }),
    };
    // Sort positions by their keys, then lay both arrays out in that order.
    let order: Vec<Value> = (start..start + len).map(|p| Value::I32(p as i32)).collect();
    let sorted = crate::collections::merge_sort_by(i, order, &mut |i, x, y| {
        let (px, py) = (x.as_i32().unwrap_or(0) as usize, y.as_i32().unwrap_or(0) as usize);
        compare(i, &key_values[px], &key_values[py])
    })?;
    let positions: Vec<usize> = sorted.iter().map(|v| v.as_i32().unwrap_or(0) as usize).collect();
    let mut new_keys = key_values.clone();
    for (n, p) in positions.iter().enumerate() {
        new_keys[start + n] = key_values[*p].clone();
    }
    write_array(i, keys, &new_keys);
    if let (Some(h), Some(iv)) = (items, item_values) {
        let mut new_items = iv.clone();
        for (n, p) in positions.iter().enumerate() {
            new_items[start + n] = iv[*p].clone();
        }
        write_array(i, h, &new_items);
    }
    Ok(None)
}

fn write_array(i: &mut Interpreter, array: Handle, values: &[Value]) {
    i.heap.with_mut::<ClrArray, _>(array, |arr| {
        for (n, v) in values.iter().enumerate() {
            arr.storage.set(n, v);
        }
    });
}

/// The type a `System.Type` argument describes.
fn type_arg_of(i: &mut Interpreter, a: &[Value], index: usize) -> ExecResult<TypeId> {
    let h = arg_handle(i, a, index)?;
    i.type_from_object(h)
        .ok_or_else(|| ExecutionError::exception(ClrExceptionKind::ArgumentNull, "Value cannot be null. (Parameter 'type')"))
}

/// The reflection surface serializers build values through: arrays and enums
/// made from a `System.Type`, and `Nullable.GetUnderlyingType`.
fn register_reflective_values(interp: &mut Interpreter) {
    interp.register_native("System.Array::CreateInstance/2", |i, a| {
        let element = type_arg_of(i, a, 0)?;
        let length = arg_i32(i, a, 1)?;
        if length < 0 {
            return Err(out_range("Non-negative number required. (Parameter 'length')"));
        }
        let array = i.alloc_array(element, length as usize);
        Ok(Some(Value::Obj(array)))
    });
    interp.register_native("System.Array::SetValue(object,int)", |i, a| {
        let array = array_handle(i, a, 0)?;
        let value = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        let index = arg_i32(i, a, 2)?;
        let len = i.heap.with::<ClrArray, _>(array, |x| x.len()).unwrap_or(0);
        if index < 0 || index as usize >= len {
            return Err(out_range("Index was outside the bounds of the array."));
        }
        i.heap.with_mut::<ClrArray, _>(array, |x| x.storage.set(index as usize, &value));
        Ok(None)
    });
    interp.register_native("System.Array::GetValue(int)", |i, a| {
        let array = array_handle(i, a, 0)?;
        let index = arg_i32(i, a, 1)?;
        let found = i.heap.with::<ClrArray, _>(array, |x| (x.storage.get(index.max(0) as usize), x.element_type));
        let Some((Some(value), element)) = found.filter(|_| index >= 0) else {
            return Err(out_range("Index was outside the bounds of the array."));
        };
        Ok(Some(crate::collections::box_for_object(i, value, Some(element))))
    });
    interp.register_native("System.Enum::ToObject/2", |i, a| {
        let enum_type = type_arg_of(i, a, 0)?;
        let raw = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        let number = raw.as_i64().or_else(|| raw.as_i32().map(i64::from)).unwrap_or(0);
        let boxed = i.box_value(enum_type, Value::I32(number as i32));
        Ok(Some(Value::Obj(boxed)))
    });
    interp.register_native("System.Nullable::GetUnderlyingType/1", |i, a| {
        let ty = type_arg_of(i, a, 0)?;
        let info = i.loader.registry.ty(ty);
        let definition = info.generic_definition.map(|d| i.loader.registry.ty(d).full_name());
        match (definition.as_deref(), info.generic_args.first().copied()) {
            (Some("System.Nullable`1"), Some(inner)) => {
                let handle = i.type_object(inner);
                Ok(Some(Value::Obj(handle)))
            }
            _ => Ok(Some(Value::Null)),
        }
    });
}

fn register_arrays(interp: &mut Interpreter) {
    register_reflective_values(interp);
    // Array.Sort(T[]), Array.Sort(T[], Comparison<T>), Array.Sort(T[], IComparer<T>).
    for name in ["Sort/1", "Sort(int[])", "Sort(double[])", "Sort(string[])"] {
        interp.register_native(k(format!("System.Array::{name}")), |i, a| {
            let array = array_handle(i, a, 0)?;
            let values = elements(i, array);
            let sorted = crate::collections::merge_sort_by(i, values, &mut |i, x, y| {
                Ok(match crate::collections::compare_values(i, x, y)? {
                    n if n < 0 => core::cmp::Ordering::Less,
                    0 => core::cmp::Ordering::Equal,
                    _ => core::cmp::Ordering::Greater,
                })
            })?;
            write_array(i, array, &sorted);
            Ok(None)
        });
    }
    // Natives bind by arity, so Sort/2 is both Sort(T[], comparer) and
    // Sort(TKey[] keys, TValue[] items); the second argument tells them apart.
    interp.register_native("System.Array::Sort/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let second = arg(i, a, 1)?;
        if is_array(i, &second) {
            let items = second.as_handle().expect("checked");
            let n = elements(i, array).len();
            return sort_keys_items(i, array, Some(items), None, 0, n);
        }
        let values = elements(i, array);
        let mut compare = crate::collections::comparison_from(i, second)?;
        let sorted = crate::collections::merge_sort_by(i, values, &mut *compare)?;
        write_array(i, array, &sorted);
        Ok(None)
    });
    // Sort(T[] array, int index, int length) and Sort(TKey[] keys, TValue[] items, IComparer<TKey>).
    interp.register_native("System.Array::Sort/3", |i, a| {
        let array = array_handle(i, a, 0)?;
        let second = arg(i, a, 1)?;
        if let Some(index) = second.as_i32() {
            let length = arg(i, a, 2)?.as_i32().unwrap_or(0);
            let total = elements(i, array).len() as i32;
            if index < 0 || length < 0 || index + length > total {
                return Err(ExecutionError::exception(
                    ClrExceptionKind::Argument,
                    "Offset and length were out of bounds for the array or count is greater than the number of elements from index to the end of the source collection.",
                ));
            }
            return sort_keys_items(i, array, None, None, index as usize, length as usize);
        }
        let items = second.as_handle().filter(|h| !h.is_null());
        let comparer = arg(i, a, 2)?;
        let comparer = comparer.as_handle().filter(|h| !h.is_null()).map(|_| comparer.clone());
        let n = elements(i, array).len();
        sort_keys_items(i, array, items, comparer, 0, n)
    });
    for name in ["Reverse/1", "Reverse(int[])"] {
        interp.register_native(k(format!("System.Array::{name}")), |i, a| {
            let array = array_handle(i, a, 0)?;
            let mut values = elements(i, array);
            values.reverse();
            write_array(i, array, &values);
            Ok(None)
        });
    }
    interp.register_native("System.Array::IndexOf/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let item = arg(i, a, 1)?;
        let values = elements(i, array);
        Ok(Some(Value::I32(values.iter().position(|v| values_equal(i, v, &item)).map(|p| p as i32).unwrap_or(-1))))
    });
    interp.register_native("System.Array::LastIndexOf/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let item = arg(i, a, 1)?;
        let values = elements(i, array);
        Ok(Some(Value::I32(values.iter().rposition(|v| values_equal(i, v, &item)).map(|p| p as i32).unwrap_or(-1))))
    });
    interp.register_native("System.Array::Exists/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let pred = arg(i, a, 1)?;
        for v in elements(i, array) {
            if predicate(i, &pred, &v)? {
                return Ok(Some(Value::I32(1)));
            }
        }
        Ok(Some(Value::I32(0)))
    });
    interp.register_native("System.Array::Find/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let pred = arg(i, a, 1)?;
        for v in elements(i, array) {
            if predicate(i, &pred, &v)? {
                return Ok(Some(v));
            }
        }
        Ok(Some(default_of_arg(i, 0)))
    });
    interp.register_native("System.Array::FindIndex/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let pred = arg(i, a, 1)?;
        for (n, v) in elements(i, array).into_iter().enumerate() {
            if predicate(i, &pred, &v)? {
                return Ok(Some(Value::I32(n as i32)));
            }
        }
        Ok(Some(Value::I32(-1)))
    });
    interp.register_native("System.Array::FindAll/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let pred = arg(i, a, 1)?;
        let element = i.heap.with::<ClrArray, _>(array, |x| x.element_type).unwrap_or(i.loader.core().object);
        let mut kept = Vec::new();
        for v in elements(i, array) {
            if predicate(i, &pred, &v)? {
                kept.push(v);
            }
        }
        let out = i.alloc_array(element, kept.len());
        write_array(i, out, &kept);
        Ok(Some(Value::Obj(out)))
    });
    interp.register_native("System.Array::Fill/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let item = arg(i, a, 1)?;
        let len = i.heap.with::<ClrArray, _>(array, |x| x.len()).unwrap_or(0);
        write_array(i, array, &vec![item; len]);
        Ok(None)
    });
    interp.register_native("System.Array::Empty()", |i, _| {
        let element = type_arg(i, 0).unwrap_or(i.loader.core().object);
        Ok(Some(Value::Obj(i.alloc_array(element, 0))))
    });
    // Copy(source, destination, length) and Copy(source, si, destination, di, length).
    interp.register_native("System.Array::Copy/3", |i, a| {
        let (src, dst) = (array_handle(i, a, 0)?, array_handle(i, a, 1)?);
        let len = arg_i32(i, a, 2)?.max(0) as usize;
        let values = elements(i, src);
        if len > values.len() {
            return Err(argument("Source array was not long enough."));
        }
        write_array(i, dst, &values[..len]);
        Ok(None)
    });
    interp.register_native("System.Array::Copy/5", |i, a| {
        let (src, dst) = (array_handle(i, a, 0)?, array_handle(i, a, 2)?);
        let (si, di, len) = (arg_i32(i, a, 1)?.max(0) as usize, arg_i32(i, a, 3)?.max(0) as usize, arg_i32(i, a, 4)?.max(0) as usize);
        let values = elements(i, src);
        if si + len > values.len() {
            return Err(argument("Source array was not long enough."));
        }
        let piece: Vec<Value> = values[si..si + len].to_vec();
        i.heap.with_mut::<ClrArray, _>(dst, |arr| {
            for (n, v) in piece.iter().enumerate() {
                arr.storage.set(di + n, v);
            }
        });
        Ok(None)
    });
    interp.register_native("System.Array::Clear/1", |i, a| {
        let array = array_handle(i, a, 0)?;
        clear_array(i, array, 0, usize::MAX);
        Ok(None)
    });
    interp.register_native("System.Array::Clear/3", |i, a| {
        let array = array_handle(i, a, 0)?;
        let (at, len) = (arg_i32(i, a, 1)?.max(0) as usize, arg_i32(i, a, 2)?.max(0) as usize);
        clear_array(i, array, at, len);
        Ok(None)
    });
    // Resize<T>(ref T[] array, int newSize).
    interp.register_native("System.Array::Resize/2", |i, a| {
        let old = arg_handle(i, a, 0)?;
        let size = arg_i32(i, a, 1)?.max(0) as usize;
        let element = if old.is_null() {
            type_arg(i, 0).unwrap_or(i.loader.core().object)
        } else {
            i.heap.with::<ClrArray, _>(old, |x| x.element_type).unwrap_or(i.loader.core().object)
        };
        let values = if old.is_null() { Vec::new() } else { elements(i, old) };
        let fresh = i.alloc_array(element, size);
        let n = values.len().min(size);
        write_array(i, fresh, &values[..n]);
        store_out(i, a, 0, Value::Obj(fresh))?;
        Ok(None)
    });
    interp.register_native("System.Array::ConvertAll/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let converter = arg(i, a, 1)?;
        let element = type_arg(i, 1).unwrap_or(i.loader.core().object);
        let values = elements(i, array);
        let mut out = Vec::with_capacity(values.len());
        for v in values {
            out.push(crate::linq::call(i, &converter, &[v])?);
        }
        let fresh = i.alloc_array(element, out.len());
        write_array(i, fresh, &out);
        Ok(Some(Value::Obj(fresh)))
    });
    interp.register_native("System.Array::TrueForAll/2", |i, a| {
        let array = array_handle(i, a, 0)?;
        let pred = arg(i, a, 1)?;
        for v in elements(i, array) {
            if !predicate(i, &pred, &v)? {
                return Ok(Some(Value::I32(0)));
            }
        }
        Ok(Some(Value::I32(1)))
    });
}

fn clear_array(i: &mut Interpreter, array: Handle, at: usize, len: usize) {
    let element = i.heap.with::<ClrArray, _>(array, |x| x.element_type);
    let zero = element.map(|t| i.zero_of(t)).unwrap_or(Value::Null);
    i.heap.with_mut::<ClrArray, _>(array, |arr| {
        let end = at.saturating_add(len).min(arr.len());
        for n in at..end {
            arr.storage.set(n, &zero);
        }
    });
}

// ── HashSet and Dictionary ───────────────────────────────────────────────────

fn set_items(i: &mut Interpreter, this: Handle) -> Vec<Value> {
    elements(i, field_handle(i, this, HASHSET_ITEMS))
}

fn set_rebuild(i: &mut Interpreter, this: Handle, items: Vec<Value>) {
    crate::collections::reset_table(i, this, HASHSET_BUCKETS, HASHSET_NEXT, &[HASHSET_ITEMS]);
    for item in items {
        if crate::collections::set_find(i, this, &item).is_none() {
            crate::collections::table_insert(i, this, HASHSET_BUCKETS, HASHSET_NEXT, HASHSET_ITEMS, item);
        }
    }
}

fn register_sets_and_dictionaries(interp: &mut Interpreter) {
    const HS: &str = "System.Collections.Generic.HashSet`1";
    interp.register_native(k(format!("{HS}::UnionWith/1")), |i, a| {
        let this = receiver(i, a)?;
        let other = arg(i, a, 1)?;
        let mut items = set_items(i, this);
        items.extend(sequence_values(i, &other));
        set_rebuild(i, this, items);
        Ok(None)
    });
    interp.register_native(k(format!("{HS}::IntersectWith/1")), |i, a| {
        let this = receiver(i, a)?;
        let other = arg(i, a, 1)?;
        let others = sequence_values(i, &other);
        let items: Vec<Value> = set_items(i, this).into_iter().filter(|v| others.iter().any(|o| values_equal(i, v, o))).collect();
        set_rebuild(i, this, items);
        Ok(None)
    });
    interp.register_native(k(format!("{HS}::ExceptWith/1")), |i, a| {
        let this = receiver(i, a)?;
        let other = arg(i, a, 1)?;
        let others = sequence_values(i, &other);
        let items: Vec<Value> = set_items(i, this).into_iter().filter(|v| !others.iter().any(|o| values_equal(i, v, o))).collect();
        set_rebuild(i, this, items);
        Ok(None)
    });
    interp.register_native(k(format!("{HS}::RemoveWhere/1")), |i, a| {
        let this = receiver(i, a)?;
        let pred = arg(i, a, 1)?;
        let items = set_items(i, this);
        let before = items.len();
        let mut kept = Vec::new();
        for v in items {
            if !predicate(i, &pred, &v)? {
                kept.push(v);
            }
        }
        let removed = before - kept.len();
        set_rebuild(i, this, kept);
        Ok(Some(Value::I32(removed as i32)))
    });
    for (name, op) in [("IsSubsetOf/1", 0), ("IsSupersetOf/1", 1), ("Overlaps/1", 2), ("SetEquals/1", 3)] {
        let f: rustclr_core::NativeFn = match op {
            0 => |i, a| set_relation(i, a, 0),
            1 => |i, a| set_relation(i, a, 1),
            2 => |i, a| set_relation(i, a, 2),
            _ => |i, a| set_relation(i, a, 3),
        };
        interp.register_native(k(format!("{HS}::{name}")), f);
    }

    const D: &str = "System.Collections.Generic.Dictionary`2";
    interp.register_native(k(format!("{D}::TryAdd/2")), |i, a| {
        let this = receiver(i, a)?;
        let key = arg(i, a, 1)?;
        let value = arg(i, a, 2)?;
        if crate::collections::dict_contains(i, this, &key) {
            return Ok(Some(Value::I32(0)));
        }
        crate::collections::dictionary_set(i, this, key, value);
        Ok(Some(Value::I32(1)))
    });
    // CollectionExtensions.GetValueOrDefault(dictionary, key[, default]).
    for name in ["GetValueOrDefault/2", "GetValueOrDefault/3"] {
        interp.register_native(k(format!("System.Collections.Generic.CollectionExtensions::{name}")), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let key = arg(i, a, 1)?;
            if let Some(v) = crate::collections::dict_get(i, this, &key) {
                return Ok(Some(v));
            }
            if a.len() == 3 {
                return Ok(Some(arg(i, a, 2)?));
            }
            Ok(Some(default_of_arg(i, 1)))
        });
    }
    // Dictionary(IEqualityComparer) and Dictionary(int, IEqualityComparer):
    // the comparer is accepted; keys compare by value. A case-insensitive
    // StringComparer is honoured by lower-casing string keys at the door,
    // which is what `StringComparer` below records.
    let _ = value_hash;
}

fn set_relation(i: &mut Interpreter, a: &[Value], op: u8) -> ExecResult<Option<Value>> {
    let this = receiver(i, a)?;
    let other = arg(i, a, 1)?;
    let mine = set_items(i, this);
    let others = sequence_values(i, &other);
    let contains = |i: &Interpreter, set: &[Value], v: &Value| set.iter().any(|x| values_equal(i, x, v));
    let result = match op {
        0 => mine.iter().all(|v| contains(i, &others, v)),
        1 => others.iter().all(|v| contains(i, &mine, v)),
        2 => mine.iter().any(|v| contains(i, &others, v)),
        _ => mine.iter().all(|v| contains(i, &others, v)) && others.iter().all(|v| contains(i, &mine, v)),
    };
    Ok(Some(Value::I32(result as i32)))
}

// ── LINQ ─────────────────────────────────────────────────────────────────────

fn register_linq(interp: &mut Interpreter) {
    const E: &str = "System.Linq.Enumerable";
    let key = |m: &str| k(format!("{E}::{m}"));

    for (name, take_max) in [("MaxBy/2", true), ("MinBy/2", false)] {
        let f: rustclr_core::NativeFn = if take_max { |i, a| extreme_by(i, a, true) } else { |i, a| extreme_by(i, a, false) };
        interp.register_native(key(name), f);
    }
    interp.register_native(key("Chunk/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let size = arg_i32(i, a, 1)?;
        if size < 1 {
            return Err(out_range("size"));
        }
        let element = type_arg(i, 0).unwrap_or(i.loader.core().object);
        let values = sequence_values(i, &source);
        let mut chunks = Vec::new();
        for piece in values.chunks(size as usize) {
            let array = i.alloc_array(element, piece.len());
            write_array(i, array, piece);
            chunks.push(Value::Obj(array));
        }
        Ok(Some(new_list(i, chunks)))
    });
    for name in ["DistinctBy/2", "DistinctBy/3"] {
        interp.register_native(key(name), |i, a| {
            let source = arg(i, a, 0)?;
            let selector = arg(i, a, 1)?;
            let mut seen: Vec<Value> = Vec::new();
            let mut out = Vec::new();
            for v in sequence_values(i, &source) {
                let key = crate::linq::call(i, &selector, &[v.clone()])?;
                if !seen.iter().any(|s| values_equal(i, s, &key)) {
                    seen.push(key);
                    out.push(v);
                }
            }
            Ok(Some(new_list(i, out)))
        });
    }
    // OfType<T> filters by type; Cast<T> passes everything through.
    interp.register_native(key("OfType/1"), |i, a| {
        let source = arg(i, a, 0)?;
        let Some(wanted) = type_arg(i, 0) else { return Ok(Some(new_list(i, Vec::new()))) };
        let mut out = Vec::new();
        for v in sequence_values(i, &source) {
            if let Value::Obj(h) = v {
                if h.is_null() {
                    continue;
                }
                if let Some(actual) = i.type_of(h) {
                    if i.loader.registry.is_assignable_to(actual, wanted) {
                        // A boxed value type comes out unboxed, as a T.
                        let inner = i.heap.with::<ClrBox, _>(h, |b| b.value.clone());
                        out.push(inner.unwrap_or(Value::Obj(h)));
                    }
                }
            }
        }
        Ok(Some(new_list(i, out)))
    });
    interp.register_native(key("Cast/1"), |i, a| {
        let source = arg(i, a, 0)?;
        let values: Vec<Value> = sequence_values(i, &source).iter().map(|v| unbox_value(i, v)).collect();
        Ok(Some(new_list(i, values)))
    });
    for name in ["SequenceEqual/2", "SequenceEqual/3"] {
        interp.register_native(key(name), |i, a| {
            let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
            let (xs, ys) = (sequence_values(i, &x), sequence_values(i, &y));
            Ok(Some(Value::I32((xs.len() == ys.len() && xs.iter().zip(&ys).all(|(p, q)| values_equal(i, p, q))) as i32)))
        });
    }
    interp.register_native(key("Join/5"), |i, a| {
        let (outer, inner) = (arg(i, a, 0)?, arg(i, a, 1)?);
        let (outer_key, inner_key, result) = (arg(i, a, 2)?, arg(i, a, 3)?, arg(i, a, 4)?);
        let inner_values = sequence_values(i, &inner);
        let mut inner_keys = Vec::with_capacity(inner_values.len());
        for v in &inner_values {
            inner_keys.push(crate::linq::call(i, &inner_key, &[v.clone()])?);
        }
        let mut out = Vec::new();
        for o in sequence_values(i, &outer) {
            let ok = crate::linq::call(i, &outer_key, &[o.clone()])?;
            for (n, ik) in inner_keys.iter().enumerate() {
                if values_equal(i, &ok, ik) {
                    out.push(crate::linq::call(i, &result, &[o.clone(), inner_values[n].clone()])?);
                }
            }
        }
        Ok(Some(new_list(i, out)))
    });
    interp.register_native(key("GroupJoin/5"), |i, a| {
        let (outer, inner) = (arg(i, a, 0)?, arg(i, a, 1)?);
        let (outer_key, inner_key, result) = (arg(i, a, 2)?, arg(i, a, 3)?, arg(i, a, 4)?);
        let inner_values = sequence_values(i, &inner);
        let mut inner_keys = Vec::with_capacity(inner_values.len());
        for v in &inner_values {
            inner_keys.push(crate::linq::call(i, &inner_key, &[v.clone()])?);
        }
        let mut out = Vec::new();
        for o in sequence_values(i, &outer) {
            let ok = crate::linq::call(i, &outer_key, &[o.clone()])?;
            let matches: Vec<Value> = inner_keys
                .iter()
                .enumerate()
                .filter(|(_, ik)| values_equal(i, &ok, ik))
                .map(|(n, _)| inner_values[n].clone())
                .collect();
            let group = new_list(i, matches);
            out.push(crate::linq::call(i, &result, &[o.clone(), group])?);
        }
        Ok(Some(new_list(i, out)))
    });
    // ToLookup: a Dictionary<TKey, List<TElement>>, which answers the lookup's
    // indexer, Count and enumeration. A missing key yields an empty sequence.
    for name in ["ToLookup/2", "ToLookup/3"] {
        interp.register_native(key(name), |i, a| {
            let source = arg(i, a, 0)?;
            let key_selector = arg(i, a, 1)?;
            let element_selector = if a.len() == 3 { Some(arg(i, a, 2)?) } else { None };
            let Some(dict_type) = crate::collections::named_type(i, "Dictionary`2") else {
                return Ok(Some(Value::Null));
            };
            let dict = i.alloc_object(dict_type);
            crate::collections::reset_dictionary(i, dict);
            for v in sequence_values(i, &source) {
                let key = crate::linq::call(i, &key_selector, &[v.clone()])?;
                let element = match &element_selector {
                    Some(sel) => crate::linq::call(i, sel, &[v.clone()])?,
                    None => v,
                };
                let bucket = match crate::collections::dict_get(i, dict, &key) {
                    Some(Value::Obj(list)) => list,
                    _ => {
                        let list = new_list(i, Vec::new());
                        crate::collections::dictionary_set(i, dict, key.clone(), list.clone());
                        list.as_handle().unwrap_or(Handle::NULL)
                    }
                };
                let items = crate::collections::ensure_list(i, bucket);
                crate::collections::push_value(i, items, element);
            }
            Ok(Some(Value::Obj(dict)))
        });
    }
    // *OrDefault for value types: the default of T, not null.
    for (name, last) in [("FirstOrDefault/1", false), ("LastOrDefault/1", true)] {
        let f: rustclr_core::NativeFn = if last { |i, a| or_default(i, a, true, false) } else { |i, a| or_default(i, a, false, false) };
        interp.register_native(key(name), f);
    }
    for (name, last) in [("FirstOrDefault/2", false), ("LastOrDefault/2", true)] {
        let f: rustclr_core::NativeFn = if last { |i, a| or_default(i, a, true, true) } else { |i, a| or_default(i, a, false, true) };
        interp.register_native(key(name), f);
    }
    for name in ["SingleOrDefault/1", "SingleOrDefault/2"] {
        interp.register_native(key(name), |i, a| {
            let source = arg(i, a, 0)?;
            let values = sequence_values(i, &source);
            let mut found = Vec::new();
            for v in values {
                if a.len() == 2 {
                    let pred = arg(i, a, 1)?;
                    if !predicate(i, &pred, &v)? {
                        continue;
                    }
                }
                found.push(v);
            }
            match found.len() {
                0 => Ok(Some(default_of_arg(i, 0))),
                1 => Ok(Some(found.remove(0))),
                _ => Err(crate::collections::invalid_operation("Sequence contains more than one matching element")),
            }
        });
    }
    interp.register_native(key("ElementAtOrDefault/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let at = arg_i32(i, a, 1)?;
        let values = sequence_values(i, &source);
        Ok(Some(if at >= 0 && (at as usize) < values.len() { values[at as usize].clone() } else { default_of_arg(i, 0) }))
    });
    interp.register_native(key("DefaultIfEmpty/1"), |i, a| {
        let source = arg(i, a, 0)?;
        let values = sequence_values(i, &source);
        let values = if values.is_empty() { vec![default_of_arg(i, 0)] } else { values };
        Ok(Some(new_list(i, values)))
    });
    interp.register_native(key("DefaultIfEmpty/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let values = sequence_values(i, &source);
        let values = if values.is_empty() { vec![arg(i, a, 1)?] } else { values };
        Ok(Some(new_list(i, values)))
    });
    interp.register_native(key("Append/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let mut values = sequence_values(i, &source);
        values.push(arg(i, a, 1)?);
        Ok(Some(new_list(i, values)))
    });
    interp.register_native(key("Prepend/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let mut values = sequence_values(i, &source);
        values.insert(0, arg(i, a, 1)?);
        Ok(Some(new_list(i, values)))
    });
    interp.register_native(key("TakeLast/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let n = arg_i32(i, a, 1)?.max(0) as usize;
        let values = sequence_values(i, &source);
        let start = values.len().saturating_sub(n);
        Ok(Some(new_list(i, values[start..].to_vec())))
    });
    interp.register_native(key("SkipLast/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let n = arg_i32(i, a, 1)?.max(0) as usize;
        let values = sequence_values(i, &source);
        let end = values.len().saturating_sub(n);
        Ok(Some(new_list(i, values[..end].to_vec())))
    });
    // OrderBy/ThenBy with an IComparer<TKey>, typically a StringComparer.
    for (name, descending) in [("OrderBy/3", false), ("OrderByDescending/3", true)] {
        let f: rustclr_core::NativeFn = if descending { |i, a| order_with_comparer(i, a, true) } else { |i, a| order_with_comparer(i, a, false) };
        interp.register_native(key(name), f);
    }
    interp.register_native(key("CountBy/2"), |i, a| {
        let source = arg(i, a, 0)?;
        let selector = arg(i, a, 1)?;
        let mut keys: Vec<Value> = Vec::new();
        let mut counts: Vec<i32> = Vec::new();
        for v in sequence_values(i, &source) {
            let key = crate::linq::call(i, &selector, &[v])?;
            match keys.iter().position(|x| values_equal(i, x, &key)) {
                Some(n) => counts[n] += 1,
                None => {
                    keys.push(key);
                    counts.push(1);
                }
            }
        }
        let pairs: Vec<Value> = keys
            .into_iter()
            .zip(counts)
            .map(|(k2, c)| crate::collections::new_key_value_pair(i, k2, Value::I32(c)))
            .collect();
        Ok(Some(new_list(i, pairs)))
    });
}

fn or_default(i: &mut Interpreter, a: &[Value], last: bool, with_predicate: bool) -> ExecResult<Option<Value>> {
    let source = arg(i, a, 0)?;
    let values = sequence_values(i, &source);
    let pred = if with_predicate { Some(arg(i, a, 1)?) } else { None };
    let ordered: Vec<Value> = if last { values.into_iter().rev().collect() } else { values };
    for v in ordered {
        match &pred {
            Some(p) => {
                if predicate(i, p, &v)? {
                    return Ok(Some(v));
                }
            }
            None => return Ok(Some(v)),
        }
    }
    Ok(Some(default_of_arg(i, 0)))
}

fn extreme_by(i: &mut Interpreter, a: &[Value], max: bool) -> ExecResult<Option<Value>> {
    let source = arg(i, a, 0)?;
    let selector = arg(i, a, 1)?;
    let mut best: Option<(Value, Value)> = None;
    for v in sequence_values(i, &source) {
        let key = crate::linq::call(i, &selector, &[v.clone()])?;
        let better = match &best {
            None => true,
            Some((_, current)) => {
                let c = crate::collections::compare_values(i, &key, current)?;
                if max { c > 0 } else { c < 0 }
            }
        };
        if better {
            best = Some((v, key));
        }
    }
    Ok(Some(best.map(|(v, _)| v).unwrap_or_else(|| default_of_arg(i, 0))))
}

fn order_with_comparer(i: &mut Interpreter, a: &[Value], descending: bool) -> ExecResult<Option<Value>> {
    let source = arg(i, a, 0)?;
    let selector = arg(i, a, 1)?;
    let comparer = arg(i, a, 2)?;
    let values = sequence_values(i, &source);
    let mut keyed = Vec::with_capacity(values.len());
    for v in values {
        let key = crate::linq::call(i, &selector, &[v.clone()])?;
        keyed.push((key, v));
    }
    let ignore_case = string_comparer_ignores_case(i, &comparer);
    let pairs: Vec<Value> = keyed.into_iter().map(|(k2, v)| crate::collections::new_key_value_pair(i, k2, v)).collect();
    let mut custom = match ignore_case {
        Some(_) => None,
        None => Some(crate::collections::comparison_from(i, comparer.clone())?),
    };
    let sorted = crate::collections::merge_sort_by(i, pairs, &mut |i, x, y| {
        let kx = crate::collections::field(i, x.as_handle().unwrap_or(Handle::NULL), 0);
        let ky = crate::collections::field(i, y.as_handle().unwrap_or(Handle::NULL), 0);
        let ord = match (&mut custom, ignore_case) {
            (Some(cmp), _) => cmp(i, &kx, &ky)?,
            (None, Some(ignore)) => {
                let ux = string_units(i, &kx);
                let uy = string_units(i, &ky);
                match compare_units(&ux, &uy, ignore) {
                    n if n < 0 => core::cmp::Ordering::Less,
                    0 => core::cmp::Ordering::Equal,
                    _ => core::cmp::Ordering::Greater,
                }
            }
            _ => core::cmp::Ordering::Equal,
        };
        Ok(if descending { ord.reverse() } else { ord })
    })?;
    let out: Vec<Value> = sorted.iter().map(|p| crate::collections::field(i, p.as_handle().unwrap_or(Handle::NULL), 1)).collect();
    Ok(Some(new_list(i, out)))
}

fn string_units(i: &Interpreter, v: &Value) -> Vec<u16> {
    match v {
        Value::Obj(h) => i.heap.with::<ClrString, _>(*h, |s| s.units.clone()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ── delegates and events ─────────────────────────────────────────────────────

fn delegate_targets(i: &Interpreter, v: &Value) -> Option<(TypeId, Vec<rustclr_core::DelegateTarget>)> {
    let h = v.as_handle().filter(|h| !h.is_null())?;
    i.heap.with::<ClrDelegate, _>(h, |d| (d.type_id, d.targets.clone()))
}

fn register_delegates(interp: &mut Interpreter) {
    // `a += b` on a delegate, and every `event` accessor.
    interp.register_native("System.Delegate::Combine/2", |i, a| {
        let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
        let (Some((ty, mut targets)), Some((_, more))) = (delegate_targets(i, &x), delegate_targets(i, &y)) else {
            // Either side null: the other one is the answer.
            return Ok(Some(if x.is_null() || delegate_targets(i, &x).is_none() { y } else { x }));
        };
        targets.extend(more);
        Ok(Some(Value::Obj(i.heap.alloc(ClrDelegate { type_id: ty, targets }))))
    });
    interp.register_native("System.Delegate::Remove/2", |i, a| {
        let (source, value) = (arg(i, a, 0)?, arg(i, a, 1)?);
        let Some((ty, targets)) = delegate_targets(i, &source) else { return Ok(Some(Value::Null)) };
        let Some((_, remove)) = delegate_targets(i, &value) else { return Ok(Some(source)) };
        // Remove the last occurrence of the whole invocation list.
        let n = remove.len();
        let mut kept = targets.clone();
        if n > 0 && n <= targets.len() {
            for start in (0..=targets.len() - n).rev() {
                let window = &targets[start..start + n];
                if window.iter().zip(&remove).all(|(p, q)| p.method == q.method && p.receiver == q.receiver) {
                    kept.drain(start..start + n);
                    break;
                }
            }
        }
        if kept.is_empty() {
            return Ok(Some(Value::Null));
        }
        if kept.len() == targets.len() {
            return Ok(Some(source));
        }
        Ok(Some(Value::Obj(i.heap.alloc(ClrDelegate { type_id: ty, targets: kept }))))
    });
    for name in ["System.Delegate::op_Equality/2", "System.MulticastDelegate::op_Equality/2"] {
        interp.register_native(name, |i, a| {
            let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
            Ok(Some(Value::I32(delegates_equal(i, &x, &y) as i32)))
        });
    }
    for name in ["System.Delegate::op_Inequality/2", "System.MulticastDelegate::op_Inequality/2"] {
        interp.register_native(name, |i, a| {
            let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
            Ok(Some(Value::I32(!delegates_equal(i, &x, &y) as i32)))
        });
    }
    interp.register_native("System.Delegate::get_Method()", |_, _| Ok(Some(Value::Null)));
    interp.register_native("System.Delegate::GetInvocationList()", |i, a| {
        let this = arg(i, a, 0)?;
        let Some((ty, targets)) = delegate_targets(i, &this) else { return Ok(Some(Value::Null)) };
        let array = i.alloc_array(ty, targets.len());
        for (n, t) in targets.into_iter().enumerate() {
            let single = i.heap.alloc(ClrDelegate { type_id: ty, targets: vec![t] });
            i.heap.with_mut::<ClrArray, _>(array, |arr| {
                arr.storage.set(n, &Value::Obj(single));
            });
        }
        Ok(Some(Value::Obj(array)))
    });
    // Interlocked.CompareExchange<T>(ref T, T, T): what an event accessor
    // loops on. One interpreter thread here, so compare-and-set is exact.
    interp.register_native("System.Threading.Interlocked::CompareExchange/3", |i, a| {
        let Some(Value::Ref(slot)) = a.first() else { return Err(ExecutionError::null_reference()) };
        let slot: ByRef = slot.clone();
        let (value, comparand) = (arg(i, a, 1)?, arg(i, a, 2)?);
        i.interlocked(|i| {
            let current = i.load_indirect_public(slot.clone())?;
            let same = match (&current, &comparand) {
                (Value::Obj(x), Value::Obj(y)) => x == y,
                (Value::Null, Value::Null) => true,
                (Value::Null, Value::Obj(h)) | (Value::Obj(h), Value::Null) => h.is_null(),
                (x, y) => x == y,
            };
            if same {
                i.store_indirect_public(slot, value)?;
            }
            Ok(Some(current))
        })
    });
    interp.register_native("System.Threading.Interlocked::Exchange/2", |i, a| {
        let Some(Value::Ref(slot)) = a.first() else { return Err(ExecutionError::null_reference()) };
        let slot: ByRef = slot.clone();
        let value = arg(i, a, 1)?;
        i.interlocked(|i| {
            let current = i.load_indirect_public(slot.clone())?;
            i.store_indirect_public(slot, value)?;
            Ok(Some(current))
        })
    });
    // EventArgs is behaviour-free; its constructor initialises nothing.
    interp.register_native("System.EventArgs::.ctor()", |_, _| Ok(None));
    interp.register_native("System.EventArgs::get_Empty()", |i, _| singleton(i, "System.EventArgs"));
    // `EventArgs.Empty` is filled by a native class constructor, so the
    // instance is made on first use, as on .NET, rather than at install.
    if let Some(ty) = interp.loader.registry.find_type_by_name("System.EventArgs") {
        let signature = rustclr_core::metadata::MethodSig {
            calling_convention: 0,
            has_this: false,
            explicit_this: false,
            generic_param_count: 0,
            return_type: rustclr_core::metadata::TypeSig::Void,
            params: Vec::new(),
            sentinel_at: None,
        };
        let cctor = interp.loader.intern_internal_call(ty, ".cctor", signature, "System.EventArgs::.cctor()".into());
        interp.loader.registry.ty_mut(ty).cctor = Some(cctor);
        // Native types are created already initialised; this one has work to do.
        interp.loader.registry.ty_mut(ty).cctor_state = rustclr_core::CctorState::NotRun;
    }
    interp.register_native("System.EventArgs::.cctor()", |i, _| {
        let Some(ty) = i.loader.registry.find_type_by_name("System.EventArgs") else { return Ok(None) };
        let field = i.loader.registry.ty(ty).static_fields.iter().copied().find(|f| i.loader.registry.field(*f).name == "Empty");
        if let Some(field) = field {
            let empty = i.alloc_object(ty);
            i.loader.set_static(field, Value::Obj(empty));
        }
        Ok(None)
    });
}

fn delegates_equal(i: &Interpreter, x: &Value, y: &Value) -> bool {
    match (delegate_targets(i, x), delegate_targets(i, y)) {
        (None, None) => true,
        (Some((_, a)), Some((_, b))) => {
            a.len() == b.len() && a.iter().zip(&b).all(|(p, q)| p.method == q.method && p.receiver == q.receiver)
        }
        _ => false,
    }
}

// ── everything else ──────────────────────────────────────────────────────────

/// A `StringComparer` object: its single field is 1 for a case-insensitive
/// comparer. `None` when the value is not a `StringComparer` at all.
fn string_comparer_ignores_case(i: &Interpreter, v: &Value) -> Option<bool> {
    let h = v.as_handle().filter(|h| !h.is_null())?;
    let ty = i.type_of(h)?;
    if i.loader.registry.ty(ty).full_name() != "System.StringComparer" {
        return None;
    }
    let flag = i.heap.with::<ClrObject, _>(h, |o| o.fields.first().cloned()).flatten();
    Some(flag.map(|f| f.is_truthy()).unwrap_or(false))
}

fn new_string_comparer(i: &mut Interpreter, ignore_case: bool) -> ExecResult<Option<Value>> {
    let ty = i.loader.registry.find_type_by_name("System.StringComparer").ok_or_else(|| argument("StringComparer"))?;
    let h = i.alloc_object(ty);
    i.heap.with_mut::<ClrObject, _>(h, |o| {
        if o.fields.is_empty() {
            o.fields.push(Value::I32(ignore_case as i32));
        } else {
            o.fields[0] = Value::I32(ignore_case as i32);
        }
    });
    Ok(Some(Value::Obj(h)))
}

fn singleton(i: &mut Interpreter, type_name: &str) -> ExecResult<Option<Value>> {
    let ty = i.loader.registry.find_type_by_name(type_name).ok_or_else(|| argument(type_name))?;
    Ok(Some(Value::Obj(i.alloc_object(ty))))
}

/// A small xorshift seeded from the clock, for `Guid.NewGuid`.
fn random_u64(i: &mut Interpreter) -> u64 {
    static mut STATE: u64 = 0;
    // One interpreter thread drives this in practice; a data race here could
    // only produce a different random number.
    unsafe {
        if STATE == 0 {
            STATE = (i.host.monotonic_nanos() ^ (i.host.wall_clock_millis() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1;
        }
        let mut x = STATE;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        STATE = x;
        x
    }
}

fn register_misc(interp: &mut Interpreter) {
    // Exceptions: inner exceptions and GetType on an exception.
    // Every framework exception type takes (message, innerException).
    for name in [
        "Exception", "SystemException", "ApplicationException", "InvalidOperationException", "ArgumentException",
        "ArgumentNullException", "ArgumentOutOfRangeException", "NotSupportedException", "NotImplementedException",
        "FormatException", "NullReferenceException", "IndexOutOfRangeException", "InvalidCastException",
        "DivideByZeroException", "OverflowException", "ArithmeticException", "KeyNotFoundException",
        "TimeoutException", "OperationCanceledException", "UnauthorizedAccessException",
    ] {
        interp.register_native(k(format!("System.{name}::.ctor/2")), exception_with_inner);
    }
    interp.register_native("System.IO.IOException::.ctor/2", exception_with_inner);
    interp.register_native("System.Exception::get_InnerException()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let mut inner = i.heap.with::<ClrException, _>(h, |e| e.inner).unwrap_or(Handle::NULL);
        let framework = i.type_of(h).map(|t| i.loader.registry.ty(t).assembly == i.loader.bcl_assembly()).unwrap_or(false);
        if inner.is_null() && framework {
            inner = i.heap.with::<ClrObject, _>(h, |o| o.fields.get(1).and_then(|v| v.as_handle())).flatten().unwrap_or(Handle::NULL);
        }
        Ok(Some(if inner.is_null() { Value::Null } else { Value::Obj(inner) }))
    });
    interp.register_native("System.Exception::GetType()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let ty = i.type_of(h).ok_or_else(ExecutionError::null_reference)?;
        Ok(Some(Value::Obj(i.type_object(ty))))
    });
    interp.register_native("System.Exception::GetBaseException()", |i, a| {
        let mut h = arg_handle(i, a, 0)?;
        loop {
            let inner = i.heap.with::<ClrException, _>(h, |e| e.inner).unwrap_or(Handle::NULL);
            if inner.is_null() {
                return Ok(Some(Value::Obj(h)));
            }
            h = inner;
        }
    });

    // object.Equals(a, b): null-safe, and a user's Equals override is used.
    interp.register_native("System.Object::Equals(object,object)", |i, a| {
        let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
        if x.is_null() || y.is_null() {
            return Ok(Some(Value::I32((x.is_null() && y.is_null()) as i32)));
        }
        Ok(Some(Value::I32(crate::collections::equals_dispatch(i, &x, &y)? as i32)))
    });

    // Guid: carried as its string form, which is how a program uses one.
    interp.register_native("System.Guid::NewGuid()", |i, _| {
        let (hi, lo) = (random_u64(i), random_u64(i));
        let bytes: Vec<u8> = hi.to_le_bytes().iter().chain(lo.to_le_bytes().iter()).copied().collect();
        let mut b = [0u8; 16];
        b.copy_from_slice(&bytes);
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let text = format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]);
        Ok(Some(string_value(i, &text)))
    });
    interp.register_native("System.Guid::ToString()", |i, a| {
        let v = arg(i, a, 0)?;
        Ok(Some(match v {
            Value::Obj(h) if !h.is_null() => Value::Obj(h),
            _ => string_value(i, "00000000-0000-0000-0000-000000000000"),
        }))
    });
    // `N` is the 32 digits, `D` the default, `B` and `P` the default in braces
    // or parentheses.
    for name in ["System.Guid::ToString(string)", "System.Guid::ToString/2"] {
        interp.register_native(name, |i, a| {
            let text = match arg(i, a, 0)? {
                Value::Obj(h) if !h.is_null() => i.string_value(h).unwrap_or_default(),
                _ => "00000000-0000-0000-0000-000000000000".into(),
            };
            let spec = match arg(i, a, 1)? {
                Value::Obj(h) if !h.is_null() => i.string_value(h).unwrap_or_default(),
                _ => String::new(),
            };
            let out = guid_format(&text, &spec).ok_or_else(|| ExecutionError::exception(
                ClrExceptionKind::Format,
                "Format string can be only \"D\", \"d\", \"N\", \"n\", \"P\", \"p\", \"B\" or \"b\".",
            ))?;
            Ok(Some(string_value(i, &out)))
        });
    }
    for name in ["System.Guid::Parse(string)", "System.Guid::.ctor(string)"] {
        interp.register_native(name, |i, a| {
            let index = if a.len() == 2 { 1 } else { 0 };
            let s = arg_string(i, a, index)?;
            let text = guid_canonical(&s).ok_or_else(|| bad_format(&s))?;
            let value = string_value(i, &text);
            construct(i, a, Ok(Some(value)))
        });
    }
    interp.register_native("System.Guid::TryParse/2", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let parsed = guid_canonical(&s);
        let value = string_value(i, parsed.as_deref().unwrap_or("00000000-0000-0000-0000-000000000000"));
        store_out(i, a, 1, value)?;
        Ok(Some(Value::I32(parsed.is_some() as i32)))
    });
    for name in ["System.Guid::op_Equality/2", "System.Guid::op_Inequality/2", "System.Guid::Equals/1"] {
        interp.register_native(name, |i, a| {
            let text = |i: &mut Interpreter, v: Value| match v {
                Value::Obj(h) if !h.is_null() => i.string_value(h).unwrap_or_default(),
                _ => "00000000-0000-0000-0000-000000000000".into(),
            };
            let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
            let y = unbox_value(i, &y);
            let same = text(i, x) == text(i, y);
            let negate = i
                .current_native_method()
                .map(|m| i.loader.registry.method(m).name == "op_Inequality")
                .unwrap_or(false);
            Ok(Some(Value::I32((same != negate) as i32)))
        });
    }
    interp.register_native("System.Guid::get_Empty()", |i, _| Ok(Some(string_value(i, "00000000-0000-0000-0000-000000000000"))));

    // HashCode.Combine<T1..T8>.
    for n in 1..=8 {
        interp.register_native(k(format!("System.HashCode::Combine/{n}")), |i, a| {
            let mut acc: u64 = 0x9E37_79B9_7F4A_7C15;
            for v in 0..a.len() {
                let value = arg(i, a, v)?;
                acc = acc.rotate_left(5) ^ value_hash(i, &value);
                acc = acc.wrapping_mul(0x100_0000_01B3);
            }
            Ok(Some(Value::I32((acc ^ (acc >> 32)) as i32)))
        });
    }

    // Lazy<T>: (factory, value, created).
    for name in ["System.Lazy`1::.ctor/1", "System.Lazy`1::.ctor/2"] {
        interp.register_native(name, |i, a| {
            let this = receiver(i, a)?;
            let factory = arg(i, a, 1)?;
            // Lazy(T value) is not a constructor .NET has; a non-delegate
            // argument is a `bool isThreadSafe` and the factory is absent.
            let is_delegate = matches!(&factory, Value::Obj(h) if i.heap.with::<ClrDelegate, _>(*h, |_| ()).is_some());
            set_field(i, this, 0, if is_delegate { factory } else { Value::Null });
            set_field(i, this, 2, Value::I32(0));
            Ok(None)
        });
    }
    interp.register_native("System.Lazy`1::get_Value()", |i, a| {
        let this = receiver(i, a)?;
        if field(i, this, 2).is_truthy() {
            return Ok(Some(field(i, this, 1)));
        }
        let factory = field(i, this, 0);
        let value = if factory.is_null() {
            match type_arg(i, 0) {
                Some(t) => {
                    let instance = i.alloc_object(t);
                    Value::Obj(instance)
                }
                None => Value::Null,
            }
        } else {
            crate::linq::call(i, &factory, &[])?
        };
        set_field(i, this, 1, value.clone());
        set_field(i, this, 2, Value::I32(1));
        Ok(Some(value))
    });
    interp.register_native("System.Lazy`1::get_IsValueCreated()", |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(Value::I32(field(i, this, 2).is_truthy() as i32)))
    });

    // Uri escaping (RFC 3986 unreserved characters pass through).
    interp.register_native("System.Uri::EscapeDataString(string)", |i, a| {
        let s = arg_string(i, a, 0)?;
        let mut out = String::with_capacity(s.len() * 3);
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.Uri::EscapeUriString(string)", |i, a| {
        let s = arg_string(i, a, 0)?;
        let mut out = String::with_capacity(s.len() * 3);
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.~:/?#[]@!$&'()*+,;=%".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.Uri::UnescapeDataString(string)", |i, a| {
        let s = arg_string(i, a, 0)?;
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut n = 0;
        while n < bytes.len() {
            if bytes[n] == b'%' && n + 2 < bytes.len() + 0 && n + 2 <= bytes.len() - 1 {
                if let Ok(v) = u8::from_str_radix(&s[n + 1..n + 3], 16) {
                    out.push(v);
                    n += 3;
                    continue;
                }
            }
            out.push(bytes[n]);
            n += 1;
        }
        let text = String::from_utf8_lossy(&out).into_owned();
        Ok(Some(string_value(i, &text)))
    });

    // Cultures: one, the invariant one.
    for name in [
        "System.Globalization.CultureInfo::get_InvariantCulture()",
        "System.Globalization.CultureInfo::get_CurrentCulture()",
        "System.Globalization.CultureInfo::get_CurrentUICulture()",
        "System.Globalization.CultureInfo::.ctor(string)",
        "System.Globalization.CultureInfo::GetCultureInfo(string)",
    ] {
        interp.register_native(name, |i, a| {
            // A constructor call has `this` already allocated.
            if let Some(Value::Obj(h)) = a.first() {
                if !h.is_null() && i.string_value(*h).is_none() {
                    return Ok(None);
                }
            }
            singleton(i, "System.Globalization.CultureInfo")
        });
    }
    interp.register_native("System.Globalization.CultureInfo::get_Name()", |i, _| Ok(Some(string_value(i, ""))));
    interp.register_native("System.Globalization.CultureInfo::get_NumberFormat()", |i, _| {
        singleton(i, "System.Globalization.NumberFormatInfo")
    });
    interp.register_native("System.Globalization.NumberFormatInfo::get_NumberDecimalSeparator()", |i, _| {
        Ok(Some(string_value(i, ".")))
    });

    // StringComparer.Ordinal / OrdinalIgnoreCase and their use as comparers.
    for (name, ignore) in [
        ("get_Ordinal()", false),
        ("get_OrdinalIgnoreCase()", true),
        ("get_InvariantCulture()", false),
        ("get_InvariantCultureIgnoreCase()", true),
        ("get_CurrentCulture()", false),
        ("get_CurrentCultureIgnoreCase()", true),
    ] {
        let f: rustclr_core::NativeFn = if ignore { |i, _| new_string_comparer(i, true) } else { |i, _| new_string_comparer(i, false) };
        interp.register_native(k(format!("System.StringComparer::{name}")), f);
    }
    interp.register_native("System.StringComparer::Compare/2", |i, a| {
        let ignore = { let v__ = arg(i, a, 0)?; string_comparer_ignores_case(i, &v__) }.unwrap_or(false);
        let (x, y) = (arg(i, a, 1)?, arg(i, a, 2)?);
        Ok(Some(Value::I32(compare_units(&string_units(i, &x), &string_units(i, &y), ignore))))
    });
    interp.register_native("System.StringComparer::Equals/2", |i, a| {
        let ignore = { let v__ = arg(i, a, 0)?; string_comparer_ignores_case(i, &v__) }.unwrap_or(false);
        let (x, y) = (arg(i, a, 1)?, arg(i, a, 2)?);
        let (ux, uy) = (string_units(i, &x), string_units(i, &y));
        Ok(Some(Value::I32((ux.len() == uy.len() && compare_units(&ux, &uy, ignore) == 0) as i32)))
    });
    interp.register_native("System.StringComparer::GetHashCode/1", |i, a| {
        let ignore = { let v__ = arg(i, a, 0)?; string_comparer_ignores_case(i, &v__) }.unwrap_or(false);
        let x = arg(i, a, 1)?;
        let units: Vec<u16> = string_units(i, &x).into_iter().map(|u| if ignore { fold(u) } else { u }).collect();
        let mut acc: u32 = 2_166_136_261;
        for u in units {
            acc ^= u as u32;
            acc = acc.wrapping_mul(16_777_619);
        }
        Ok(Some(Value::I32(acc as i32)))
    });

    // ConstructorInfo.Invoke(args): allocate, then run the constructor, as
    // `newobj` would. Dependency injection builds services this way.
    interp.register_native("System.Reflection.ConstructorInfo::Invoke(object[])", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let ctor = MethodId(field(i, this, 0).as_i32().unwrap_or(0) as u32);
        let declaring = i.loader.registry.method(ctor).declaring_type;
        let params = i.loader.registry.method(ctor).signature.params.clone();
        let supplied = match arg(i, a, 1)? {
            Value::Obj(h) if !h.is_null() => array_values(i, h),
            _ => Vec::new(),
        };
        if supplied.len() != params.len() {
            return Err(ExecutionError::exception(ClrExceptionKind::Argument, "Parameter count mismatch."));
        }
        let instance = i.alloc_object(declaring);
        let mut call_args = vec![Value::Obj(instance)];
        for (value, param) in supplied.into_iter().zip(params.iter()) {
            // A boxed value-type argument arrives as an object; the
            // constructor takes the value.
            let boxed_value = !matches!(
                param.unwrap_modifiers(),
                rustclr_core::metadata::TypeSig::Class(_) | rustclr_core::metadata::TypeSig::Object
                    | rustclr_core::metadata::TypeSig::String | rustclr_core::metadata::TypeSig::SzArray(_)
                    | rustclr_core::metadata::TypeSig::GenericInst { is_value_type: false, .. }
            );
            call_args.push(if boxed_value { unbox_value(i, &value) } else { value });
        }
        i.invoke(ctor, call_args)?;
        Ok(Some(Value::Obj(instance)))
    });
    interp.register_native("System.String::Join(string,string[],int,int)", |i, a| {
        let separator = arg_string_or_empty(i, a, 0)?;
        let items = { let h__ = arg_handle(i, a, 1)?; array_values(i, h__) };
        let (start, count) = (arg_i32(i, a, 2)?.max(0) as usize, arg_i32(i, a, 3)?.max(0) as usize);
        if start + count > items.len() {
            return Err(out_range("count"));
        }
        let parts: Vec<String> = items[start..start + count].iter().map(|v| display(i, v)).collect();
        let joined = parts.join(&separator);
        Ok(Some(string_value(i, &joined)))
    });

    // Base64: canvas pixels, file contents and binary messages cross the
    // JavaScript bridge as text.
    interp.register_native("System.Convert::ToBase64String(byte[])", |i, a| {
        let bytes = arg_bytes(i, a, 0)?;
        let text = base64_encode(&bytes);
        Ok(Some(string_value(i, &text)))
    });
    interp.register_native("System.Convert::FromBase64String(string)", |i, a| {
        let text = arg_string(i, a, 0)?;
        let bytes = base64_decode(&text).ok_or_else(|| bad_format("base64"))?;
        Ok(Some(Value::Obj(byte_array(i, &bytes))))
    });

    // BitConverter: bit patterns between numbers and bytes (little-endian).
    const BC: &str = "System.BitConverter";
    interp.register_native(k(format!("{BC}::get_IsLittleEndian()")), |_, _| Ok(Some(Value::I32(1))));
    interp.register_native(k(format!("{BC}::SingleToInt32Bits(float)")), |i, a| Ok(Some(Value::I32((arg_f64(i, a, 0)? as f32).to_bits() as i32))));
    interp.register_native(k(format!("{BC}::Int32BitsToSingle(int)")), |i, a| Ok(Some(Value::F(f32::from_bits(arg_i32(i, a, 0)? as u32) as f64))));
    interp.register_native(k(format!("{BC}::DoubleToInt64Bits(double)")), |i, a| Ok(Some(Value::I64(arg_f64(i, a, 0)?.to_bits() as i64))));
    interp.register_native(k(format!("{BC}::Int64BitsToDouble(long)")), |i, a| Ok(Some(Value::F(f64::from_bits(arg_i64(i, a, 0)? as u64)))));
    interp.register_native(k(format!("{BC}::GetBytes(int)")), |i, a| {
        let bytes = arg_i32(i, a, 0)?.to_le_bytes();
        Ok(Some(Value::Obj(byte_array(i, &bytes))))
    });
    interp.register_native(k(format!("{BC}::GetBytes(long)")), |i, a| {
        let bytes = arg_i64(i, a, 0)?.to_le_bytes();
        Ok(Some(Value::Obj(byte_array(i, &bytes))))
    });
    interp.register_native(k(format!("{BC}::GetBytes(float)")), |i, a| {
        let bytes = (arg_f64(i, a, 0)? as f32).to_le_bytes();
        Ok(Some(Value::Obj(byte_array(i, &bytes))))
    });
    interp.register_native(k(format!("{BC}::GetBytes(double)")), |i, a| {
        let bytes = arg_f64(i, a, 0)?.to_le_bytes();
        Ok(Some(Value::Obj(byte_array(i, &bytes))))
    });
    interp.register_native(k(format!("{BC}::GetBytes(short)")), |i, a| {
        let bytes = (arg_i32(i, a, 0)? as i16).to_le_bytes();
        Ok(Some(Value::Obj(byte_array(i, &bytes))))
    });
    fn slice<const N: usize>(i: &mut Interpreter, a: &[Value]) -> ExecResult<[u8; N]> {
        let bytes = arg_bytes(i, a, 0)?;
        let at = arg_i32(i, a, 1)?.max(0) as usize;
        if at + N > bytes.len() {
            return Err(out_range("startIndex"));
        }
        let mut out = [0u8; N];
        out.copy_from_slice(&bytes[at..at + N]);
        Ok(out)
    }
    interp.register_native(k(format!("{BC}::ToInt32(byte[],int)")), |i, a| Ok(Some(Value::I32(i32::from_le_bytes(slice::<4>(i, a)?)))));
    interp.register_native(k(format!("{BC}::ToUInt32(byte[],int)")), |i, a| Ok(Some(Value::I32(u32::from_le_bytes(slice::<4>(i, a)?) as i32))));
    interp.register_native(k(format!("{BC}::ToInt16(byte[],int)")), |i, a| Ok(Some(Value::I32(i16::from_le_bytes(slice::<2>(i, a)?) as i32))));
    interp.register_native(k(format!("{BC}::ToUInt16(byte[],int)")), |i, a| Ok(Some(Value::I32(u16::from_le_bytes(slice::<2>(i, a)?) as i32))));
    interp.register_native(k(format!("{BC}::ToInt64(byte[],int)")), |i, a| Ok(Some(Value::I64(i64::from_le_bytes(slice::<8>(i, a)?)))));
    interp.register_native(k(format!("{BC}::ToSingle(byte[],int)")), |i, a| Ok(Some(Value::F(f32::from_le_bytes(slice::<4>(i, a)?) as f64))));
    interp.register_native(k(format!("{BC}::ToDouble(byte[],int)")), |i, a| Ok(Some(Value::F(f64::from_le_bytes(slice::<8>(i, a)?)))));
    interp.register_native(k(format!("{BC}::ToString(byte[])")), |i, a| {
        let bytes = arg_bytes(i, a, 0)?;
        let text: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
        Ok(Some(string_value(i, &text.join("-"))))
    });

    // Assembly and Module are compared by the assembly they describe: two
    // `typeof(X).Assembly` calls make two objects for one assembly.
    for ty in ["System.Reflection.Assembly", "System.Reflection.Module"] {
        interp.register_native(k(format!("{ty}::op_Equality/2")), |i, a| {
            let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
            Ok(Some(Value::I32(same_assembly(i, &x, &y) as i32)))
        });
        interp.register_native(k(format!("{ty}::op_Inequality/2")), |i, a| {
            let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
            Ok(Some(Value::I32(!same_assembly(i, &x, &y) as i32)))
        });
        interp.register_native(k(format!("{ty}::Equals(object)")), |i, a| {
            let (x, y) = (arg(i, a, 0)?, arg(i, a, 1)?);
            Ok(Some(Value::I32(same_assembly(i, &x, &y) as i32)))
        });
        interp.register_native(k(format!("{ty}::GetHashCode()")), |i, a| {
            let x = arg(i, a, 0)?;
            let id = x.as_handle().filter(|h| !h.is_null()).map(|h| field(i, h, 0).as_i32().unwrap_or(0)).unwrap_or(0);
            Ok(Some(Value::I32(id)))
        });
    }
    let _ = return_type_name;
    let _ = MethodId(0);
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `IReadOnlyList<T>`, `IList<T>` and the collection interfaces, when the
/// receiver is a built-in collection: an array, a `List<T>`, a set or a map.
/// A user type implementing the interface is dispatched to its own method
/// before these are consulted; these serve the receivers that have no IL.
fn register_collection_interfaces(interp: &mut Interpreter) {
    const G: &str = "System.Collections.Generic";
    for iface in ["IReadOnlyCollection`1", "ICollection`1", "IReadOnlyList`1", "IList`1", "IReadOnlyDictionary`2", "IDictionary`2"] {
        interp.register_native(k(format!("{G}.{iface}::get_Count()")), |i, a| {
            let this = arg(i, a, 0)?;
            Ok(Some(Value::I32(sequence_values(i, &this).len() as i32)))
        });
    }
    for iface in ["IReadOnlyList`1", "IList`1"] {
        interp.register_native(k(format!("{G}.{iface}::get_Item(int)")), |i, a| {
            let this = arg(i, a, 0)?;
            let index = arg_i32(i, a, 1)?;
            let values = sequence_values(i, &this);
            values
                .get(index.max(0) as usize)
                .cloned()
                .filter(|_| index >= 0)
                .map(Some)
                .ok_or_else(|| out_range("index"))
        });
        interp.register_native(k(format!("{G}.{iface}::IndexOf(!0)")), |i, a| {
            let this = arg(i, a, 0)?;
            let item = arg(i, a, 1)?;
            let values = sequence_values(i, &this);
            Ok(Some(Value::I32(values.iter().position(|v| values_equal(i, v, &item)).map(|p| p as i32).unwrap_or(-1))))
        });
    }
    interp.register_native(k(format!("{G}.IList`1::set_Item(int,!0)")), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let index = arg_i32(i, a, 1)?.max(0) as usize;
        let value = arg(i, a, 2)?;
        let target = if i.heap.with::<ClrArray, _>(this, |_| ()).is_some() { this } else { list_items(i, this) };
        let ok = i.heap.with_mut::<ClrArray, _>(target, |arr| index < arr.len() && arr.storage.set(index, &value)).unwrap_or(false);
        if ok { Ok(None) } else { Err(out_range("index")) }
    });
    interp.register_native(k(format!("{G}.ICollection`1::Contains(!0)")), |i, a| {
        let this = arg(i, a, 0)?;
        let item = arg(i, a, 1)?;
        let values = sequence_values(i, &this);
        Ok(Some(Value::I32(values.iter().any(|v| values_equal(i, v, &item)) as i32)))
    });
    // `IEnumerable<T>.GetEnumerator()` on a receiver with no enumerator of its
    // own — an array, reached through the interface (`IEnumerable<string> xs =
    // new[] { ... }; foreach (var x in xs)`). Types that have one, managed or
    // native, are dispatched to it before this fallback is consulted. Arrays
    // are walked in place; anything else is walked over a snapshot.
    fn enumerate(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
        let this = arg_handle(i, a, 0)?;
        let source = if i.heap.with::<ClrArray, _>(this, |_| ()).is_some() {
            this
        } else {
            let values = sequence_values(i, &Value::Obj(this));
            let copy = i.alloc_value_array(0);
            with_values(i, copy, |v| *v = values);
            copy
        };
        Ok(Some(crate::collections::new_enumerator(i, "List`1+Enumerator", source)))
    }
    interp.register_native(k(format!("{G}.IEnumerable`1::GetEnumerator()")), enumerate);
    interp.register_native("System.Collections.IEnumerable::GetEnumerator()", enumerate);
    interp.register_native("System.Collections.ICollection::get_Count()", |i, a| {
        let this = arg(i, a, 0)?;
        Ok(Some(Value::I32(sequence_values(i, &this).len() as i32)))
    });

    // `IDictionary.GetEnumerator()` yields `DictionaryEntry`, where the
    // dictionary's own enumerator yields `KeyValuePair`. The entries are a
    // snapshot, as a `foreach` that does not modify the dictionary cannot tell.
    interp.register_native(k(format!("{G}.Dictionary`2::System.Collections.IDictionary.GetEnumerator()")), |i, a| {
        let this = arg(i, a, 0)?;
        let Some(entry_type) = i.loader.registry.find_type_by_name("System.Collections.DictionaryEntry") else {
            return Ok(Some(Value::Null));
        };
        let pairs = sequence_values(i, &this);
        let mut entries = Vec::with_capacity(pairs.len());
        for pair in pairs {
            let Some(p) = pair.as_handle().filter(|h| !h.is_null()) else { continue };
            let (key, value) = (field(i, p, 0), field(i, p, 1));
            // `DictionaryEntry` holds objects: a `double` value is read back
            // through `case double d`, which only a boxed one satisfies.
            let key = crate::collections::box_for_object(i, key, None);
            let value = crate::collections::box_for_object(i, value, None);
            let entry = i.alloc_object(entry_type);
            set_field(i, entry, 0, key);
            set_field(i, entry, 1, value);
            entries.push(Value::Obj(entry));
        }
        let array = i.alloc_value_array(0);
        with_values(i, array, |v| *v = entries);
        Ok(Some(crate::collections::new_enumerator(i, "List`1+Enumerator", array)))
    });
    interp.register_native("System.Collections.DictionaryEntry::get_Key()", |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(field(i, this, 0)))
    });
    interp.register_native("System.Collections.DictionaryEntry::get_Value()", |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(field(i, this, 1)))
    });
    // The non-generic `IList`, which a serializer fills a `List<T>` through
    // once it has made one from a `Type`. Values arrive boxed; a `List<int>`
    // stores them unboxed, as its own `Add` would.
    fn list_values(i: &mut Interpreter, a: &[Value]) -> ExecResult<Handle> {
        let this = arg_handle(i, a, 0)?;
        if i.heap.with::<ClrArray, _>(this, |_| ()).is_some() {
            return Ok(this);
        }
        Ok(crate::collections::ensure_list(i, this))
    }
    fn fixed_size(i: &Interpreter, a: &[Value]) -> bool {
        a.first().and_then(|v| v.as_handle()).map(|h| i.heap.with::<ClrArray, _>(h, |_| ()).is_some()).unwrap_or(false)
    }
    fn not_supported() -> ExecutionError {
        ExecutionError::exception(ClrExceptionKind::NotSupported, "Collection was of a fixed size.")
    }
    interp.register_native("System.Collections.IList::Add(object)", |i, a| {
        if fixed_size(i, a) { return Err(not_supported()); }
        let items = list_values(i, a)?;
        let value = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        let mut index = 0;
        with_values(i, items, |v| { index = v.len(); v.push(value); });
        Ok(Some(Value::I32(index as i32)))
    });
    interp.register_native("System.Collections.IList::Insert(int,object)", |i, a| {
        if fixed_size(i, a) { return Err(not_supported()); }
        let items = list_values(i, a)?;
        let index = arg_i32(i, a, 1)?;
        let value = { let v = arg(i, a, 2)?; unbox_value(i, &v) };
        let mut ok = false;
        with_values(i, items, |v| {
            if index >= 0 && index as usize <= v.len() { v.insert(index as usize, value); ok = true; }
        });
        if ok { Ok(None) } else { Err(out_range("index")) }
    });
    interp.register_native("System.Collections.IList::RemoveAt(int)", |i, a| {
        if fixed_size(i, a) { return Err(not_supported()); }
        let items = list_values(i, a)?;
        let index = arg_i32(i, a, 1)?;
        let mut ok = false;
        with_values(i, items, |v| {
            if index >= 0 && (index as usize) < v.len() { v.remove(index as usize); ok = true; }
        });
        if ok { Ok(None) } else { Err(out_range("index")) }
    });
    interp.register_native("System.Collections.IList::Clear()", |i, a| {
        if fixed_size(i, a) { return Err(not_supported()); }
        let items = list_values(i, a)?;
        with_values(i, items, |v| v.clear());
        Ok(None)
    });
    interp.register_native("System.Collections.IList::get_Item(int)", |i, a| {
        let this = arg(i, a, 0)?;
        let index = arg_i32(i, a, 1)?;
        let values = sequence_values(i, &this);
        let value = values.get(index.max(0) as usize).cloned().filter(|_| index >= 0).ok_or_else(|| out_range("index"))?;
        Ok(Some(crate::collections::box_for_object(i, value, None)))
    });
    interp.register_native("System.Collections.IList::set_Item(int,object)", |i, a| {
        let items = list_values(i, a)?;
        let index = arg_i32(i, a, 1)?;
        let value = { let v = arg(i, a, 2)?; unbox_value(i, &v) };
        let ok = i
            .heap
            .with_mut::<ClrArray, _>(items, |arr| index >= 0 && (index as usize) < arr.len() && arr.storage.set(index as usize, &value))
            .unwrap_or(false);
        if ok { Ok(None) } else { Err(out_range("index")) }
    });
    interp.register_native("System.Collections.IList::Contains(object)", |i, a| {
        let this = arg(i, a, 0)?;
        let item = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        let values = sequence_values(i, &this);
        Ok(Some(Value::I32(values.iter().any(|v| values_equal(i, v, &item)) as i32)))
    });
    interp.register_native("System.Collections.IList::IndexOf(object)", |i, a| {
        let this = arg(i, a, 0)?;
        let item = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        let values = sequence_values(i, &this);
        Ok(Some(Value::I32(values.iter().position(|v| values_equal(i, v, &item)).map(|p| p as i32).unwrap_or(-1))))
    });
    interp.register_native("System.Collections.IList::get_IsFixedSize()", |i, a| Ok(Some(Value::I32(fixed_size(i, a) as i32))));

    // The non-generic `IDictionary`, for the same reason. A dictionary made by
    // `Activator` from a `Type` may not have run its native constructor yet.
    fn dictionary(i: &mut Interpreter, a: &[Value]) -> ExecResult<Handle> {
        let this = arg_handle(i, a, 0)?;
        if this.is_null() {
            return Err(ExecutionError::null_reference());
        }
        if field(i, this, 0).as_handle().map(|h| h.is_null()).unwrap_or(true) {
            crate::collections::reset_dictionary(i, this);
        }
        Ok(this)
    }
    interp.register_native("System.Collections.IDictionary::set_Item(object,object)", |i, a| {
        let this = dictionary(i, a)?;
        let key = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        let value = { let v = arg(i, a, 2)?; unbox_value(i, &v) };
        crate::collections::dictionary_set(i, this, key, value);
        Ok(None)
    });
    interp.register_native("System.Collections.IDictionary::Add(object,object)", |i, a| {
        let this = dictionary(i, a)?;
        let key = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        if crate::collections::dict_contains(i, this, &key) {
            return Err(argument("An item with the same key has already been added."));
        }
        let value = { let v = arg(i, a, 2)?; unbox_value(i, &v) };
        crate::collections::dictionary_set(i, this, key, value);
        Ok(None)
    });
    interp.register_native("System.Collections.IDictionary::get_Item(object)", |i, a| {
        let this = dictionary(i, a)?;
        let key = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        match crate::collections::dict_get(i, this, &key) {
            Some(value) => Ok(Some(crate::collections::box_for_object(i, value, None))),
            None => Ok(Some(Value::Null)),
        }
    });
    interp.register_native("System.Collections.IDictionary::Contains(object)", |i, a| {
        let this = dictionary(i, a)?;
        let key = { let v = arg(i, a, 1)?; unbox_value(i, &v) };
        Ok(Some(Value::I32(crate::collections::dict_contains(i, this, &key) as i32)))
    });
    interp.register_native("System.Collections.IList::get_IsReadOnly()", |_i, _a| Ok(Some(Value::I32(0))));

    interp.register_native(k(format!("{G}.ICollection`1::get_IsReadOnly()")), |i, a| {
        let this = arg_handle(i, a, 0)?;
        Ok(Some(Value::I32(i.heap.with::<ClrArray, _>(this, |_| ()).is_some() as i32)))
    });
}

fn same_assembly(i: &Interpreter, x: &Value, y: &Value) -> bool {
    let id = |v: &Value| v.as_handle().filter(|h| !h.is_null()).map(|h| field(i, h, 0).as_i32().unwrap_or(-1));
    id(x) == id(y)
}

fn exception_with_inner(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let this = arg_handle(i, a, 0)?;
    let message = arg_string_or_empty(i, a, 1)?;
    let inner = arg_handle(i, a, 2)?;
    let mut message = Some(message);
    let stored = i.heap.with_mut::<ClrException, _>(this, |e| {
        e.message = message.take().unwrap_or_default();
        e.inner = inner;
    });
    if stored.is_none() {
        let text = message.unwrap_or_default();
        let h = i.alloc_string(&text);
        // A framework exception allocated as a plain object has no fields of
        // its own, so slot 1 is free for the inner exception. A user type's
        // slots are its own fields and are left alone.
        let framework = i.type_of(this).map(|t| i.loader.registry.ty(t).assembly == i.loader.bcl_assembly()).unwrap_or(false);
        i.heap.with_mut::<ClrObject, _>(this, |o| {
            if o.fields.is_empty() {
                o.fields.push(Value::Obj(h));
            } else {
                o.fields[0] = Value::Obj(h);
            }
            if framework {
                if o.fields.len() < 2 {
                    o.fields.resize(2, Value::Null);
                }
                o.fields[1] = Value::Obj(inner);
            }
        });
    }
    Ok(None)
}


/// Members modern C# reaches for that the libraries Rustzor builds on use:
/// interpolated `StringBuilder.Append`, `string.Create` with a culture,
/// `Array.Clone`, base-n `Convert` parsing and `double.CompareTo`.
fn register_framework_gaps(interp: &mut Interpreter) {
    interp.register_native("System.Net.WebUtility::HtmlEncode(string)", |i, a| {
        let Some(text) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()).and_then(|h| i.string_value(h)) else {
            return Ok(Some(Value::Null));
        };
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            match c {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '&' => out.push_str("&amp;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                c => out.push(c),
            }
        }
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.Net.WebUtility::HtmlDecode(string)", |i, a| {
        let Some(text) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()).and_then(|h| i.string_value(h)) else {
            return Ok(Some(Value::Null));
        };
        let out = text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.Net.WebUtility::UrlEncode(string)", |i, a| {
        let Some(text) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()).and_then(|h| i.string_value(h)) else {
            return Ok(Some(Value::Null));
        };
        let mut out = String::new();
        for b in text.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'*' | b'(' | b')' => out.push(b as char),
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.Net.WebUtility::UrlDecode(string)", |i, a| {
        let Some(text) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()).and_then(|h| i.string_value(h)) else {
            return Ok(Some(Value::Null));
        };
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut k = 0;
        while k < bytes.len() {
            match bytes[k] {
                b'+' => out.push(b' '),
                b'%' if k + 2 < bytes.len() => {
                    match u8::from_str_radix(core::str::from_utf8(&bytes[k + 1..k + 3]).unwrap_or(""), 16) {
                        Ok(v) => {
                            out.push(v);
                            k += 2;
                        }
                        Err(_) => out.push(b'%'),
                    }
                }
                c => out.push(c),
            }
            k += 1;
        }
        Ok(Some(string_value(i, &String::from_utf8_lossy(&out))))
    });
    crate::interpolation::register_builder_handler(interp);
    // `sb.Append($"...")` and `sb.AppendLine($"...")` pass the handler by
    // reference; any other single-argument overload without its own binding
    // appends the value's display text.
    fn append_any(i: &mut Interpreter, a: &[Value], newline: bool) -> ExecResult<Option<Value>> {
        let this = arg_handle(i, a, 0)?;
        let value = a.get(1).cloned().unwrap_or(Value::Null);
        let mut text = match &value {
            Value::Ref(_) => crate::interpolation::take_handler_text(i, &value)?,
            other => display(i, other),
        };
        if newline {
            text.push(char::from(10u8));
        }
        crate::strings::builder_append(i, this, &text);
        Ok(Some(Value::Obj(this)))
    }
    interp.register_native("System.Text.StringBuilder::Append/1", |i, a| append_any(i, a, false));
    interp.register_native("System.Text.StringBuilder::AppendLine/1", |i, a| append_any(i, a, true));
    interp.register_native("System.Text.StringBuilder::Append/2", |i, a| {
        // Append(IFormatProvider, ref handler)
        let this = arg_handle(i, a, 0)?;
        let handler = a.get(2).cloned().unwrap_or(Value::Null);
        let text = crate::interpolation::take_handler_text(i, &handler)?;
        crate::strings::builder_append(i, this, &text);
        Ok(Some(Value::Obj(this)))
    });
    // string.Create(IFormatProvider, ref DefaultInterpolatedStringHandler)
    interp.register_native("System.String::Create/2", |i, a| {
        let handler = a.get(1).cloned().unwrap_or(Value::Null);
        let text = crate::interpolation::take_handler_text(i, &handler)?;
        Ok(Some(string_value(i, &text)))
    });
    interp.register_native("System.Array::Clone()", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let copy = i.heap.with::<ClrArray, _>(this, |arr| arr.clone()).ok_or_else(ExecutionError::null_reference)?;
        Ok(Some(Value::Obj(i.heap.alloc(copy))))
    });
    fn parse_radix(i: &mut Interpreter, a: &[Value]) -> ExecResult<i64> {
        let s = arg_string(i, a, 0)?;
        let base = arg_i32(i, a, 1)? as u32;
        let digits = s.trim().trim_start_matches("0x").trim_start_matches("0X");
        i64::from_str_radix(digits, base).map_err(|_| bad_format(&s))
    }
    interp.register_native("System.Convert::ToByte(string,int)", |i, a| Ok(Some(Value::I32(parse_radix(i, a)? as u8 as i32))));
    interp.register_native("System.Convert::ToInt16(string,int)", |i, a| Ok(Some(Value::I32(parse_radix(i, a)? as i16 as i32))));
    interp.register_native("System.Convert::ToInt64(string,int)", |i, a| Ok(Some(Value::I64(parse_radix(i, a)?))));
    interp.register_native("System.Convert::ToUInt32(string,int)", |i, a| Ok(Some(Value::I32(parse_radix(i, a)? as u32 as i32))));
    fn compare_f(x: f64, y: f64) -> i32 {
        // .NET orders NaN below everything, and equal to itself.
        match (x.is_nan(), y.is_nan()) {
            (true, true) => 0,
            (true, false) => -1,
            (false, true) => 1,
            _ => if x < y { -1 } else if x > y { 1 } else { 0 },
        }
    }
    interp.register_native("System.Double::CompareTo(double)", |i, a| Ok(Some(Value::I32(compare_f(arg_f64(i, a, 0)?, arg_f64(i, a, 1)?)))));
    interp.register_native("System.Single::CompareTo(float)", |i, a| Ok(Some(Value::I32(compare_f(arg_f64(i, a, 0)?, arg_f64(i, a, 1)?)))));
    interp.register_native("System.Int64::CompareTo(long)", |i, a| Ok(Some(Value::I32(arg_i64(i, a, 0)?.cmp(&arg_i64(i, a, 1)?) as i32))));
}

/// A GUID in any of the `D`, `N`, `B` or `P` shapes → the lower-case `D` form.
fn guid_canonical(text: &str) -> Option<String> {
    let digits: String = text
        .trim()
        .trim_matches(|c| c == '{' || c == '}' || c == '(' || c == ')')
        .chars()
        .filter(|c| *c != '-')
        .collect();
    if digits.len() != 32 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let d = digits.to_ascii_lowercase();
    Some(format!("{}-{}-{}-{}-{}", &d[0..8], &d[8..12], &d[12..16], &d[16..20], &d[20..32]))
}

/// A GUID's `D` form in the shape `spec` names, or `None` for an unknown spec.
fn guid_format(text: &str, spec: &str) -> Option<String> {
    Some(match spec {
        "" | "D" | "d" => text.to_string(),
        "N" | "n" => text.replace('-', ""),
        "B" | "b" => format!("{{{text}}}"),
        "P" | "p" => format!("({text})"),
        _ => return None,
    })
}

/// Surrogate pairs: how .NET code walks text with emoji in it. Full tier only;
/// the minimal tier keeps `char.ToString` and `StringBuilder`, which already
/// carry surrogates through as code units.
pub(crate) fn register_surrogates(interp: &mut Interpreter) {
    // Surrogate pairs: how .NET code walks text with emoji in it.
    interp.register_native("System.Char::ConvertFromUtf32(int)", |i, a| {
        let code = arg_i32(i, a, 0)? as u32;
        let Some(c) = char::from_u32(code) else {
            return Err(ExecutionError::exception(
                rustclr_core::ClrExceptionKind::ArgumentOutOfRange,
                "A valid UTF32 value is between 0x000000 and 0x10ffff, inclusive, and should not include surrogate codepoint values (0x00d800 ~ 0x00dfff).",
            ));
        };
        Ok(Some(string_value(i, c.encode_utf8(&mut [0; 4]))))
    });
    interp.register_native("System.Char::ConvertToUtf32(char,char)", |i, a| {
        let (hi, lo) = (arg_i32(i, a, 0)? as u32, arg_i32(i, a, 1)? as u32);
        Ok(Some(Value::I32((0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)) as i32)))
    });
    interp.register_native("System.Char::ConvertToUtf32(string,int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let at = arg_i32(i, a, 1)? as usize;
        let units = i.heap.with::<rustclr_core::ClrString, _>(h, |s| s.units.clone()).unwrap_or_default();
        let hi = units.get(at).copied().unwrap_or(0) as u32;
        let code = match units.get(at + 1) {
            Some(&lo) if (0xD800..0xDC00).contains(&hi) && (0xDC00..0xE000).contains(&(lo as u32)) => 0x10000 + ((hi - 0xD800) << 10) + (lo as u32 - 0xDC00),
            _ => hi,
        };
        Ok(Some(Value::I32(code as i32)))
    });
    fn unit_at(i: &mut Interpreter, a: &[Value]) -> ExecResult<u32> {
        // (char) or (string, int).
        match arg(i, a, 0)? {
            Value::Obj(h) => {
                let at = arg_i32(i, a, 1)? as usize;
                Ok(i.heap.with::<rustclr_core::ClrString, _>(h, |s| s.units.get(at).copied().unwrap_or(0)).unwrap_or(0) as u32)
            }
            v => Ok(v.as_i32().unwrap_or(0) as u32),
        }
    }
    for (name, lo, hi) in [("IsSurrogate", 0xD800u32, 0xE000u32), ("IsHighSurrogate", 0xD800, 0xDC00), ("IsLowSurrogate", 0xDC00, 0xE000)] {
        for shape in ["(char)", "(string,int)"] {
            let k: &'static str = Box::leak(format!("System.Char::{name}{shape}").into_boxed_str());
            match (lo, hi) {
                (0xD800, 0xE000) => interp.register_native(k, |i, a| { let u = unit_at(i, a)?; Ok(Some(Value::I32((0xD800..0xE000).contains(&u) as i32))) }),
                (0xD800, _) => interp.register_native(k, |i, a| { let u = unit_at(i, a)?; Ok(Some(Value::I32((0xD800..0xDC00).contains(&u) as i32))) }),
                _ => interp.register_native(k, |i, a| { let u = unit_at(i, a)?; Ok(Some(Value::I32((0xDC00..0xE000).contains(&u) as i32))) }),
            }
        }
    }
    interp.register_native("System.Char::IsSurrogatePair(char,char)", |i, a| {
        let (hi, lo) = (arg_i32(i, a, 0)? as u32, arg_i32(i, a, 1)? as u32);
        Ok(Some(Value::I32(((0xD800..0xDC00).contains(&hi) && (0xDC00..0xE000).contains(&lo)) as i32)))
    });
}
