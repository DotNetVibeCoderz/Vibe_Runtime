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
            .heap
            .get_as::<ClrString>(h)
            .and_then(|s| s.char_at(index.max(0) as usize));
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
    interp.register_native("System.String::Compare(string,string)", |i, a| {
        let x = arg_string_or_empty(i, a, 0)?;
        let y = arg_string_or_empty(i, a, 1)?;
        Ok(Some(Value::I32(match x.cmp(&y) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        })))
    });
    interp.register_native("System.String::GetHashCode()", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        Ok(Some(Value::I32(string_hash(&s))))
    });
    interp.register_native("System.String::ToString()", |i, a| {
        let h = arg_handle(i, a, 0)?;
        Ok(Some(Value::Obj(h)))
    });

    interp.register_native("System.String::Split(char[])", |i, a| {
        let s = arg_string_or_empty(i, a, 0)?;
        let seps = arg_handle(i, a, 1)?;
        let separators: Vec<char> = array_values(i, seps)
            .iter()
            .filter_map(|v| v.as_i32())
            .filter_map(|c| char::from_u32(c as u32))
            .collect();
        let parts: Vec<String> = if separators.is_empty() {
            s.split_whitespace().map(str::to_string).collect()
        } else {
            s.split(|c| separators.contains(&c)).map(str::to_string).collect()
        };
        let array = string_array(i, &parts);
        Ok(Some(Value::Obj(array)))
    });
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

    register_string_builder(interp);
}

/// `StringBuilder` is backed by a managed object whose single field holds a
/// managed string; that keeps its contents visible to the collector without a
/// bespoke heap object kind.
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
        .heap
        .get_as::<ClrObject>(this)
        .and_then(|o| o.fields.first().cloned());
    Ok(match inner {
        Some(Value::Obj(h)) => read_string(interp, h),
        _ => String::new(),
    })
}

fn set_builder(interp: &mut Interpreter, args: &[Value], text: String) -> ExecResult<()> {
    let this = builder_handle(interp, args)?;
    let handle = interp.alloc_string(&text);
    if let Some(o) = interp.heap.get_as_mut::<ClrObject>(this) {
        if o.fields.is_empty() {
            o.fields.push(Value::Obj(handle));
        } else {
            o.fields[0] = Value::Obj(handle);
        }
    }
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
