//! `System.String`, `System.Char` and `System.Text.StringBuilder`.

use crate::support::*;
use rustclr_core::{ClrObject, ClrString, ExecResult, Interpreter, Value};
use rustclr_gc::Handle;

#[allow(unused_imports)]
use crate::prelude::*;

pub fn register(interp: &mut Interpreter) {
    // -- String --------------------------------------------------------------
    interp.register_native("System.String::get_Length()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(Value::I32(s.encode_utf16().count() as i32)))
    });

    interp.register_native("System.String::get_Chars(int)", |i, a| {
        let h = arg_handle(i, a, 0)?;
        let index = arg_i32(i, a, 1)?;
        let unit = i
            .heap.with::<ClrString, _>(h, |s| s.char_at(index.max(0) as usize)).flatten();
        match unit {
            Some(u) => Ok(Some(Value::I32(u as i32))),
            None => Err(out_of_range("index")),
        }
    });

    interp.register_native("System.String::Concat(string,string)", |i, a| {
        let x = arg_string_or_empty(i, a, 0)?;
        let y = arg_string_or_empty(i, a, 1)?;
        Ok(Some(string_value(i, &format!("{x}{y}"))))
    });
    interp.register_native("System.String::Concat(string,string,string)", |i, a| {
        let x = arg_string_or_empty(i, a, 0)?;
        let y = arg_string_or_empty(i, a, 1)?;
        let z = arg_string_or_empty(i, a, 2)?;
        Ok(Some(string_value(i, &format!("{x}{y}{z}"))))
    });
    interp.register_native("System.String::Concat(string,string,string,string)", |i, a| {
        let mut out = String::new();
        for k in 0..4 {
            out.push_str(&arg_string_or_empty(i, a, k)?);
        }
        Ok(Some(string_value(i, &out)))
    });
    // The object overloads are what `"a" + 1` compiles to.
    interp.register_native("System.String::Concat(object,object)", |i, a| {
        let x = arg(i, a, 0)?;
        let y = arg(i, a, 1)?;
        let s = format!("{}{}", display(i, &x), display(i, &y));
        Ok(Some(string_value(i, &s)))
    });
    interp.register_native("System.String::Concat(object,object,object)", |i, a| {
        let mut out = String::new();
        for k in 0..3 {
            let v = arg(i, a, k)?;
            out.push_str(&display(i, &v));
        }
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.String::Concat(string[])", |i, a| {
        let array = arg_handle(i, a, 0)?;
        let parts = array_values(i, array);
        let mut out = String::new();
        for p in parts {
            out.push_str(&display(i, &p));
        }
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.String::Concat(object[])", |i, a| {
        let array = arg_handle(i, a, 0)?;
        let parts = array_values(i, array);
        let mut out = String::new();
        for p in parts {
            out.push_str(&display(i, &p));
        }
        Ok(Some(string_value(i, &out)))
    });

    interp.register_native("System.String::Substring(int)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let start = arg_i32(i, a, 1)?;
        let units: Vec<u16> = s.encode_utf16().collect();
        if start < 0 || start as usize > units.len() {
            return Err(out_of_range("startIndex"));
        }
        let out = String::from_utf16_lossy(&units[start as usize..]);
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.String::Substring(int,int)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let start = arg_i32(i, a, 1)?;
        let len = arg_i32(i, a, 2)?;
        let units: Vec<u16> = s.encode_utf16().collect();
        if start < 0 || len < 0 || (start as usize).saturating_add(len as usize) > units.len() {
            return Err(out_of_range("length"));
        }
        let out = String::from_utf16_lossy(&units[start as usize..start as usize + len as usize]);
        Ok(Some(string_value(i, &out)))
    });

    interp.register_native("System.String::IndexOf(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let needle = arg_string_or_empty(i, a, 1)?;
        Ok(Some(Value::I32(utf16_index_of(&s, &needle))))
    });
    interp.register_native("System.String::IndexOf(char)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let c = char_from(arg_i32(i, a, 1)?);
        Ok(Some(Value::I32(utf16_index_of(&s, &c))))
    });
    interp.register_native("System.String::LastIndexOf(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let needle = arg_string_or_empty(i, a, 1)?;
        let units: Vec<u16> = s.encode_utf16().collect();
        let target: Vec<u16> = needle.encode_utf16().collect();
        Ok(Some(Value::I32(match rfind_units(&units, &target) {
            Some(k) => k as i32,
            None => -1,
        })))
    });

    interp.register_native("System.String::Contains(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let needle = arg_string_or_empty(i, a, 1)?;
        Ok(Some(Value::I32(s.contains(&needle) as i32)))
    });
    interp.register_native("System.String::StartsWith(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let p = arg_string_or_empty(i, a, 1)?;
        Ok(Some(Value::I32(s.starts_with(&p) as i32)))
    });
    interp.register_native("System.String::EndsWith(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let p = arg_string_or_empty(i, a, 1)?;
        Ok(Some(Value::I32(s.ends_with(&p) as i32)))
    });

    interp.register_native("System.String::ToUpper()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(string_value(i, &s.to_uppercase())))
    });
    interp.register_native("System.String::ToLower()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(string_value(i, &s.to_lowercase())))
    });
    interp.register_native("System.String::Trim()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(string_value(i, s.trim())))
    });
    interp.register_native("System.String::TrimStart()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(string_value(i, s.trim_start())))
    });
    interp.register_native("System.String::TrimEnd()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(string_value(i, s.trim_end())))
    });
    interp.register_native("System.String::Replace(string,string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let from = arg_string_or_empty(i, a, 1)?;
        let to = arg_string_or_empty(i, a, 2)?;
        if from.is_empty() {
            return Ok(Some(string_value(i, &s)));
        }
        Ok(Some(string_value(i, &s.replace(&from, &to))))
    });
    interp.register_native("System.String::Replace(char,char)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let from = char_from(arg_i32(i, a, 1)?);
        let to = char_from(arg_i32(i, a, 2)?);
        Ok(Some(string_value(i, &s.replace(&from, &to))))
    });
    interp.register_native("System.String::PadLeft(int)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let width = arg_i32(i, a, 1)?.max(0) as usize;
        let len = s.encode_utf16().count();
        let out = if len >= width { s } else { format!("{}{s}", " ".repeat(width - len)) };
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.String::PadRight(int)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let width = arg_i32(i, a, 1)?.max(0) as usize;
        let len = s.encode_utf16().count();
        let out = if len >= width { s } else { format!("{s}{}", " ".repeat(width - len)) };
        Ok(Some(string_value(i, &out)))
    });

    interp.register_native("System.String::IsNullOrEmpty(string)", |i, a| {
        let v = arg(i, a, 0)?;
        let empty = match &v {
            Value::Obj(h) => i.string_value(*h).map(|s| s.is_empty()).unwrap_or(true),
            _ => true,
        };
        Ok(Some(Value::I32(empty as i32)))
    });
    interp.register_native("System.String::IsNullOrWhiteSpace(string)", |i, a| {
        let v = arg(i, a, 0)?;
        let blank = match &v {
            Value::Obj(h) => i.string_value(*h).map(|s| s.trim().is_empty()).unwrap_or(true),
            _ => true,
        };
        Ok(Some(Value::I32(blank as i32)))
    });

    interp.register_native("System.String::Equals(string)", |i, a| {
        let x = arg_string_or_empty(i, a, 0)?;
        let y = arg_string_or_empty(i, a, 1)?;
        Ok(Some(Value::I32((x == y) as i32)))
    });
    interp.register_native("System.String::Equals(string,string)", |i, a| {
        let x = arg_string_or_empty(i, a, 0)?;
        let y = arg_string_or_empty(i, a, 1)?;
        Ok(Some(Value::I32((x == y) as i32)))
    });
    interp.register_native("System.String::op_Equality(string,string)", |i, a| {
        let x = arg(i, a, 0)?;
        let y = arg(i, a, 1)?;
        Ok(Some(Value::I32(strings_equal(i, &x, &y) as i32)))
    });
    interp.register_native("System.String::op_Inequality(string,string)", |i, a| {
        let x = arg(i, a, 0)?;
        let y = arg(i, a, 1)?;
        Ok(Some(Value::I32(!strings_equal(i, &x, &y) as i32)))
    });
    // `CompareOrdinal` is what a comparison lambda usually reaches for, and
    // ordinal is what this runtime does anyway: `Compare` here is already a
    // byte-order comparison with no culture behind it, so the two share an
    // implementation and the name records which semantics were asked for.
    interp.register_native("System.String::CompareOrdinal(string,string)", compare_ordinal);
    interp.register_native("System.String::CompareOrdinal/2", compare_ordinal);
    interp.register_native("System.String::CompareTo(string)", compare_ordinal);
    interp.register_native("System.String::Compare(string,string)", compare_ordinal);
    interp.register_native("System.String::Compare/2", compare_ordinal);
    interp.register_native("System.String::GetHashCode()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(Value::I32(string_hash(&s))))
    });
    interp.register_native("System.String::ToString()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        Ok(Some(Value::Obj(h)))
    });

    // `Split` binds through arity as well as through the array key.
    //
    // `"a b".Split(' ')` looks like a one-argument call and is not: C# resolves
    // it to `Split(char, StringSplitOptions)` and passes the default, so the
    // typed key carries a token for `StringSplitOptions` that means nothing
    // outside the calling assembly. Binding `Split/2` and `Split/1` as well is
    // what makes the ordinary spelling work — without it a program that splits
    // a string fails with "no implementation" while `Split(char[])` sits
    // registered and unreachable.
    interp.register_native("System.String::Split(char[])", split_string);
    interp.register_native("System.String::Split/1", split_string);
    interp.register_native("System.String::Split/2", split_string);
    interp.register_native("System.String::Split/3", split_string);
    interp.register_native("System.String::ToCharArray()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let array = char_array(i, &s);
        Ok(Some(Value::Obj(array)))
    });
    // `Join` has an array overload and an `IEnumerable<T>` one. The latter's
    // typed key embeds metadata tokens, so both bind through the arity key and
    // the source is read with whatever reader fits it.
    interp.register_native("System.String::Join(string,string[])", join_sequence);
    interp.register_native("System.String::Join/2", join_sequence);

    interp.register_native("System.String::Format(string,object)", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let v = arg(i, a, 1)?;
        let rendered = vec![display(i, &v)];
        Ok(Some(string_value(i, &format_composite(&fmt, &rendered))))
    });
    interp.register_native("System.String::Format(string,object,object)", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let mut rendered = Vec::new();
        for k in 1..3 {
            let v = arg(i, a, k)?;
            rendered.push(display(i, &v));
        }
        Ok(Some(string_value(i, &format_composite(&fmt, &rendered))))
    });
    interp.register_native("System.String::Format(string,object,object,object)", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let mut rendered = Vec::new();
        for k in 1..4 {
            let v = arg(i, a, k)?;
            rendered.push(display(i, &v));
        }
        Ok(Some(string_value(i, &format_composite(&fmt, &rendered))))
    });
    interp.register_native("System.String::Format(string,object[])", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let array = arg_handle(i, a, 1)?;
        let rendered: Vec<String> = array_values(i, array)
            .iter()
            .map(|v| {
                let v = v.clone();
                display(i, &v)
            })
            .collect();
        Ok(Some(string_value(i, &format_composite(&fmt, &rendered))))
    });
    interp.register_native("System.String::Intern(string)", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(Value::Obj(i.intern(&s))))
    });

    // -- Char ------------------------------------------------------------------
    interp.register_native("System.Char::ToString()", |i, a| {
        let c = arg_i32(i, a, 0)?;
        Ok(Some(string_value(i, &char_from(c))))
    });
    interp.register_native("System.Char::IsDigit(char)", |i, a| {
        let c = arg_i32(i, a, 0)?;
        let is = char::from_u32(c as u32).is_some_and(|c| c.is_ascii_digit());
        Ok(Some(Value::I32(is as i32)))
    });
    interp.register_native("System.Char::IsLetter(char)", |i, a| {
        let c = arg_i32(i, a, 0)?;
        let is = char::from_u32(c as u32).is_some_and(|c| c.is_alphabetic());
        Ok(Some(Value::I32(is as i32)))
    });
    interp.register_native("System.Char::IsWhiteSpace(char)", |i, a| {
        let c = arg_i32(i, a, 0)?;
        let is = char::from_u32(c as u32).is_some_and(|c| c.is_whitespace());
        Ok(Some(Value::I32(is as i32)))
    });
    interp.register_native("System.Char::ToUpper(char)", |i, a| {
        let c = arg_i32(i, a, 0)?;
        let up = char::from_u32(c as u32)
            .and_then(|c| c.to_uppercase().next())
            .map(|c| c as i32)
            .unwrap_or(c);
        Ok(Some(Value::I32(up)))
    });
    interp.register_native("System.Char::ToLower(char)", |i, a| {
        let c = arg_i32(i, a, 0)?;
        let lo = char::from_u32(c as u32)
            .and_then(|c| c.to_lowercase().next())
            .map(|c| c as i32)
            .unwrap_or(c);
        Ok(Some(Value::I32(lo)))
    });

    register_span_concat(interp);
    register_span_concat_calls(interp);

    // The invariant forms. Rust's `to_uppercase` is already invariant — it
    // does not consult a locale — so these are the same implementation under a
    // different name rather than a second one. Registering them matters
    // because `char.ToUpperInvariant` is what culture-correct C# actually
    // calls, and a program that used it failed with "no implementation"
    // despite `ToUpper` being right there.
    interp.register_native("System.Char::ToUpperInvariant(char)", |i, a| {
        let c = arg_i32(i, a, 0)?;
        let up = char::from_u32(c as u32)
            .and_then(|c| c.to_uppercase().next())
            .map(|c| c as i32)
            .unwrap_or(c);
        Ok(Some(Value::I32(up)))
    });
    interp.register_native("System.Char::ToLowerInvariant(char)", |i, a| {
        let c = arg_i32(i, a, 0)?;
        let lo = char::from_u32(c as u32)
            .and_then(|c| c.to_lowercase().next())
            .map(|c| c as i32)
            .unwrap_or(c);
        Ok(Some(Value::I32(lo)))
    });

    register_string_builder(interp);
}

/// `StringBuilder` is backed by a managed object whose single field holds a
/// managed string; that keeps its contents visible to the collector without a
/// bespoke heap object kind.
/// Ordinal string comparison, returning a sign.
///
/// Ordinal rather than culture-aware, which is the only kind this runtime does:
/// there is no culture data here, and a comparison that claimed to be
/// culture-aware while being ordinal would be wrong in a way nothing would
/// notice until it shipped somewhere with a different collation.
fn compare_ordinal(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let x = arg_string_or_empty(i, a, 0)?;
    let y = arg_string_or_empty(i, a, 1)?;
    Ok(Some(Value::I32(match x.cmp(&y) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    })))
}

/// `string.Split(...)`, however the call was spelled.
///
/// The second argument is a separator — a `char`, or an array of them — or a
/// `StringSplitOptions` the caller never wrote. Splitting on whitespace when it
/// is neither matches `Split(null)`, which is what a bare `Split()` means.
fn split_string(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let s = arg_string_or_empty(i, a, 0)?;

    let mut separators: Vec<char> = Vec::new();
    if let Some(second) = a.get(1) {
        match second {
            // `Split('x')` — a char arrives as an integer.
            Value::I32(c) => {
                if let Some(c) = char::from_u32(*c as u32) {
                    separators.push(c);
                }
            }
            // `Split(new[] { 'x', 'y' })`.
            Value::Obj(handle) => {
                for value in array_values(i, *handle) {
                    if let Some(c) = value.as_i32().and_then(|c| char::from_u32(c as u32)) {
                        separators.push(c);
                    }
                }
            }
            _ => {}
        }
    }

    let parts: Vec<String> = if separators.is_empty() {
        s.split_whitespace().map(str::to_string).collect()
    } else {
        s.split(|c| separators.contains(&c)).map(str::to_string).collect()
    };
    let array = string_array(i, &parts);
    Ok(Some(Value::Obj(array)))
}

/// `string.Join(separator, values)` over an array or any enumerable.
fn join_sequence(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let separator = arg_string_or_empty(interp, args, 0)?;
    let source = arg(interp, args, 1)?;
    let values = crate::collections::sequence_values(interp, &source);
    let parts: Vec<String> = values
        .into_iter()
        .map(|v| display(interp, &v))
        .collect();
    Ok(Some(string_value(interp, &parts.join(&separator))))
}

fn register_string_builder(interp: &mut Interpreter) {
    interp.register_native("System.Text.StringBuilder::.ctor()", |i, a| {
        set_builder(i, a, String::new())?;
        Ok(None)
    });
    interp.register_native("System.Text.StringBuilder::.ctor(string)", |i, a| {
        let initial = arg_string_or_empty(i, a, 1)?;
        set_builder(i, a, initial)?;
        Ok(None)
    });
    interp.register_native("System.Text.StringBuilder::.ctor(int)", |i, a| {
        set_builder(i, a, String::new())?;
        Ok(None)
    });

    interp.register_native("System.Text.StringBuilder::Append(string)", |i, a| {
        let addition = arg_string_or_empty(i, a, 1)?;
        append_builder(i, a, &addition)
    });
    interp.register_native("System.Text.StringBuilder::Append(object)", |i, a| {
        let v = arg(i, a, 1)?;
        let addition = display(i, &v);
        append_builder(i, a, &addition)
    });
    interp.register_native("System.Text.StringBuilder::Append(int)", |i, a| {
        let v = arg_i32(i, a, 1)?;
        append_builder(i, a, &v.to_string())
    });
    interp.register_native("System.Text.StringBuilder::Append(char)", |i, a| {
        let v = arg_i32(i, a, 1)?;
        append_builder(i, a, &char_from(v))
    });
    interp.register_native("System.Text.StringBuilder::AppendLine(string)", |i, a| {
        let addition = arg_string_or_empty(i, a, 1)?;
        append_builder(i, a, &format!("{addition}\n"))
    });
    interp.register_native("System.Text.StringBuilder::AppendLine()", |i, a| {
        append_builder(i, a, "\n")
    });
    interp.register_native("System.Text.StringBuilder::ToString()", |i, a| {
        let current = get_builder(i, a)?;
        Ok(Some(string_value(i, &current)))
    });
    interp.register_native("System.Text.StringBuilder::get_Length()", |i, a| {
        let current = get_builder(i, a)?;
        Ok(Some(Value::I32(current.encode_utf16().count() as i32)))
    });
    interp.register_native("System.Text.StringBuilder::Clear()", |i, a| {
        set_builder(i, a, String::new())?;
        let this = arg_handle(i, a, 0)?;
        Ok(Some(Value::Obj(this)))
    });
}

fn builder_handle(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Handle> {
    arg_handle(interp, args, 0)
}

fn get_builder(interp: &mut Interpreter, args: &[Value]) -> ExecResult<String> {
    let this = builder_handle(interp, args)?;
    let inner = interp
        .heap.with::<ClrObject, _>(this, |o| o.fields.first().cloned()).flatten();
    Ok(match inner {
        Some(Value::Obj(h)) => read_string(interp, h),
        _ => String::new(),
    })
}

fn set_builder(interp: &mut Interpreter, args: &[Value], text: String) -> ExecResult<()> {
    let this = builder_handle(interp, args)?;
    let handle = interp.alloc_string(&text);
    interp.heap.with_mut::<ClrObject, _>(this, |o| {
        if o.fields.is_empty() {
            o.fields.push(Value::Obj(handle));
        } else {
            o.fields[0] = Value::Obj(handle);
        }
    });
    Ok(())
}

fn append_builder(
    interp: &mut Interpreter,
    args: &[Value],
    addition: &str,
) -> ExecResult<Option<Value>> {
    let mut current = get_builder(interp, args)?;
    current.push_str(addition);
    set_builder(interp, args, current)?;
    // `Append` returns the builder so calls can chain.
    let this = builder_handle(interp, args)?;
    Ok(Some(Value::Obj(this)))
}

// -- helpers ----------------------------------------------------------------

fn strings_equal(interp: &mut Interpreter, a: &Value, b: &Value) -> bool {
    match (a.as_handle(), b.as_handle()) {
        (Some(x), Some(y)) if x.is_null() && y.is_null() => true,
        (Some(x), Some(y)) if x.is_null() || y.is_null() => false,
        (Some(x), Some(y)) => read_string(interp, x) == read_string(interp, y),
        _ => false,
    }
}

fn char_from(code: i32) -> String {
    char::from_u32(code as u32).map(String::from).unwrap_or_default()
}

/// UTF-16 aware `IndexOf`, since .NET indices are in code units.
fn utf16_index_of(haystack: &str, needle: &str) -> i32 {
    let h: Vec<u16> = haystack.encode_utf16().collect();
    let n: Vec<u16> = needle.encode_utf16().collect();
    if n.is_empty() {
        return 0;
    }
    if n.len() > h.len() {
        return -1;
    }
    for start in 0..=(h.len() - n.len()) {
        if h[start..start + n.len()] == n[..] {
            return start as i32;
        }
    }
    -1
}

fn rfind_units(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=(haystack.len() - needle.len()))
        .rev()
        .find(|&start| haystack[start..start + needle.len()] == needle[..])
}

/// .NET's `String.GetHashCode` is not a documented algorithm, so this uses FNV
/// -1a. It is stable within a run, which is all managed code may rely on.
fn string_hash(s: &str) -> i32 {
    let mut hash: u32 = 2166136261;
    for b in s.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash as i32
}

/// Composite formatting: `{0}`, `{1}` and `{{`/`}}` escapes.
///
/// Alignment and format specifiers (`{0,10:N2}`) are parsed and the index
/// honoured, but the specifier itself is ignored rather than misapplied.
pub fn format_composite(format: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();

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
                let index_part = spec.split([',', ':']).next().unwrap_or("");
                match index_part.trim().parse::<usize>() {
                    Ok(index) => out.push_str(args.get(index).map(String::as_str).unwrap_or("")),
                    Err(_) => {
                        out.push('{');
                        out.push_str(&spec);
                        out.push('}');
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_format_substitutes_by_index() {
        assert_eq!(
            format_composite("{0} and {1}", &["a".into(), "b".into()]),
            "a and b"
        );
    }

    #[test]
    fn composite_format_honours_braces_escapes() {
        assert_eq!(format_composite("{{0}}", &["x".into()]), "{0}");
    }

    #[test]
    fn composite_format_ignores_the_specifier_but_keeps_the_index() {
        assert_eq!(format_composite("{0,8:N2}!", &["3.5".into()]), "3.5!");
    }

    #[test]
    fn index_of_counts_utf16_units() {
        // The emoji occupies two UTF-16 units, so "b" is at index 2.
        assert_eq!(utf16_index_of("\u{1F600}b", "b"), 2);
        assert_eq!(utf16_index_of("hello", "ll"), 2);
        assert_eq!(utf16_index_of("hello", "z"), -1);
    }
}

// ── string building through ReadOnlySpan<char> ───────────────────────────────

/// The span members that ordinary string concatenation goes through.
///
/// **This is not `Span<T>` support.** It is three BCL members, and the reason
/// they are here is that without them an ordinary line of C# does not run:
///
/// ```csharp
/// text += "0123456789ABCDEF"[nibble];
/// ```
///
/// `string + char` looks like it should reach `String.Concat(string, string)`
/// and does not. Roslyn on .NET 10 lowers it to
///
/// ```text
/// ldarg.0
/// call     ReadOnlySpan<char> String::op_Implicit(string)
/// ldarg.1
/// stloc.0
/// ldloca.s 0
/// newobj   ReadOnlySpan<char>::.ctor(ref char)
/// call     string String::Concat(ReadOnlySpan<char>, ReadOnlySpan<char>)
/// ```
///
/// so a program that never mentions `Span` fails with "no implementation for
/// System.String::op_Implicit". Three templates in CodeGen hit exactly this.
///
/// # A span over characters is represented as a string
///
/// That works because of what these three members do with it: the span is
/// read-only, it is over `char`, and the only thing that consumes it is
/// `Concat`. Nothing here indexes, slices or writes through one.
///
/// It is a representation choice for *this path*, not a `ReadOnlySpan<char>`
/// implementation. `span[0]`, `span.Slice(..)` and `span.Length` are not
/// registered and still refuse — which is the intended outcome: a partial span
/// that half-works would be harder to diagnose than one that says no.
fn register_span_concat(interp: &mut Interpreter) {
    // `string` to `ReadOnlySpan<char>`: the string is the representation, so
    // the conversion is the identity.
    interp.register_native("System.String::op_Implicit(string)", |i, a| {
        Ok(Some(arg(i, a, 0)?))
    });

    // `new ReadOnlySpan<char>(ref c)` — the one-element form the lowering uses
    // for the `char` operand. Argument 0 is the span being constructed,
    // argument 1 the managed pointer to the character.
    // Both the typed key and the arity key: the typed one is what the
    // `MemberRef` produces (`!0&` — a managed pointer to the element type),
    // and the arity key catches the spellings that do not.
    for key in [
        "System.ReadOnlySpan`1::.ctor(!0&)",
        "System.ReadOnlySpan`1::.ctor/2",
        "System.ReadOnlySpan`1::.ctor/1",
        "System.Span`1::.ctor(!0&)",
        "System.Span`1::.ctor/2",
    ] {
        interp.register_native(key, span_ctor);
    }
}

/// `new ReadOnlySpan<char>(ref c)`.
fn span_ctor(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    // `new Span<T>(void*, int)` — what `Span<int> s = stackalloc int[4]`
    // compiles to. The buffer exists and the length is given; the width of one
    // element lives only in `T`, and a framework generic has no runtime type
    // per construction to ask. The *call site* spells it out, though, and the
    // loader now records it — see `Loader::member_ref_type_args`.
    if matches!(a.get(1), Some(Value::Ptr(_))) {
        return crate::spans::raw_span(i, a);
    }
    {
        let text = match a.get(1) {
            Some(Value::Ref(target)) => {
                let value = i.load_indirect_public(target.clone())?;
                char_to_string(value)
            }
            // Already a string: a span over one, which `op_Implicit` produced.
            Some(other) => read_span(i, other.clone()),
            None => String::new(),
        };
        let value = string_value(i, &text);
        match a.first() {
            Some(Value::Ref(target)) => {
                let target = target.clone();
                i.store_indirect_public(target, value)?;
                Ok(None)
            }
            _ => Ok(Some(value)),
        }
    }
}

/// The concatenations Roslyn emits for span-based string building.
fn register_span_concat_calls(interp: &mut Interpreter) {
    // The concatenations Roslyn emits. Two spans covers `s + c`, `c + s` and
    // `s + s`; three and four cover the longer chains it folds into one call.
    for arity in 2..=4 {
        let key: &'static str =
            Box::leak(format!("System.String::Concat/{arity}").into_boxed_str());
        interp.register_native(key, |i, a| {
            let mut text = String::new();
            for value in a {
                text.push_str(&read_span(i, value.clone()));
            }
            Ok(Some(string_value(i, &text)))
        });
    }
}

/// A `char` as a one-character string.
///
/// Characters arrive as integers, which is how the interpreter carries them.
fn char_to_string(value: Value) -> String {
    match value.as_i32().and_then(|c| char::from_u32(c as u32)) {
        Some(c) => c.to_string(),
        None => String::new(),
    }
}

/// The text a span-or-string argument stands for.
fn read_span(interp: &mut Interpreter, value: Value) -> String {
    match &value {
        // A bare integer here is a `char` that never needed a span.
        Value::I32(_) => char_to_string(value),
        _ => match value.as_handle() {
            Some(h) if !h.is_null() => interp.string_value(h).unwrap_or_default(),
            _ => String::new(),
        },
    }
}
