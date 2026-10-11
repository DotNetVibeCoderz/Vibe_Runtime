//! `System.Text.RegularExpressions` over [`crate::regex_engine`].
//!
//! Objects keep their state in fixed slots:
//!
//! * `Regex`: pattern, options.
//! * `Capture` / `Group`: input, index, length, success, name.
//! * `Match`: the group slots, then the groups array (group 0 is the match
//!   itself, as in .NET), the regex, and where the next match starts.
//! * `GroupCollection` / `MatchCollection`: an array of their items.
//!
//! Compiled patterns are cached per (pattern, options), so the static helpers
//! and a `Regex` built in a loop compile once.

use rustclr_core::{ClrExceptionKind, ClrObject, ClrString, ExecResult, ExecutionError, Interpreter, TypeId, Value};
use rustclr_gc::Handle;

use crate::collections::{elements, field, field_handle, set_field, with_values};
use crate::regex_engine::{self as engine, Captures, RegexError};
use crate::support::*;

#[allow(unused_imports)]
use crate::prelude::*;

const NS: &str = "System.Text.RegularExpressions";

const INPUT: usize = 0;
const INDEX: usize = 1;
const LENGTH: usize = 2;
const SUCCESS: usize = 3;
const NAME: usize = 4;
const GROUPS: usize = 5;
const REGEX: usize = 6;
const NEXT: usize = 7;

type Cache = BTreeMap<(String, i32), engine::Regex>;
use alloc::collections::BTreeMap;

fn k(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn key(ty: &str, member: &str) -> &'static str {
    k(format!("{NS}.{ty}::{member}"))
}

fn ty(i: &Interpreter, name: &str) -> TypeId {
    i.loader.registry.find_type_by_name(&format!("{NS}.{name}")).unwrap_or(i.loader.core().object)
}

fn units_of(i: &Interpreter, v: &Value) -> Option<Vec<u16>> {
    match v {
        Value::Obj(h) if !h.is_null() => i.heap.with::<ClrString, _>(*h, |s| s.units.clone()),
        _ => None,
    }
}

fn required_units(i: &mut Interpreter, a: &[Value], index: usize, name: &str) -> ExecResult<Vec<u16>> {
    let v = arg(i, a, index)?;
    units_of(i, &v).ok_or_else(|| ExecutionError::exception(ClrExceptionKind::ArgumentNull, &format!("Value cannot be null. (Parameter '{name}')")))
}

fn error(e: RegexError) -> ExecutionError {
    match e {
        RegexError::Parse(message) => ExecutionError::exception(ClrExceptionKind::Argument, &message),
        RegexError::Timeout => ExecutionError::exception(
            ClrExceptionKind::InvalidOperation,
            "The regular expression took too long: it backtracked more than 50 million steps. Simplify nested quantifiers such as (a+)+.",
        ),
    }
}

/// A compiled pattern, from the cache.
fn compile(i: &mut Interpreter, pattern: &str, options: i32) -> ExecResult<engine::Regex> {
    let cache = i.native_state.entry("regex").or_insert_with(|| Box::new(Cache::new()));
    let cache = cache.downcast_mut::<Cache>().expect("regex cache");
    let id = (pattern.to_string(), options);
    if let Some(re) = cache.get(&id) {
        return Ok(re.clone());
    }
    let re = engine::Regex::new(pattern, options).map_err(error)?;
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(id, re.clone());
    Ok(re)
}

/// The compiled pattern a `Regex` object holds.
fn regex_of(i: &mut Interpreter, this: Handle) -> ExecResult<engine::Regex> {
    let pattern = match field(i, this, 0) {
        Value::Obj(h) if !h.is_null() => i.string_value(h).unwrap_or_default(),
        _ => return Err(ExecutionError::null_reference()),
    };
    let options = field(i, this, 1).as_i32().unwrap_or(0);
    compile(i, &pattern, options)
}

fn new_regex_object(i: &mut Interpreter, pattern: &str, options: i32) -> ExecResult<Handle> {
    compile(i, pattern, options)?;
    let p = i.alloc_string(pattern);
    let obj = ClrObject { type_id: ty(i, "Regex"), fields: vec![Value::Obj(p), Value::I32(options)], monitor: None };
    Ok(i.alloc_clr_object(obj))
}

/// Builds a `Match` (and its groups) from captures, or `Match.Empty`'s shape.
fn new_match(i: &mut Interpreter, re: &engine::Regex, regex_obj: Handle, input: Handle, caps: Option<&Captures>) -> Handle {
    let (index, length, success) = match caps.and_then(|c| c[0]) {
        Some((s, e)) => (s as i32, (e - s) as i32, true),
        None => (0, 0, false),
    };
    let next = match caps.and_then(|c| c[0]) {
        Some((s, e)) if s == e => e as i32 + 1,
        Some((_, e)) => e as i32,
        None => 0,
    };
    let name = i.alloc_string("0");
    let m = i.alloc_clr_object(ClrObject {
        type_id: ty(i, "Match"),
        fields: vec![
            Value::Obj(input),
            Value::I32(index),
            Value::I32(length),
            Value::I32(success as i32),
            Value::Obj(name),
            Value::Null,
            Value::Obj(regex_obj),
            Value::I32(next),
        ],
        monitor: None,
    });
    let group_type = ty(i, "Group");
    let mut groups = vec![Value::Obj(m)];
    for n in 1..re.group_count {
        let span = caps.and_then(|c| c.get(n).copied().flatten()).filter(|_| success);
        let (gi, gl, ok) = match span {
            Some((s, e)) => (s as i32, (e - s) as i32, true),
            None => (0, 0, false),
        };
        let gname = i.alloc_string(&re.names[n]);
        let g = i.alloc_clr_object(ClrObject {
            type_id: group_type,
            fields: vec![Value::Obj(input), Value::I32(gi), Value::I32(gl), Value::I32(ok as i32), Value::Obj(gname)],
            monitor: None,
        });
        groups.push(Value::Obj(g));
    }
    let array = i.alloc_value_array(0);
    with_values(i, array, |v| *v = groups);
    let collection = i.alloc_clr_object(ClrObject { type_id: ty(i, "GroupCollection"), fields: vec![Value::Obj(array)], monitor: None });
    set_field(i, m, GROUPS, Value::Obj(collection));
    m
}

fn collection(i: &mut Interpreter, type_name: &str, items: Vec<Value>) -> Value {
    let array = i.alloc_value_array(0);
    with_values(i, array, |v| *v = items);
    let obj = i.alloc_clr_object(ClrObject { type_id: ty(i, type_name), fields: vec![Value::Obj(array)], monitor: None });
    Value::Obj(obj)
}

/// The text a capture covers.
fn capture_value(i: &mut Interpreter, this: Handle) -> Value {
    let input = field_handle(i, this, INPUT);
    let index = field(i, this, INDEX).as_i32().unwrap_or(0).max(0) as usize;
    let length = field(i, this, LENGTH).as_i32().unwrap_or(0).max(0) as usize;
    let units = i
        .heap
        .with::<ClrString, _>(input, |s| s.units.get(index..index + length).map(|u| u.to_vec()))
        .flatten()
        .unwrap_or_default();
    Value::Obj(i.alloc_clr_string(ClrString { units }))
}

/// Who a call is: an instance call on a `Regex`, or the static helper taking
/// the pattern as its second argument. Returns the regex, its object, and the
/// index of the first argument after the input and (for statics) pattern.
fn resolve(i: &mut Interpreter, a: &[Value]) -> ExecResult<(engine::Regex, Handle, usize, bool)> {
    let has_this = i
        .current_native_method()
        .map(|m| i.loader.registry.method(m).signature.has_this)
        .unwrap_or(false);
    if has_this {
        let this = arg_handle(i, a, 0)?;
        let re = regex_of(i, this)?;
        Ok((re, this, 1, true))
    } else {
        // Static: (input, pattern, ...). Options, when given, follow the
        // replacement or come third.
        let pattern = String::from_utf16_lossy(&required_units(i, a, 1, "pattern")?);
        let options = static_options(i, a);
        let obj = new_regex_object(i, &pattern, options)?;
        let re = compile(i, &pattern, options)?;
        Ok((re, obj, 0, false))
    }
}

/// The `RegexOptions` argument of a static call, found by its declared type.
fn static_options(i: &mut Interpreter, a: &[Value]) -> i32 {
    let Some(m) = i.current_native_method() else { return 0 };
    let count = i.loader.registry.method(m).signature.params.len();
    for n in 0..count {
        if crate::extras::param_type_name(i, n).as_deref() == Some("System.Text.RegularExpressions.RegexOptions") {
            return a.get(n).and_then(|v| v.as_i32()).unwrap_or(0);
        }
    }
    0
}

fn param_is(i: &Interpreter, index: usize, name: &str) -> bool {
    crate::extras::param_type_name(i, index).as_deref() == Some(name)
}

fn string_arg(i: &mut Interpreter, a: &[Value], index: usize) -> ExecResult<String> {
    arg_string_or_empty(i, a, index)
}

pub fn register(interp: &mut Interpreter) {
    interp.loader.register_regex_types();

    // -- Regex: construction and options ------------------------------------
    for name in [".ctor(string)", ".ctor/1", ".ctor/2", ".ctor/3"] {
        interp.register_native(key("Regex", name), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let pattern_units = required_units(i, a, 1, "pattern")?;
            let pattern = String::from_utf16_lossy(&pattern_units);
            let options = a.get(2).and_then(|v| v.as_i32()).unwrap_or(0);
            compile(i, &pattern, options)?;
            let p = i.alloc_string(&pattern);
            set_field(i, this, 0, Value::Obj(p));
            set_field(i, this, 1, Value::I32(options));
            Ok(None)
        });
    }
    interp.register_native(key("Regex", "get_Options()"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        Ok(Some(Value::I32(field(i, this, 1).as_i32().unwrap_or(0))))
    });
    interp.register_native(key("Regex", "ToString()"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        Ok(Some(field(i, this, 0)))
    });
    interp.register_native(key("Regex", "GetGroupNames()"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let re = regex_of(i, this)?;
        let names: Vec<Value> = re.names.iter().map(|n| Value::Obj(i.alloc_string(n))).collect();
        let string_type = i.loader.core().string;
        let array = i.alloc_array(string_type, names.len());
        i.heap.with_mut::<rustclr_core::ClrArray, _>(array, |arr| {
            for (n, v) in names.iter().enumerate() {
                arr.storage.set(n, v);
            }
        });
        Ok(Some(Value::Obj(array)))
    });
    interp.register_native(key("Regex", "GroupNumberFromName(string)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let re = regex_of(i, this)?;
        let name = string_arg(i, a, 1)?;
        Ok(Some(Value::I32(re.group_number(&name).map(|n| n as i32).unwrap_or(-1))))
    });
    interp.register_native(key("Regex", "Escape(string)"), |i, a| {
        let s = string_arg(i, a, 0)?;
        Ok(Some(string_value(i, &engine::escape(&s))))
    });
    interp.register_native(key("Regex", "Unescape(string)"), |i, a| {
        let s = string_arg(i, a, 0)?;
        let out = engine::unescape(&s).map_err(error)?;
        Ok(Some(string_value(i, &out)))
    });

    // -- IsMatch / Match / Matches -------------------------------------------
    for name in ["IsMatch/1", "IsMatch/2", "IsMatch/3", "IsMatch/4"] {
        interp.register_native(key("Regex", name), |i, a| {
            let (re, _, first, has_this) = resolve(i, a)?;
            let input = required_units(i, a, first, "input")?;
            // Instance IsMatch(input, startat).
            let start = if has_this && a.len() > 2 && !param_is(i, 1, "System.Text.RegularExpressions.RegexOptions") {
                a[2].as_i32().unwrap_or(0).max(0) as usize
            } else {
                0
            };
            let found = re.find_at(&input, start.min(input.len()), start).map_err(error)?;
            Ok(Some(Value::I32(found.is_some() as i32)))
        });
    }
    for name in ["Match/1", "Match/2", "Match/3", "Match/4"] {
        interp.register_native(key("Regex", name), |i, a| {
            let (re, obj, first, has_this) = resolve(i, a)?;
            let input_value = arg(i, a, first)?;
            let input = units_of(i, &input_value).ok_or_else(|| ExecutionError::exception(ClrExceptionKind::ArgumentNull, "Value cannot be null. (Parameter 'input')"))?;
            let start = if has_this && a.len() > 2 { a[2].as_i32().unwrap_or(0).max(0) as usize } else { 0 };
            if start > input.len() {
                return Err(ExecutionError::exception(ClrExceptionKind::ArgumentOutOfRange, "Specified argument was out of the range of valid values. (Parameter 'startat')"));
            }
            let caps = re.find_at(&input, start, start).map_err(error)?;
            let input_handle = input_value.as_handle().unwrap_or(Handle::NULL);
            Ok(Some(Value::Obj(new_match(i, &re, obj, input_handle, caps.as_ref()))))
        });
    }
    for name in ["Matches/1", "Matches/2", "Matches/3", "Matches/4"] {
        interp.register_native(key("Regex", name), |i, a| {
            let (re, obj, first, has_this) = resolve(i, a)?;
            let input_value = arg(i, a, first)?;
            let input = units_of(i, &input_value).ok_or_else(|| ExecutionError::exception(ClrExceptionKind::ArgumentNull, "Value cannot be null. (Parameter 'input')"))?;
            let start = if has_this && a.len() > 2 { a[2].as_i32().unwrap_or(0).max(0) as usize } else { 0 };
            let all = re.find_all(&input, start.min(input.len())).map_err(error)?;
            let input_handle = input_value.as_handle().unwrap_or(Handle::NULL);
            let matches: Vec<Value> = all.iter().map(|c| Value::Obj(new_match(i, &re, obj, input_handle, Some(c)))).collect();
            Ok(Some(collection(i, "MatchCollection", matches)))
        });
    }

    // -- Replace ------------------------------------------------------------
    for name in ["Replace/2", "Replace/3", "Replace/4", "Replace/5"] {
        interp.register_native(key("Regex", name), replace);
    }

    // -- Split ----------------------------------------------------------------
    for name in ["Split/1", "Split/2", "Split/3", "Split/4"] {
        interp.register_native(key("Regex", name), |i, a| {
            let (re, _, first, has_this) = resolve(i, a)?;
            let input = required_units(i, a, first, "input")?;
            let count = if has_this && a.len() > 2 { a[2].as_i32().unwrap_or(0) } else { 0 };
            let mut all = re.find_all(&input, 0).map_err(error)?;
            if count > 0 {
                all.truncate((count - 1).max(0) as usize);
            }
            let mut pieces: Vec<Vec<u16>> = Vec::new();
            let mut at = 0;
            for caps in &all {
                let (s, e) = caps[0].unwrap_or((at, at));
                pieces.push(input[at..s].to_vec());
                // Captured groups are included, as in .NET.
                for g in caps.iter().skip(1).flatten() {
                    pieces.push(input[g.0..g.1].to_vec());
                }
                at = e;
            }
            pieces.push(input[at..].to_vec());
            let values: Vec<Value> = pieces.into_iter().map(|u| Value::Obj(i.alloc_clr_string(ClrString { units: u }))).collect();
            let string_type = i.loader.core().string;
            let array = i.alloc_array(string_type, values.len());
            i.heap.with_mut::<rustclr_core::ClrArray, _>(array, |arr| {
                for (n, v) in values.iter().enumerate() {
                    arr.storage.set(n, v);
                }
            });
            Ok(Some(Value::Obj(array)))
        });
    }

    // -- Capture, Group, Match ------------------------------------------------
    for owner in ["Capture", "Group", "Match"] {
        interp.register_native(key(owner, "get_Index()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(Value::I32(field(i, this, INDEX).as_i32().unwrap_or(0))))
        });
        interp.register_native(key(owner, "get_Length()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(Value::I32(field(i, this, LENGTH).as_i32().unwrap_or(0))))
        });
        interp.register_native(key(owner, "get_Value()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(capture_value(i, this)))
        });
        interp.register_native(key(owner, "ToString()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(capture_value(i, this)))
        });
        interp.register_native(key(owner, "get_Success()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(Value::I32(field(i, this, SUCCESS).as_i32().unwrap_or(0))))
        });
        interp.register_native(key(owner, "get_Name()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(field(i, this, NAME)))
        });
        // Captures: the last capture only, which is what most code reads.
        interp.register_native(key(owner, "get_Captures()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let items = if field(i, this, SUCCESS).as_i32().unwrap_or(0) != 0 { vec![Value::Obj(this)] } else { Vec::new() };
            Ok(Some(collection(i, "MatchCollection", items)))
        });
    }
    interp.register_native(key("Match", "get_Groups()"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        Ok(Some(field(i, this, GROUPS)))
    });
    interp.register_native(key("Match", "NextMatch()"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let input = field_handle(i, this, INPUT);
        let regex_obj = field_handle(i, this, REGEX);
        if field(i, this, SUCCESS).as_i32().unwrap_or(0) == 0 || regex_obj.is_null() {
            return Ok(Some(Value::Obj(this)));
        }
        let re = regex_of(i, regex_obj)?;
        let start = field(i, this, NEXT).as_i32().unwrap_or(0).max(0) as usize;
        let prev = (field(i, this, INDEX).as_i32().unwrap_or(0) + field(i, this, LENGTH).as_i32().unwrap_or(0)).max(0) as usize;
        let units = i.heap.with::<ClrString, _>(input, |s| s.units.clone()).unwrap_or_default();
        let caps = if start > units.len() { None } else { re.find_at(&units, start, prev).map_err(error)? };
        Ok(Some(Value::Obj(new_match(i, &re, regex_obj, input, caps.as_ref()))))
    });
    interp.register_native(key("Match", "Result(string)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let replacement = required_units(i, a, 1, "replacement")?;
        let regex_obj = field_handle(i, this, REGEX);
        let re = regex_of(i, regex_obj)?;
        let input = field_handle(i, this, INPUT);
        let units = i.heap.with::<ClrString, _>(input, |s| s.units.clone()).unwrap_or_default();
        let caps = captures_of(i, this, re.group_count);
        let mut out = Vec::new();
        engine::expand(&engine::parse_replacement(&re, &replacement), &units, &caps, &mut out);
        Ok(Some(Value::Obj(i.alloc_clr_string(ClrString { units: out }))))
    });
    interp.register_native(key("Match", "get_Empty()"), |i, _| {
        let empty = i.alloc_string("");
        let re = engine::Regex::new("", 0).map_err(error)?;
        Ok(Some(Value::Obj(new_match(i, &re, Handle::NULL, empty, None))))
    });

    // -- GroupCollection and MatchCollection ----------------------------------
    for owner in ["GroupCollection", "MatchCollection"] {
        interp.register_native(key(owner, "get_Count()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            Ok(Some(Value::I32(elements(i, field_handle(i, this, 0)).len() as i32)))
        });
        interp.register_native(key(owner, "GetEnumerator()"), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let array = field_handle(i, this, 0);
            Ok(Some(crate::collections::new_enumerator(i, "List`1+Enumerator", array)))
        });
    }
    interp.register_native(key("MatchCollection", "get_Item(int)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let n = arg_i32(i, a, 1)?;
        let items = elements(i, field_handle(i, this, 0));
        items.get(n.max(0) as usize).cloned().filter(|_| n >= 0).map(Some).ok_or_else(|| {
            ExecutionError::exception(ClrExceptionKind::ArgumentOutOfRange, "Specified argument was out of the range of valid values. (Parameter 'i')")
        })
    });
    // `match.Groups[n]` or `match.Groups["name"]`: an absent group is a
    // failed group, never null, as in .NET.
    for name in ["get_Item(int)", "get_Item(string)", "get_Item/1"] {
        interp.register_native(key("GroupCollection", name), |i, a| {
            let this = arg_handle(i, a, 0)?;
            let items = elements(i, field_handle(i, this, 0));
            let by_name = matches!(arg(i, a, 1)?, Value::Obj(_));
            let found = if by_name {
                let wanted = string_arg(i, a, 1)?;
                items.iter().find(|g| {
                    g.as_handle()
                        .map(|h| matches!(field(i, h, NAME), Value::Obj(n) if i.string_value(n).as_deref() == Some(wanted.as_str())))
                        .unwrap_or(false)
                }).cloned()
            } else {
                let n = arg_i32(i, a, 1)?;
                items.get(n.max(0) as usize).cloned().filter(|_| n >= 0)
            };
            match found {
                Some(g) => Ok(Some(g)),
                None => {
                    let input = items.first().and_then(|g| g.as_handle()).map(|h| field_handle(i, h, INPUT)).unwrap_or(Handle::NULL);
                    let name = i.alloc_string("");
                    let g = i.alloc_clr_object(ClrObject {
                        type_id: ty(i, "Group"),
                        fields: vec![Value::Obj(input), Value::I32(0), Value::I32(0), Value::I32(0), Value::Obj(name)],
                        monitor: None,
                    });
                    Ok(Some(Value::Obj(g)))
                }
            }
        });
    }
    interp.register_native(key("GroupCollection", "ContainsKey(string)"), |i, a| {
        let this = arg_handle(i, a, 0)?;
        let wanted = string_arg(i, a, 1)?;
        let items = elements(i, field_handle(i, this, 0));
        let hit = items.iter().any(|g| {
            g.as_handle()
                .map(|h| matches!(field(i, h, NAME), Value::Obj(n) if i.string_value(n).as_deref() == Some(wanted.as_str())))
                .unwrap_or(false)
        });
        Ok(Some(Value::I32(hit as i32)))
    });
}

/// The spans a `Match` object records, in engine form.
fn captures_of(i: &mut Interpreter, m: Handle, count: usize) -> Captures {
    let groups = field_handle(i, m, GROUPS);
    let items = elements(i, field_handle(i, groups, 0));
    (0..count)
        .map(|n| {
            let h = items.get(n).and_then(|v| v.as_handle())?;
            if field(i, h, SUCCESS).as_i32().unwrap_or(0) == 0 {
                return None;
            }
            let s = field(i, h, INDEX).as_i32().unwrap_or(0) as usize;
            let l = field(i, h, LENGTH).as_i32().unwrap_or(0) as usize;
            Some((s, s + l))
        })
        .collect()
}

/// Every `Replace` overload: instance or static, replacement text or a
/// `MatchEvaluator`, optional count and start.
fn replace(i: &mut Interpreter, a: &[Value]) -> ExecResult<Option<Value>> {
    let (re, obj, first, has_this) = resolve(i, a)?;
    let input_value = arg(i, a, first)?;
    let input = units_of(i, &input_value).ok_or_else(|| ExecutionError::exception(ClrExceptionKind::ArgumentNull, "Value cannot be null. (Parameter 'input')"))?;
    // The replacement or evaluator follows the input (instance) or the
    // pattern (static).
    let what_index = if has_this { 2 } else { 2 };
    let param_index = if has_this { 1 } else { 2 };
    let evaluator = param_is(i, param_index, "System.Text.RegularExpressions.MatchEvaluator");
    let what = arg(i, a, what_index)?;
    // Instance overloads may add (count) or (count, startat).
    let (count, start) = if has_this {
        let count = a.get(3).and_then(|v| v.as_i32()).unwrap_or(-1);
        let start = a.get(4).and_then(|v| v.as_i32()).unwrap_or(0).max(0) as usize;
        (count, start)
    } else {
        (-1, 0)
    };
    let mut all = re.find_all(&input, start.min(input.len())).map_err(error)?;
    if count >= 0 {
        all.truncate(count as usize);
    }
    if all.is_empty() {
        return Ok(Some(input_value));
    }
    let pieces = if evaluator {
        None
    } else {
        let replacement = units_of(i, &what).ok_or_else(|| ExecutionError::exception(ClrExceptionKind::ArgumentNull, "Value cannot be null. (Parameter 'replacement')"))?;
        Some(engine::parse_replacement(&re, &replacement))
    };
    let input_handle = input_value.as_handle().unwrap_or(Handle::NULL);
    let mut out: Vec<u16> = Vec::with_capacity(input.len());
    let mut at = 0;
    for caps in &all {
        let (s, e) = caps[0].unwrap_or((at, at));
        out.extend_from_slice(&input[at..s]);
        match &pieces {
            Some(p) => engine::expand(p, &input, caps, &mut out),
            None => {
                let m = new_match(i, &re, obj, input_handle, Some(caps));
                let result = crate::linq::call(i, &what, &[Value::Obj(m)])?;
                if let Some(u) = units_of(i, &result) {
                    out.extend_from_slice(&u);
                }
            }
        }
        at = e;
    }
    out.extend_from_slice(&input[at..]);
    Ok(Some(Value::Obj(i.alloc_clr_string(ClrString { units: out }))))
}
