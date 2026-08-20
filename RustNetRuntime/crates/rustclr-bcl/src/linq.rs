//! `System.Linq.Enumerable`.
//!
//! LINQ is what makes the generic collections worth having: almost no modern
//! C# iterates with an index once `Where` and `Select` are available. The
//! operators are ordinary static methods taking an `IEnumerable<T>` and a
//! delegate, so implementing them natively needs two things this runtime
//! already has — a way to read any sequence, and a way to call a delegate.
//!
//! # Binding
//!
//! A LINQ call site is a `MethodSpec` over a `MemberRef`, and its parameter
//! types are generic instantiations whose rendering embeds metadata tokens —
//! unstable between assemblies. So these bind on the *arity* key
//! (`Enumerable::Where/2`) rather than the typed one. Where two overloads share
//! an arity, such as `Select(source, item => …)` and
//! `Select(source, (item, index) => …)`, the delegate's own parameter count
//! picks between them at run time.
//!
//! # Evaluation is eager
//!
//! .NET's operators are lazy: `Where` returns an iterator that runs the
//! predicate only as the result is consumed. Here every operator materialises
//! a `List<T>` immediately. For the overwhelming majority of code the results
//! are identical, but three differences are real, and are stated in
//! `docs/limitations.md` rather than left to be discovered:
//!
//! - Side effects inside a predicate or selector happen at the point of the
//!   LINQ call, not at the point of consumption.
//! - An infinite sequence never terminates, where .NET would stream it.
//! - A change to the source after the call is not reflected in the result.

use crate::collections::{
    as_number, elements, field, field_handle, invalid_operation, named_type, new_list,
    sequence_values, set_field, value_hash, values_equal, with_values,
};
use crate::support::*;
use rustclr_core::{
    ClrArray, ClrDelegate, ClrString, ExecResult, ExecutionError, Interpreter, Value,
};
use rustclr_gc::Handle;

const ENUMERABLE: &str = "System.Linq.Enumerable";

/// Leaks a stable key string; the native table holds it for the process life.
fn key(member: &str) -> &'static str {
    Box::leak(format!("{ENUMERABLE}::{member}").into_boxed_str())
}

pub fn register(interp: &mut Interpreter) {
    register_filters(interp);
    register_aggregates(interp);
    register_element_access(interp);
    register_ordering(interp);
    register_grouping(interp);
    register_conversions(interp);
    register_generators(interp);
}

// -- calling back into managed code ------------------------------------------

/// Invokes a delegate with the given arguments.
///
/// This mirrors the interpreter's own `Invoke` intrinsic: a multicast delegate
/// runs every target and yields the last result, which is what a `Func` used as
/// a selector would do on .NET.
fn call(interp: &mut Interpreter, delegate: &Value, args: &[Value]) -> ExecResult<Value> {
    let handle = delegate.as_handle().filter(|h| !h.is_null()).ok_or_else(|| {
        ExecutionError::exception(
            rustclr_core::ClrExceptionKind::ArgumentNull,
            "Value cannot be null. (Parameter 'selector')",
        )
    })?;
    let targets = interp
        .heap
        .get_as::<ClrDelegate>(handle)
        .map(|d| d.targets.clone())
        .ok_or_else(ExecutionError::null_reference)?;

    let mut result = Value::Null;
    for target in targets {
        let mut call_args = Vec::with_capacity(args.len() + 1);
        if !target.receiver.is_null() {
            call_args.push(Value::Obj(target.receiver));
        }
        call_args.extend_from_slice(args);
        result = interp.invoke(target.method, call_args)?.unwrap_or(Value::Null);
    }
    Ok(result)
}

/// How many arguments a delegate's target expects, ignoring its receiver.
///
/// This is what separates `Select(source, item => …)` from
/// `Select(source, (item, index) => …)`: both are two-argument calls to
/// `Enumerable`, and only the lambda's own shape tells them apart.
fn delegate_arity(interp: &Interpreter, delegate: &Value) -> usize {
    let Some(handle) = delegate.as_handle() else { return 1 };
    let Some(d) = interp.heap.get_as::<ClrDelegate>(handle) else { return 1 };
    let Some(target) = d.targets.first() else { return 1 };
    let method = target.method;
    let info = interp.loader.registry.method(method);
    let declared = info.signature.params.len();
    // An instance target's receiver is passed separately, so it is not counted
    // here either way; a closure's display class arrives as the receiver.
    declared
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::I32(n) => *n != 0,
        Value::I64(n) | Value::NativeInt(n) => *n != 0,
        Value::Null => false,
        _ => true,
    }
}

/// The source sequence of an operator call, as a vector.
fn source(interp: &mut Interpreter, args: &[Value], index: usize) -> Vec<Value> {
    let v = args.get(index).cloned().unwrap_or(Value::Null);
    sequence_values(interp, &v)
}

fn empty_sequence(what: &str) -> ExecutionError {
    invalid_operation(&format!("Sequence contains no {what}"))
}

// -- filtering and projection ------------------------------------------------

fn register_filters(interp: &mut Interpreter) {
    interp.register_native(key("Where/2"), |i, a| {
        let items = source(i, a, 0);
        let predicate = a.get(1).cloned().unwrap_or(Value::Null);
        let indexed = delegate_arity(i, &predicate) >= 2;
        let mut out = Vec::new();
        for (index, item) in items.into_iter().enumerate() {
            let keep = if indexed {
                call(i, &predicate, &[item.clone(), Value::I32(index as i32)])?
            } else {
                call(i, &predicate, &[item.clone()])?
            };
            if truthy(&keep) {
                out.push(item);
            }
        }
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("Select/2"), |i, a| {
        let items = source(i, a, 0);
        let selector = a.get(1).cloned().unwrap_or(Value::Null);
        let indexed = delegate_arity(i, &selector) >= 2;
        let mut out = Vec::with_capacity(items.len());
        for (index, item) in items.into_iter().enumerate() {
            out.push(if indexed {
                call(i, &selector, &[item, Value::I32(index as i32)])?
            } else {
                call(i, &selector, &[item])?
            });
        }
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("SelectMany/2"), |i, a| {
        let items = source(i, a, 0);
        let selector = a.get(1).cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        for item in items {
            let inner = call(i, &selector, &[item])?;
            out.extend(sequence_values(i, &inner));
        }
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("Take/2"), |i, a| {
        let items = source(i, a, 0);
        let count = arg_i32(i, a, 1)?.max(0) as usize;
        let out = items.into_iter().take(count).collect();
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("Skip/2"), |i, a| {
        let items = source(i, a, 0);
        let count = arg_i32(i, a, 1)?.max(0) as usize;
        let out = items.into_iter().skip(count).collect();
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("TakeWhile/2"), |i, a| {
        let items = source(i, a, 0);
        let predicate = a.get(1).cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        for item in items {
            if !truthy(&call(i, &predicate, &[item.clone()])?) {
                break;
            }
            out.push(item);
        }
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("SkipWhile/2"), |i, a| {
        let items = source(i, a, 0);
        let predicate = a.get(1).cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        let mut skipping = true;
        for item in items {
            if skipping && truthy(&call(i, &predicate, &[item.clone()])?) {
                continue;
            }
            skipping = false;
            out.push(item);
        }
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("Distinct/1"), |i, a| {
        let items = source(i, a, 0);
        let mut out: Vec<Value> = Vec::new();
        for item in items {
            if !out.iter().any(|seen| values_equal(i, seen, &item)) {
                out.push(item);
            }
        }
        Ok(Some(new_list(i, out)))
    });

    interp.register_native(key("Reverse/1"), |i, a| {
        let mut items = source(i, a, 0);
        items.reverse();
        Ok(Some(new_list(i, items)))
    });

    interp.register_native(key("Concat/2"), |i, a| {
        let mut items = source(i, a, 0);
        items.extend(source(i, a, 1));
        Ok(Some(new_list(i, items)))
    });

    interp.register_native(key("Append/2"), |i, a| {
        let mut items = source(i, a, 0);
        items.push(arg(i, a, 1)?);
        Ok(Some(new_list(i, items)))
    });

    interp.register_native(key("Prepend/2"), |i, a| {
        let mut items = source(i, a, 0);
        items.insert(0, arg(i, a, 1)?);
        Ok(Some(new_list(i, items)))
    });

    interp.register_native(key("Zip/3"), |i, a| {
        let left = source(i, a, 0);
        let right = source(i, a, 1);
        let selector = a.get(2).cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        for (x, y) in left.into_iter().zip(right) {
            out.push(call(i, &selector, &[x, y])?);
        }
        Ok(Some(new_list(i, out)))
    });
}

// -- aggregation -------------------------------------------------------------

fn register_aggregates(interp: &mut Interpreter) {
    interp.register_native(key("Count/1"), |i, a| {
        Ok(Some(Value::I32(source(i, a, 0).len() as i32)))
    });
    interp.register_native(key("Count/2"), |i, a| {
        let items = source(i, a, 0);
        let predicate = a.get(1).cloned().unwrap_or(Value::Null);
        let mut n = 0;
        for item in items {
            if truthy(&call(i, &predicate, &[item])?) {
                n += 1;
            }
        }
        Ok(Some(Value::I32(n)))
    });
    interp.register_native(key("LongCount/1"), |i, a| {
        Ok(Some(Value::I64(source(i, a, 0).len() as i64)))
    });

    interp.register_native(key("Any/1"), |i, a| {
        Ok(Some(Value::I32(!source(i, a, 0).is_empty() as i32)))
    });
    interp.register_native(key("Any/2"), |i, a| {
        let items = source(i, a, 0);
        let predicate = a.get(1).cloned().unwrap_or(Value::Null);
        for item in items {
            if truthy(&call(i, &predicate, &[item])?) {
                return Ok(Some(Value::I32(1)));
            }
        }
        Ok(Some(Value::I32(0)))
    });
    interp.register_native(key("All/2"), |i, a| {
        let items = source(i, a, 0);
        let predicate = a.get(1).cloned().unwrap_or(Value::Null);
        for item in items {
            if !truthy(&call(i, &predicate, &[item])?) {
                return Ok(Some(Value::I32(0)));
            }
        }
        Ok(Some(Value::I32(1)))
    });

    interp.register_native(key("Contains/2"), |i, a| {
        let items = source(i, a, 0);
        let needle = arg(i, a, 1)?;
        Ok(Some(Value::I32(
            items.iter().any(|v| values_equal(i, v, &needle)) as i32,
        )))
    });

    interp.register_native(key("Sum/1"), |i, a| {
        let items = source(i, a, 0);
        Ok(Some(total(&items)))
    });
    interp.register_native(key("Sum/2"), |i, a| {
        let projected = project(i, a)?;
        Ok(Some(total(&projected)))
    });

    interp.register_native(key("Average/1"), |i, a| {
        let items = source(i, a, 0);
        average(&items)
    });
    interp.register_native(key("Average/2"), |i, a| {
        let projected = project(i, a)?;
        average(&projected)
    });

    interp.register_native(key("Min/1"), |i, a| {
        let items = source(i, a, 0);
        extreme(i, items, true)
    });
    interp.register_native(key("Min/2"), |i, a| {
        let projected = project(i, a)?;
        extreme(i, projected, true)
    });
    interp.register_native(key("Max/1"), |i, a| {
        let items = source(i, a, 0);
        extreme(i, items, false)
    });
    interp.register_native(key("Max/2"), |i, a| {
        let projected = project(i, a)?;
        extreme(i, projected, false)
    });

    interp.register_native(key("Aggregate/2"), |i, a| {
        let items = source(i, a, 0);
        let combine = a.get(1).cloned().unwrap_or(Value::Null);
        let mut iter = items.into_iter();
        let Some(mut acc) = iter.next() else {
            return Err(empty_sequence("elements"));
        };
        for item in iter {
            acc = call(i, &combine, &[acc, item])?;
        }
        Ok(Some(acc))
    });
    interp.register_native(key("Aggregate/3"), |i, a| {
        let items = source(i, a, 0);
        let mut acc = arg(i, a, 1)?;
        let combine = a.get(2).cloned().unwrap_or(Value::Null);
        for item in items {
            acc = call(i, &combine, &[acc, item])?;
        }
        Ok(Some(acc))
    });
}

/// Applies the selector of a `Sum`/`Min`/`Max`/`Average` overload.
fn project(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Vec<Value>> {
    let items = source(interp, args, 0);
    let selector = args.get(1).cloned().unwrap_or(Value::Null);
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(call(interp, &selector, &[item])?);
    }
    Ok(out)
}

/// Sums a sequence, staying in integer arithmetic unless a float appears.
///
/// `new[] { 1, 2 }.Sum()` must produce `3`, not `3.0` — the difference shows up
/// the moment the result is printed.
fn total(items: &[Value]) -> Value {
    if items.iter().any(|v| matches!(v, Value::F(_))) {
        return Value::F(items.iter().filter_map(as_number).sum());
    }
    if items.iter().any(|v| matches!(v, Value::I64(_))) {
        return Value::I64(items.iter().filter_map(|v| v.as_i64()).sum());
    }
    Value::I32(items.iter().filter_map(|v| v.as_i64()).sum::<i64>() as i32)
}

fn average(items: &[Value]) -> ExecResult<Option<Value>> {
    if items.is_empty() {
        return Err(empty_sequence("elements"));
    }
    let sum: f64 = items.iter().filter_map(as_number).sum();
    Ok(Some(Value::F(sum / items.len() as f64)))
}

/// `Min` or `Max` over numbers or strings.
fn extreme(
    interp: &mut Interpreter,
    items: Vec<Value>,
    smallest: bool,
) -> ExecResult<Option<Value>> {
    if items.is_empty() {
        return Err(empty_sequence("elements"));
    }
    let mut best = items[0].clone();
    for item in items.into_iter().skip(1) {
        let order = compare(interp, &item, &best)?;
        if (smallest && order < 0) || (!smallest && order > 0) {
            best = item;
        }
    }
    Ok(Some(best))
}

/// Ordering for LINQ: numbers numerically, strings ordinally.
///
/// Anything else is refused rather than ordered arbitrarily — a silently wrong
/// sort is far harder to notice than a failed one.
pub(crate) fn compare(interp: &Interpreter, x: &Value, y: &Value) -> ExecResult<i32> {
    if let (Some(a), Some(b)) = (as_number(x), as_number(y)) {
        return Ok(match a.partial_cmp(&b) {
            Some(std::cmp::Ordering::Less) => -1,
            Some(std::cmp::Ordering::Greater) => 1,
            _ => 0,
        });
    }
    if let (Value::Obj(a), Value::Obj(b)) = (x, y) {
        if let (Some(sa), Some(sb)) = (
            interp.heap.get_as::<ClrString>(*a).map(|s| s.units.clone()),
            interp.heap.get_as::<ClrString>(*b).map(|s| s.units.clone()),
        ) {
            return Ok(match sa.cmp(&sb) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Equal => 0,
            });
        }
    }
    if matches!(x, Value::Null) && matches!(y, Value::Null) {
        return Ok(0);
    }
    Err(invalid_operation(
        "ordering on this runtime compares numbers and strings; \
         for any other key type supply a comparer, which is not yet supported",
    ))
}

// -- element access ----------------------------------------------------------

fn register_element_access(interp: &mut Interpreter) {
    interp.register_native(key("First/1"), |i, a| {
        source(i, a, 0).into_iter().next().map(Some).ok_or_else(|| empty_sequence("elements"))
    });
    interp.register_native(key("FirstOrDefault/1"), |i, a| {
        Ok(Some(source(i, a, 0).into_iter().next().unwrap_or(Value::Null)))
    });
    interp.register_native(key("Last/1"), |i, a| {
        source(i, a, 0).pop().map(Some).ok_or_else(|| empty_sequence("elements"))
    });
    interp.register_native(key("LastOrDefault/1"), |i, a| {
        Ok(Some(source(i, a, 0).pop().unwrap_or(Value::Null)))
    });

    interp.register_native(key("First/2"), |i, a| match matching(i, a)?.into_iter().next() {
        Some(v) => Ok(Some(v)),
        None => Err(empty_sequence("matching elements")),
    });
    interp.register_native(key("FirstOrDefault/2"), |i, a| {
        Ok(Some(matching(i, a)?.into_iter().next().unwrap_or(Value::Null)))
    });
    interp.register_native(key("Last/2"), |i, a| match matching(i, a)?.pop() {
        Some(v) => Ok(Some(v)),
        None => Err(empty_sequence("matching elements")),
    });
    interp.register_native(key("LastOrDefault/2"), |i, a| {
        Ok(Some(matching(i, a)?.pop().unwrap_or(Value::Null)))
    });

    interp.register_native(key("Single/1"), |i, a| single(source(i, a, 0), false));
    interp.register_native(key("SingleOrDefault/1"), |i, a| single(source(i, a, 0), true));
    interp.register_native(key("Single/2"), |i, a| single(matching(i, a)?, false));
    interp.register_native(key("SingleOrDefault/2"), |i, a| single(matching(i, a)?, true));

    interp.register_native(key("ElementAt/2"), |i, a| {
        let items = source(i, a, 0);
        let index = arg_i32(i, a, 1)?;
        if index < 0 || index as usize >= items.len() {
            return Err(out_of_range("index"));
        }
        Ok(Some(items[index as usize].clone()))
    });
    interp.register_native(key("ElementAtOrDefault/2"), |i, a| {
        let items = source(i, a, 0);
        let index = arg_i32(i, a, 1)?;
        Ok(Some(
            usize::try_from(index).ok().and_then(|n| items.get(n).cloned()).unwrap_or(Value::Null),
        ))
    });
}

/// The elements a predicate accepts.
fn matching(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Vec<Value>> {
    let items = source(interp, args, 0);
    let predicate = args.get(1).cloned().unwrap_or(Value::Null);
    let mut out = Vec::new();
    for item in items {
        if truthy(&call(interp, &predicate, &[item.clone()])?) {
            out.push(item);
        }
    }
    Ok(out)
}

fn single(items: Vec<Value>, allow_empty: bool) -> ExecResult<Option<Value>> {
    match items.len() {
        0 if allow_empty => Ok(Some(Value::Null)),
        0 => Err(empty_sequence("elements")),
        1 => Ok(Some(items.into_iter().next().expect("length checked"))),
        _ => Err(invalid_operation("Sequence contains more than one element")),
    }
}

// -- ordering ----------------------------------------------------------------

const ORDERED_ITEMS: usize = 0;
const ORDERED_LEVELS: usize = 1;
const ORDERED_DESCENDING: usize = 2;

fn register_ordering(interp: &mut Interpreter) {
    interp.register_native(key("OrderBy/2"), |i, a| order_by(i, a, false, false));
    interp.register_native(key("OrderByDescending/2"), |i, a| order_by(i, a, true, false));
    interp.register_native(key("ThenBy/2"), |i, a| order_by(i, a, false, true));
    interp.register_native(key("ThenByDescending/2"), |i, a| order_by(i, a, true, true));

    interp.register_native("System.Linq.OrderedEnumerable`1::GetEnumerator()", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let sorted = ordered_values(i, this);
        let snapshot = i.alloc_value_array(0);
        with_values(i, snapshot, |v| *v = sorted);
        Ok(Some(crate::collections::new_enumerator(
            i,
            "List`1+Enumerator",
            snapshot,
        )))
    });
    interp.register_native("System.Linq.OrderedEnumerable`1::get_Count()", |i, a| {
        let this = arg_handle(i, a, 0)?;
        Ok(Some(Value::I32(ordered_values(i, this).len() as i32)))
    });
}

/// Builds or extends an ordering.
///
/// `ThenBy` must *refine* the previous ordering, not replace it, so the keys of
/// each level are kept separately and applied in order when the result is
/// finally read. Sorting eagerly at each step and re-sorting on `ThenBy` would
/// discard the primary ordering — a wrong answer that looks plausible.
fn order_by(
    interp: &mut Interpreter,
    args: &[Value],
    descending: bool,
    is_secondary: bool,
) -> ExecResult<Option<Value>> {
    let selector = args.get(1).cloned().unwrap_or(Value::Null);
    let previous = args.first().cloned().unwrap_or(Value::Null);

    // A `ThenBy` on something that is not already ordered is not valid C#, so
    // this only has to handle the shape the compiler produces.
    let existing = previous
        .as_handle()
        .filter(|h| !h.is_null() && is_secondary)
        .filter(|h| is_ordered(interp, *h));

    let (items, mut levels, mut descending_flags) = match existing {
        Some(h) => (
            elements(interp, field_handle(interp, h, ORDERED_ITEMS)),
            elements(interp, field_handle(interp, h, ORDERED_LEVELS)),
            elements(interp, field_handle(interp, h, ORDERED_DESCENDING)),
        ),
        None => (source(interp, args, 0), Vec::new(), Vec::new()),
    };

    let mut keys = Vec::with_capacity(items.len());
    for item in &items {
        keys.push(call(interp, &selector, &[item.clone()])?);
    }
    let key_array = interp.alloc_value_array(0);
    with_values(interp, key_array, |v| *v = keys);

    levels.push(Value::Obj(key_array));
    descending_flags.push(Value::I32(descending as i32));

    let Some(type_id) = named_type(interp, "OrderedEnumerable`1")
        .or_else(|| interp.loader.registry.find_type_by_name("System.Linq.OrderedEnumerable`1"))
    else {
        return Ok(Some(new_list(interp, items)));
    };
    let handle = interp.alloc_object(type_id);
    for (slot, values) in [
        (ORDERED_ITEMS, items),
        (ORDERED_LEVELS, levels),
        (ORDERED_DESCENDING, descending_flags),
    ] {
        let array = interp.alloc_value_array(0);
        with_values(interp, array, |v| *v = values);
        set_field(interp, handle, slot, Value::Obj(array));
    }
    Ok(Some(Value::Obj(handle)))
}

fn is_ordered(interp: &Interpreter, handle: Handle) -> bool {
    interp
        .type_of(handle)
        .map(|t| interp.loader.registry.ty(t).full_name() == "System.Linq.OrderedEnumerable`1")
        .unwrap_or(false)
}

/// Applies every ordering level, most significant first.
///
/// The sort is stable, so elements equal on every level keep their original
/// order — which is what LINQ guarantees.
pub(crate) fn ordered_values(interp: &mut Interpreter, handle: Handle) -> Vec<Value> {
    let items = elements(interp, field_handle(interp, handle, ORDERED_ITEMS));
    let levels = elements(interp, field_handle(interp, handle, ORDERED_LEVELS));
    let flags = elements(interp, field_handle(interp, handle, ORDERED_DESCENDING));

    let keys: Vec<Vec<Value>> = levels
        .iter()
        .map(|level| match level.as_handle() {
            Some(h) => elements(interp, h),
            None => Vec::new(),
        })
        .collect();
    let descending: Vec<bool> = flags.iter().map(truthy).collect();

    let mut order: Vec<usize> = (0..items.len()).collect();
    // A comparison that cannot be made leaves the order untouched rather than
    // panicking inside the sort.
    order.sort_by(|a, b| {
        for (level, key_values) in keys.iter().enumerate() {
            let (Some(ka), Some(kb)) = (key_values.get(*a), key_values.get(*b)) else {
                continue;
            };
            let Ok(result) = compare(interp, ka, kb) else { continue };
            if result != 0 {
                let ordering = if result < 0 {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
                return if descending.get(level).copied().unwrap_or(false) {
                    ordering.reverse()
                } else {
                    ordering
                };
            }
        }
        std::cmp::Ordering::Equal
    });

    order.into_iter().filter_map(|i| items.get(i).cloned()).collect()
}

// -- grouping ----------------------------------------------------------------

fn register_grouping(interp: &mut Interpreter) {
    interp.register_native(key("GroupBy/2"), |i, a| {
        let items = source(i, a, 0);
        let selector = a.get(1).cloned().unwrap_or(Value::Null);

        // Groups are emitted in first-appearance order, as .NET does.
        let mut keys: Vec<Value> = Vec::new();
        let mut buckets: Vec<Vec<Value>> = Vec::new();
        for item in items {
            let k = call(i, &selector, &[item.clone()])?;
            match keys.iter().position(|existing| values_equal(i, existing, &k)) {
                Some(index) => buckets[index].push(item),
                None => {
                    keys.push(k);
                    buckets.push(vec![item]);
                }
            }
        }

        let mut groups = Vec::with_capacity(keys.len());
        for (k, members) in keys.into_iter().zip(buckets) {
            groups.push(new_grouping(i, k, members));
        }
        Ok(Some(new_list(i, groups)))
    });

    interp.register_native("System.Linq.Grouping`2::get_Key()", |i, a| {
        let this = arg_handle(i, a, 0)?;
        Ok(Some(field(i, this, 0)))
    });
    interp.register_native("System.Linq.Grouping`2::get_Count()", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let items = field_handle(i, this, 1);
        let count = i.heap.get_as::<ClrArray>(items).map(|x| x.len()).unwrap_or(0);
        Ok(Some(Value::I32(count as i32)))
    });
    interp.register_native("System.Linq.Grouping`2::GetEnumerator()", |i, a| {
        let this = arg_handle(i, a, 0)?;
        let items = field_handle(i, this, 1);
        Ok(Some(crate::collections::new_enumerator(i, "List`1+Enumerator", items)))
    });
}

fn new_grouping(interp: &mut Interpreter, k: Value, members: Vec<Value>) -> Value {
    let Some(type_id) = interp.loader.registry.find_type_by_name("System.Linq.Grouping`2") else {
        return Value::Null;
    };
    let handle = interp.alloc_object(type_id);
    let array = interp.alloc_value_array(0);
    with_values(interp, array, |v| *v = members);
    set_field(interp, handle, 0, k);
    set_field(interp, handle, 1, Value::Obj(array));
    Value::Obj(handle)
}

// -- materialisation ---------------------------------------------------------

fn register_conversions(interp: &mut Interpreter) {
    interp.register_native(key("ToList/1"), |i, a| {
        let items = source(i, a, 0);
        Ok(Some(new_list(i, items)))
    });

    interp.register_native(key("ToArray/1"), |i, a| {
        let items = source(i, a, 0);
        let array = i.alloc_value_array(0);
        with_values(i, array, |v| *v = items);
        Ok(Some(Value::Obj(array)))
    });

    interp.register_native(key("ToHashSet/1"), |i, a| {
        let items = source(i, a, 0);
        let mut out: Vec<Value> = Vec::new();
        for item in items {
            if !out.iter().any(|seen| values_equal(i, seen, &item)) {
                out.push(item);
            }
        }
        Ok(Some(new_list(i, out)))
    });

    // `ToDictionary(keySelector)` and `ToDictionary(keySelector, valueSelector)`.
    interp.register_native(key("ToDictionary/2"), |i, a| to_dictionary(i, a, false));
    interp.register_native(key("ToDictionary/3"), |i, a| to_dictionary(i, a, true));
}

fn to_dictionary(
    interp: &mut Interpreter,
    args: &[Value],
    with_value_selector: bool,
) -> ExecResult<Option<Value>> {
    let items = source(interp, args, 0);
    let key_selector = args.get(1).cloned().unwrap_or(Value::Null);
    let value_selector = args.get(2).cloned().unwrap_or(Value::Null);

    let Some(type_id) = named_type(interp, "Dictionary`2") else {
        return Ok(Some(Value::Null));
    };
    let handle = interp.alloc_object(type_id);
    crate::collections::reset_dictionary(interp, handle);

    for item in items {
        let k = call(interp, &key_selector, &[item.clone()])?;
        let v = if with_value_selector {
            call(interp, &value_selector, &[item])?
        } else {
            item
        };
        crate::collections::dictionary_set(interp, handle, k, v);
    }
    Ok(Some(Value::Obj(handle)))
}

// -- generators --------------------------------------------------------------

fn register_generators(interp: &mut Interpreter) {
    interp.register_native(key("Range/2"), |i, a| {
        let start = arg_i32(i, a, 0)?;
        let count = arg_i32(i, a, 1)?;
        if count < 0 {
            return Err(out_of_range("count"));
        }
        let values = (0..count).map(|n| Value::I32(start.wrapping_add(n))).collect();
        Ok(Some(new_list(i, values)))
    });

    interp.register_native(key("Repeat/2"), |i, a| {
        let item = arg(i, a, 0)?;
        let count = arg_i32(i, a, 1)?;
        if count < 0 {
            return Err(out_of_range("count"));
        }
        Ok(Some(new_list(i, vec![item; count as usize])))
    });

    interp.register_native(key("Empty/0"), |i, _a| Ok(Some(new_list(i, Vec::new()))));

    // Consistent with `values_equal`, which is what `Distinct` and `Contains`
    // use — the hash is exposed so a user comparer would agree with them.
    interp.register_native(key("GetHashCode/1"), |i, a| {
        let v = arg(i, a, 0)?;
        Ok(Some(Value::I32(value_hash(i, &v) as i32)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_stays_in_integer_arithmetic_unless_a_float_appears() {
        assert_eq!(total(&[Value::I32(1), Value::I32(2)]), Value::I32(3));
        assert_eq!(total(&[Value::I64(1), Value::I32(2)]), Value::I64(3));
        assert_eq!(total(&[Value::I32(1), Value::F(0.5)]), Value::F(1.5));
        assert_eq!(total(&[]), Value::I32(0), "an empty sum is zero, not an error");
    }

    #[test]
    fn single_distinguishes_empty_from_ambiguous() {
        assert!(single(Vec::new(), false).is_err());
        assert_eq!(single(Vec::new(), true).unwrap(), Some(Value::Null));
        assert_eq!(single(vec![Value::I32(4)], false).unwrap(), Some(Value::I32(4)));
        assert!(single(vec![Value::I32(4), Value::I32(5)], true).is_err());
    }
}
