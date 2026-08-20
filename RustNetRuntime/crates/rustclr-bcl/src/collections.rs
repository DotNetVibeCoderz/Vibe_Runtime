//! `System.Collections.Generic`: `List<T>`, `Dictionary<K,V>`, `HashSet<T>`,
//! `Queue<T>` and `Stack<T>`.
//!
//! These are the types that separate "runs a test program" from "runs real
//! code", and none of them can be managed code here — their bodies live in
//! CoreLib, which this runtime does not load. Each is a native type whose state
//! is held in ordinary managed arrays, so the collector traces the elements
//! with no special case and nothing needs pinning.
//!
//! # Why erasure is not a problem for these types
//!
//! Generic *types* are erased: `List<int>` and `List<string>` share one
//! `RuntimeType`, and the binding key for `Add` is
//! `List`1::Add(!0)` whatever `T` is. That would be a correctness problem if
//! storage were typed — but the element storage is `Value`, which already
//! carries its own shape. An `I32` slot and an `Obj` slot are distinguishable
//! without ever consulting a type argument, so `List<int>` holds unboxed
//! integers and `List<string>` holds string references, from the same code.
//!
//! # Hashing
//!
//! `Dictionary` and `HashSet` are chained hash tables: a bucket array of
//! 1-based entry indices, a `next` array threading each chain, and parallel
//! key/value arrays in insertion order. Enumeration walks the entry arrays, so
//! it yields insertion order — which is what .NET does too, absent removals.
//!
//! `Remove` rehashes the whole table rather than keeping a free list. That
//! makes removal O(n) and every other operation O(1), and it keeps enumeration
//! in insertion order after a removal, which .NET does not guarantee. The
//! trade is deliberate: removal is the rare operation, and a predictable
//! iteration order is worth more here than an asymptotically better `Remove`.

use crate::support::*;
use rustclr_core::{
    ArrayStorage, ClrArray, ClrBox, ClrExceptionKind, ClrObject, ClrString, ExecResult,
    ExecutionError, Interpreter, MethodId, MethodKind, TypeId, Value, DEFAULT_COMPARER_FIELD,
};
use rustclr_gc::Handle;

#[allow(unused_imports)]
use crate::prelude::*;

const GENERIC: &str = "System.Collections.Generic";

pub fn register(interp: &mut Interpreter) {
    register_list(interp);
    register_dictionary(interp);
    register_hash_set(interp);
    register_queue(interp);
    register_stack(interp);
    register_key_value_pair(interp);
    register_enumerators(interp);
    register_comparers(interp);
}

/// Leaks a stable key string; the native table holds it for the process life.
fn key(type_name: &str, member: &str) -> &'static str {
    Box::leak(format!("{GENERIC}.{type_name}::{member}").into_boxed_str())
}

// -- shared storage plumbing -------------------------------------------------

/// Reads field `slot` of a native collection object.
pub(crate) fn field(interp: &Interpreter, this: Handle, slot: usize) -> Value {
    interp
        .heap
        .get_as::<ClrObject>(this)
        .and_then(|o| o.fields.get(slot).cloned())
        .unwrap_or(Value::Null)
}

pub(crate) fn set_field(interp: &mut Interpreter, this: Handle, slot: usize, value: Value) {
    if let Some(o) = interp.heap.get_as_mut::<ClrObject>(this) {
        if let Some(f) = o.fields.get_mut(slot) {
            *f = value;
        }
    }
}

pub(crate) fn field_handle(interp: &Interpreter, this: Handle, slot: usize) -> Handle {
    match field(interp, this, slot) {
        Value::Obj(h) => h,
        _ => Handle::NULL,
    }
}

/// The elements of a `Values`-backed array.
pub(crate) fn elements(interp: &Interpreter, array: Handle) -> Vec<Value> {
    match interp.heap.get_as::<ClrArray>(array) {
        Some(a) => (0..a.len()).filter_map(|i| a.storage.get(i)).collect(),
        None => Vec::new(),
    }
}

pub(crate) fn element_count(interp: &Interpreter, array: Handle) -> usize {
    interp.heap.get_as::<ClrArray>(array).map(|a| a.len()).unwrap_or(0)
}

pub(crate) fn element_at(interp: &Interpreter, array: Handle, index: usize) -> Option<Value> {
    interp.heap.get_as::<ClrArray>(array).and_then(|a| a.storage.get(index))
}

/// Applies `edit` to the backing vector of a `Values` array.
///
/// Returns `None` when the handle is not untyped storage, which can only
/// happen if a collection's field was overwritten from outside — every path in
/// this module allocates with [`Interpreter::alloc_value_array`].
pub(crate) fn with_values<R>(
    interp: &mut Interpreter,
    array: Handle,
    edit: impl FnOnce(&mut Vec<Value>) -> R,
) -> Option<R> {
    interp
        .heap
        .get_as_mut::<ClrArray>(array)
        .and_then(|a| a.storage.values_mut())
        .map(edit)
}

/// `this` as a handle, refusing a null receiver the way the CLR does.
pub(crate) fn receiver(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Handle> {
    let h = arg_handle(interp, args, 0)?;
    if h.is_null() {
        return Err(ExecutionError::null_reference());
    }
    Ok(h)
}

pub(crate) fn invalid_operation(message: &str) -> ExecutionError {
    ExecutionError::exception(ClrExceptionKind::InvalidOperation, message.to_string())
}

fn key_not_found(interp: &mut Interpreter, k: &Value) -> ExecutionError {
    let rendered = display(interp, k);
    ExecutionError::exception(
        ClrExceptionKind::InvalidOperation,
        format!("The given key '{rendered}' was not present in the dictionary."),
    )
}

// -- value equality and hashing ----------------------------------------------

/// Structural equality for collection elements and dictionary keys.
///
/// Numbers compare by value across the widths the evaluation stack collapses,
/// strings by content, and boxes by the value inside — so `dict[1]` finds the
/// entry added as a boxed `int`. Reference types compare by identity, which is
/// `object.Equals` unless a type overrides it; a user override is not consulted
/// here, and [`values_equal`] is the only place that would need to change if it
/// ever should be.
pub fn values_equal(interp: &Interpreter, a: &Value, b: &Value) -> bool {
    let a = unbox(interp, a);
    let b = unbox(interp, b);
    match (&a, &b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::F(x), Value::F(y)) => x == y,
        (Value::F(x), other) | (other, Value::F(x)) => {
            as_number(other).map(|y| *x == y).unwrap_or(false)
        }
        (Value::Obj(x), Value::Obj(y)) => {
            if x == y {
                return true;
            }
            match (
                interp.heap.get_as::<ClrString>(*x),
                interp.heap.get_as::<ClrString>(*y),
            ) {
                (Some(sx), Some(sy)) => sx.units == sy.units,
                _ => false,
            }
        }
        (Value::Obj(_), _) | (_, Value::Obj(_)) => false,
        _ => match (a.as_i64(), b.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        },
    }
}

/// The numeric value of a slot, whatever width it holds.
///
/// `Value::as_f64` deliberately answers only for `F`, because IL conversion
/// and arithmetic must not silently widen an integer. Comparing collection
/// elements is the opposite case: a `List<double>` searched for an `int`
/// literal should find it, so this widens on purpose.
pub(crate) fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::I32(n) => Some(*n as f64),
        Value::I64(n) | Value::NativeInt(n) => Some(*n as f64),
        Value::F(f) => Some(*f),
        _ => None,
    }
}

/// Unwraps a box so a boxed `int` hashes and compares as the `int` it holds.
fn unbox(interp: &Interpreter, v: &Value) -> Value {
    match v {
        Value::Obj(h) => match interp.heap.get_as::<ClrBox>(*h) {
            Some(b) => b.value.clone(),
            None => v.clone(),
        },
        other => other.clone(),
    }
}

/// A hash consistent with [`values_equal`].
///
/// Anything that compares equal must hash equal, so integers of every width
/// fold to the same `i64` hash and a `double` holding an integral value hashes
/// as that integer.
pub fn value_hash(interp: &Interpreter, v: &Value) -> u64 {
    let v = unbox(interp, v);
    match &v {
        Value::Null => 0,
        Value::F(f) => {
            // A `double` that compares equal to an integer must hash with it.
            if crate::fmath::fract(*f) == 0.0 && f.is_finite() && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                mix(*f as i64 as u64)
            } else {
                mix(f.to_bits())
            }
        }
        Value::Obj(h) => match interp.heap.get_as::<ClrString>(*h) {
            Some(s) => {
                let mut acc = 0xcbf2_9ce4_8422_2325u64;
                for unit in &s.units {
                    acc ^= *unit as u64;
                    acc = acc.wrapping_mul(0x0000_0100_0000_01b3);
                }
                acc
            }
            None => mix(h.to_bits()),
        },
        other => match other.as_i64() {
            Some(n) => mix(n as u64),
            None => 0,
        },
    }
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

// -- List<T> -----------------------------------------------------------------

/// Slot 0 of a `List<T>`: the backing array, whose length *is* the count.
const LIST_ITEMS: usize = 0;

fn list_items(interp: &Interpreter, this: Handle) -> Handle {
    field_handle(interp, this, LIST_ITEMS)
}

/// Ensures the receiver has storage, so a `List<T>` reached before its
/// constructor ran still behaves as an empty list rather than throwing.
fn ensure_list(interp: &mut Interpreter, this: Handle) -> Handle {
    let existing = list_items(interp, this);
    if !existing.is_null() {
        return existing;
    }
    let array = interp.alloc_value_array(0);
    set_field(interp, this, LIST_ITEMS, Value::Obj(array));
    array
}

fn register_list(interp: &mut Interpreter) {
    for ctor in [".ctor()", ".ctor(int)", ".ctor/0", ".ctor/1"] {
        interp.register_native(key("List`1", ctor), |i, a| {
            let this = receiver(i, a)?;
            let array = i.alloc_value_array(0);
            set_field(i, this, LIST_ITEMS, Value::Obj(array));
            // `new List<T>(otherCollection)` copies the source in.
            if let Some(source) = a.get(1) {
                let seed = sequence_values(i, source);
                with_values(i, array, |v| *v = seed);
            }
            Ok(None)
        });
    }

    interp.register_native(key("List`1", "Add(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let array = ensure_list(i, this);
        let item = arg(i, a, 1)?;
        with_values(i, array, |v| v.push(item));
        Ok(None)
    });

    interp.register_native(key("List`1", "AddRange/1"), |i, a| {
        let this = receiver(i, a)?;
        let array = ensure_list(i, this);
        let source = a.get(1).cloned().unwrap_or(Value::Null);
        let extra = sequence_values(i, &source);
        with_values(i, array, |v| v.extend(extra));
        Ok(None)
    });

    interp.register_native(key("List`1", "get_Count()"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        Ok(Some(Value::I32(element_count(i, array) as i32)))
    });

    // Capacity is not modelled separately: the backing vector grows on demand,
    // so capacity and count are the same number. Reading it is harmless;
    // setting it is a hint this runtime does not need.
    interp.register_native(key("List`1", "get_Capacity()"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        Ok(Some(Value::I32(element_count(i, array) as i32)))
    });
    interp.register_native(key("List`1", "set_Capacity(int)"), |_, _| Ok(None));

    interp.register_native(key("List`1", "get_Item(int)"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        let index = arg_i32(i, a, 1)?;
        let count = element_count(i, array);
        if index < 0 || index as usize >= count {
            return Err(out_of_range("index"));
        }
        Ok(Some(element_at(i, array, index as usize).unwrap_or(Value::Null)))
    });

    interp.register_native(key("List`1", "set_Item(int,!0)"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        let index = arg_i32(i, a, 1)?;
        let item = arg(i, a, 2)?;
        let count = element_count(i, array);
        if index < 0 || index as usize >= count {
            return Err(out_of_range("index"));
        }
        with_values(i, array, |v| v[index as usize] = item);
        Ok(None)
    });

    interp.register_native(key("List`1", "Insert(int,!0)"), |i, a| {
        let this = receiver(i, a)?;
        let array = ensure_list(i, this);
        let index = arg_i32(i, a, 1)?;
        let item = arg(i, a, 2)?;
        let count = element_count(i, array);
        if index < 0 || index as usize > count {
            return Err(out_of_range("index"));
        }
        with_values(i, array, |v| v.insert(index as usize, item));
        Ok(None)
    });

    interp.register_native(key("List`1", "RemoveAt(int)"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        let index = arg_i32(i, a, 1)?;
        let count = element_count(i, array);
        if index < 0 || index as usize >= count {
            return Err(out_of_range("index"));
        }
        with_values(i, array, |v| v.remove(index as usize));
        Ok(None)
    });

    interp.register_native(key("List`1", "Remove(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        let item = arg(i, a, 1)?;
        match index_of(i, array, &item) {
            Some(index) => {
                with_values(i, array, |v| v.remove(index));
                Ok(Some(Value::I32(1)))
            }
            None => Ok(Some(Value::I32(0))),
        }
    });

    interp.register_native(key("List`1", "IndexOf(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        let item = arg(i, a, 1)?;
        Ok(Some(Value::I32(index_of(i, array, &item).map(|n| n as i32).unwrap_or(-1))))
    });

    interp.register_native(key("List`1", "Contains(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        let item = arg(i, a, 1)?;
        Ok(Some(Value::I32(index_of(i, array, &item).is_some() as i32)))
    });

    interp.register_native(key("List`1", "Clear()"), |i, a| {
        let this = receiver(i, a)?;
        let array = ensure_list(i, this);
        with_values(i, array, |v| v.clear());
        Ok(None)
    });

    interp.register_native(key("List`1", "Reverse()"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        with_values(i, array, |v| v.reverse());
        Ok(None)
    });

    interp.register_native(key("List`1", "Sort()"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        sort_in_place(i, array)
    });

    interp.register_native(key("List`1", "ToArray()"), |i, a| {
        let this = receiver(i, a)?;
        let array = list_items(i, this);
        let values = elements(i, array);
        let copy = i.alloc_value_array(0);
        with_values(i, copy, |v| *v = values);
        Ok(Some(Value::Obj(copy)))
    });

    interp.register_native(key("List`1", "GetEnumerator()"), |i, a| {
        let this = receiver(i, a)?;
        let array = ensure_list(i, this);
        Ok(Some(new_enumerator(i, "List`1+Enumerator", array)))
    });
}

fn index_of(interp: &Interpreter, array: Handle, item: &Value) -> Option<usize> {
    let values = elements(interp, array);
    values.iter().position(|v| values_equal(interp, v, item))
}

/// Sorts untyped storage by the ordering `Comparer<T>.Default` would use.
///
/// Numbers compare numerically and strings ordinally, which covers the element
/// types a program can sort without supplying a comparer. A mixed or
/// unorderable list is refused rather than silently left in an arbitrary order.
fn sort_in_place(interp: &mut Interpreter, array: Handle) -> ExecResult<Option<Value>> {
    let mut values = elements(interp, array);
    if !values.is_empty() && values.iter().all(|v| as_number(v).is_some()) {
        values.sort_by(|a, b| {
            as_number(a)
                .unwrap_or(0.0)
                .partial_cmp(&as_number(b).unwrap_or(0.0))
                .unwrap_or(core::cmp::Ordering::Equal)
        });
    } else if values
        .iter()
        .all(|v| matches!(v, Value::Obj(h) if interp.heap.get_as::<ClrString>(*h).is_some()))
    {
        let mut keyed: Vec<(Vec<u16>, Value)> = values
            .into_iter()
            .map(|v| {
                let units = match &v {
                    Value::Obj(h) => interp
                        .heap
                        .get_as::<ClrString>(*h)
                        .map(|s| s.units.clone())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                (units, v)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        values = keyed.into_iter().map(|(_, v)| v).collect();
    } else if !values.is_empty() {
        return Err(invalid_operation(
            "Sort() on this runtime orders numbers and strings; \
             for any other element type pass a comparer, which is not yet supported.",
        ));
    }
    with_values(interp, array, |v| *v = values);
    Ok(None)
}

/// Reads anything enumerable into a vector of values.
///
/// The built-in collections are read straight out of their backing arrays,
/// which is both faster and free of allocation. Anything else — a user type
/// implementing `IEnumerable<T>`, including the state machine a `yield return`
/// method compiles to — is driven through the enumerator protocol itself:
/// `GetEnumerator`, then `MoveNext` and `Current` until it stops. That is what
/// lets LINQ and `new List<T>(source)` work over sequences this runtime has
/// never heard of.
pub fn sequence_values(interp: &mut Interpreter, source: &Value) -> Vec<Value> {
    let Some(h) = source.as_handle().filter(|h| !h.is_null()) else {
        return Vec::new();
    };
    // A plain array.
    if interp.heap.get_as::<ClrArray>(h).is_some() {
        return elements(interp, h);
    }
    let Some(type_id) = interp.type_of(h) else { return Vec::new() };
    let name = interp.loader.registry.ty(type_id).full_name();
    match name.rsplit_once('.').map(|(_, n)| n).unwrap_or(&name) {
        "List`1" | "Queue`1" | "Stack`1" => elements(interp, field_handle(interp, h, 0)),
        "HashSet`1" => elements(interp, field_handle(interp, h, HASHSET_ITEMS)),
        "Grouping`2" => elements(interp, field_handle(interp, h, 1)),
        "Dictionary`2" => {
            let keys = elements(interp, field_handle(interp, h, DICT_KEYS));
            let values = elements(interp, field_handle(interp, h, DICT_VALUES));
            keys.into_iter()
                .zip(values)
                .map(|(k, v)| new_key_value_pair(interp, k, v))
                .collect()
        }
        "OrderedEnumerable`1" => crate::linq::ordered_values(interp, h),
        _ => drive_enumerator(interp, h).unwrap_or_default(),
    }
}

/// Walks an arbitrary `IEnumerable` by calling its own enumerator.
///
/// Bounded by [`ENUMERATION_LIMIT`]: an enumerator whose `MoveNext` never
/// returns false would otherwise hang the process with no diagnosis. The limit
/// is high enough that no honest sequence reaches it.
fn drive_enumerator(interp: &mut Interpreter, source: Handle) -> Option<Vec<Value>> {
    let type_id = interp.type_of(source)?;
    let get_enumerator = find_member(interp, type_id, "GetEnumerator", 0)?;
    let enumerator = interp.invoke(get_enumerator, vec![Value::Obj(source)]).ok()??;
    let enumerator_handle = enumerator.as_handle().filter(|h| !h.is_null())?;
    let enumerator_type = interp.type_of(enumerator_handle)?;

    let move_next = find_member(interp, enumerator_type, "MoveNext", 0)?;
    let current = find_member(interp, enumerator_type, "get_Current", 0)?;

    let mut out = Vec::new();
    for _ in 0..ENUMERATION_LIMIT {
        let advanced = interp.invoke(move_next, vec![enumerator.clone()]).ok()?;
        if advanced.and_then(|v| v.as_i32()).unwrap_or(0) == 0 {
            return Some(out);
        }
        let item = interp.invoke(current, vec![enumerator.clone()]).ok()??;
        out.push(item);
    }
    Some(out)
}

/// The point at which an enumerator is treated as non-terminating.
const ENUMERATION_LIMIT: usize = 100_000_000;

/// Finds a zero-or-more-argument member on a type, managed or native.
///
/// Managed methods are preferred; a natively implemented type has none, so its
/// binding is interned on demand — the same mechanism interface dispatch uses.
///
/// An explicit interface implementation is emitted under a qualified name —
/// `System.Collections.Generic.IEnumerator<int>.get_Current` — so the match is
/// on the final segment too. Every `yield return` state machine looks like
/// that, and matching only the bare name would miss all of them.
fn find_member(
    interp: &mut Interpreter,
    type_id: TypeId,
    name: &str,
    params: usize,
) -> Option<MethodId> {
    let suffix = format!(".{name}");
    for t in interp.loader.registry.base_chain(type_id).collect::<Vec<_>>() {
        for m in interp.loader.registry.ty(t).methods.clone() {
            let info = interp.loader.registry.method(m);
            if info.signature.params.len() != params {
                continue;
            }
            if info.name == name || info.name.ends_with(&suffix) {
                return Some(m);
            }
        }
    }
    let full = interp.loader.registry.ty(type_id).full_name();
    let native = format!("{full}::{name}({})", vec!["!0"; params].join(","));
    let native = if params == 0 { format!("{full}::{name}()") } else { native };
    if !interp.has_native(&native) {
        return None;
    }
    let signature = rustclr_core::metadata::MethodSig {
        calling_convention: 0,
        has_this: true,
        explicit_this: false,
        generic_param_count: 0,
        return_type: rustclr_core::metadata::TypeSig::Object,
        params: vec![rustclr_core::metadata::TypeSig::Object; params],
        sentinel_at: None,
    };
    Some(interp.loader.intern_internal_call(type_id, name, signature, native))
}

/// Allocates a `List<T>` holding `values`.
///
/// Every LINQ operator returns one of these: a concrete collection the rest of
/// the runtime already knows how to enumerate, index and count.
pub(crate) fn new_list(interp: &mut Interpreter, values: Vec<Value>) -> Value {
    let Some(type_id) = named_type(interp, "List`1") else {
        return Value::Null;
    };
    let handle = interp.alloc_object(type_id);
    let array = interp.alloc_value_array(0);
    with_values(interp, array, |v| *v = values);
    set_field(interp, handle, LIST_ITEMS, Value::Obj(array));
    Value::Obj(handle)
}

// -- Dictionary<K,V> ---------------------------------------------------------

const DICT_BUCKETS: usize = 0;
const DICT_NEXT: usize = 1;
const DICT_KEYS: usize = 2;
const DICT_VALUES: usize = 3;

fn register_dictionary(interp: &mut Interpreter) {
    for ctor in [".ctor()", ".ctor(int)", ".ctor/0", ".ctor/1"] {
        interp.register_native(key("Dictionary`2", ctor), |i, a| {
            let this = receiver(i, a)?;
            reset_table(i, this, DICT_BUCKETS, DICT_NEXT, &[DICT_KEYS, DICT_VALUES]);
            Ok(None)
        });
    }

    interp.register_native(key("Dictionary`2", "Add(!0,!1)"), |i, a| {
        let this = receiver(i, a)?;
        let k = arg(i, a, 1)?;
        let v = arg(i, a, 2)?;
        if dict_find(i, this, &k).is_some() {
            let rendered = display(i, &k);
            return Err(ExecutionError::exception(
                ClrExceptionKind::Argument,
                format!("An item with the same key has already been added. Key: {rendered}"),
            ));
        }
        dict_insert(i, this, k, v);
        Ok(None)
    });

    interp.register_native(key("Dictionary`2", "set_Item(!0,!1)"), |i, a| {
        let this = receiver(i, a)?;
        let k = arg(i, a, 1)?;
        let v = arg(i, a, 2)?;
        match dict_find(i, this, &k) {
            Some(index) => {
                let values = field_handle(i, this, DICT_VALUES);
                with_values(i, values, |vs| vs[index] = v);
            }
            None => dict_insert(i, this, k, v),
        }
        Ok(None)
    });

    interp.register_native(key("Dictionary`2", "get_Item(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let k = arg(i, a, 1)?;
        match dict_find(i, this, &k) {
            Some(index) => {
                let values = field_handle(i, this, DICT_VALUES);
                Ok(Some(element_at(i, values, index).unwrap_or(Value::Null)))
            }
            None => Err(key_not_found(i, &k)),
        }
    });

    interp.register_native(key("Dictionary`2", "TryGetValue(!0,!1&)"), |i, a| {
        let this = receiver(i, a)?;
        let k = arg(i, a, 1)?;
        let found = dict_find(i, this, &k);
        let result = match found {
            Some(index) => {
                let values = field_handle(i, this, DICT_VALUES);
                element_at(i, values, index).unwrap_or(Value::Null)
            }
            None => Value::Null,
        };
        if let Some(Value::Ref(target)) = a.get(2) {
            let target = target.clone();
            i.store_indirect_public(target, result)?;
        }
        Ok(Some(Value::I32(found.is_some() as i32)))
    });

    interp.register_native(key("Dictionary`2", "ContainsKey(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let k = arg(i, a, 1)?;
        Ok(Some(Value::I32(dict_find(i, this, &k).is_some() as i32)))
    });

    interp.register_native(key("Dictionary`2", "ContainsValue(!1)"), |i, a| {
        let this = receiver(i, a)?;
        let needle = arg(i, a, 1)?;
        let values = elements(i, field_handle(i, this, DICT_VALUES));
        Ok(Some(Value::I32(
            values.iter().any(|v| values_equal(i, v, &needle)) as i32,
        )))
    });

    interp.register_native(key("Dictionary`2", "get_Count()"), |i, a| {
        let this = receiver(i, a)?;
        let keys = field_handle(i, this, DICT_KEYS);
        Ok(Some(Value::I32(element_count(i, keys) as i32)))
    });

    interp.register_native(key("Dictionary`2", "Remove(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let k = arg(i, a, 1)?;
        let Some(index) = dict_find(i, this, &k) else {
            return Ok(Some(Value::I32(0)));
        };
        let mut keys = elements(i, field_handle(i, this, DICT_KEYS));
        let mut values = elements(i, field_handle(i, this, DICT_VALUES));
        keys.remove(index);
        values.remove(index);
        reset_table(i, this, DICT_BUCKETS, DICT_NEXT, &[DICT_KEYS, DICT_VALUES]);
        for (k, v) in keys.into_iter().zip(values) {
            dict_insert(i, this, k, v);
        }
        Ok(Some(Value::I32(1)))
    });

    interp.register_native(key("Dictionary`2", "Clear()"), |i, a| {
        let this = receiver(i, a)?;
        reset_table(i, this, DICT_BUCKETS, DICT_NEXT, &[DICT_KEYS, DICT_VALUES]);
        Ok(None)
    });

    interp.register_native(key("Dictionary`2", "GetEnumerator()"), |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(new_enumerator(i, "Dictionary`2+Enumerator", this)))
    });

    interp.register_native(key("Dictionary`2", "get_Keys()"), |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(new_view(i, "Dictionary`2+KeyCollection", this)))
    });
    interp.register_native(key("Dictionary`2", "get_Values()"), |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(new_view(i, "Dictionary`2+ValueCollection", this)))
    });

    // `Keys` and `Values` differ only in which entry array they expose. Native
    // bindings are plain function pointers, so each shape gets its own thin
    // entry point rather than a captured slot index.
    interp.register_native(key("Dictionary`2+KeyCollection", "get_Count()"), |i, a| {
        view_count(i, a, DICT_KEYS)
    });
    interp.register_native(key("Dictionary`2+ValueCollection", "get_Count()"), |i, a| {
        view_count(i, a, DICT_VALUES)
    });
    interp.register_native(key("Dictionary`2+KeyCollection", "GetEnumerator()"), |i, a| {
        view_enumerator(i, a, DICT_KEYS, "Dictionary`2+KeyCollection+Enumerator")
    });
    interp.register_native(key("Dictionary`2+ValueCollection", "GetEnumerator()"), |i, a| {
        view_enumerator(i, a, DICT_VALUES, "Dictionary`2+ValueCollection+Enumerator")
    });
}

fn view_count(interp: &mut Interpreter, args: &[Value], slot: usize) -> ExecResult<Option<Value>> {
    let this = receiver(interp, args)?;
    let source = field_handle(interp, this, 0);
    let array = field_handle(interp, source, slot);
    Ok(Some(Value::I32(element_count(interp, array) as i32)))
}

fn view_enumerator(
    interp: &mut Interpreter,
    args: &[Value],
    slot: usize,
    enumerator_type: &str,
) -> ExecResult<Option<Value>> {
    let this = receiver(interp, args)?;
    let source = field_handle(interp, this, 0);
    let array = field_handle(interp, source, slot);
    Ok(Some(new_enumerator(interp, enumerator_type, array)))
}

/// Appends to a `Values`-backed array.
pub(crate) fn push_value(interp: &mut Interpreter, array: Handle, value: Value) {
    with_values(interp, array, |v| v.push(value));
}

/// Resets a dictionary to empty storage. Used by `Enumerable.ToDictionary`,
/// which builds one without going through a managed constructor.
pub(crate) fn reset_dictionary(interp: &mut Interpreter, this: Handle) {
    reset_table(interp, this, DICT_BUCKETS, DICT_NEXT, &[DICT_KEYS, DICT_VALUES]);
}

/// Adds or replaces an entry, as the indexer does.
pub(crate) fn dictionary_set(interp: &mut Interpreter, this: Handle, k: Value, v: Value) {
    match dict_find(interp, this, &k) {
        Some(index) => {
            let values = field_handle(interp, this, DICT_VALUES);
            with_values(interp, values, |vs| vs[index] = v);
        }
        None => dict_insert(interp, this, k, v),
    }
}

/// Installs empty bucket, chain and entry arrays on a hash-backed collection.
fn reset_table(
    interp: &mut Interpreter,
    this: Handle,
    buckets_slot: usize,
    next_slot: usize,
    entry_slots: &[usize],
) {
    let int_type = interp.loader.primitive_type(rustclr_core::Primitive::Int32);
    let buckets = interp.alloc_array(int_type, INITIAL_BUCKETS);
    set_field(interp, this, buckets_slot, Value::Obj(buckets));
    let next = interp.alloc_array(int_type, 0);
    set_field(interp, this, next_slot, Value::Obj(next));
    for slot in entry_slots {
        let array = interp.alloc_value_array(0);
        set_field(interp, this, *slot, Value::Obj(array));
    }
}

const INITIAL_BUCKETS: usize = 8;

fn bucket_of(interp: &Interpreter, buckets: Handle, k: &Value) -> usize {
    let n = element_count(interp, buckets).max(1);
    (value_hash(interp, k) % n as u64) as usize
}

fn int_at(interp: &Interpreter, array: Handle, index: usize) -> i32 {
    element_at(interp, array, index).and_then(|v| v.as_i32()).unwrap_or(0)
}

fn set_int(interp: &mut Interpreter, array: Handle, index: usize, value: i32) {
    if let Some(a) = interp.heap.get_as_mut::<ClrArray>(array) {
        a.storage.set(index, &Value::I32(value));
    }
}

fn push_int(interp: &mut Interpreter, array: Handle, value: i32) {
    if let Some(a) = interp.heap.get_as_mut::<ClrArray>(array) {
        if let ArrayStorage::I32(v) = &mut a.storage {
            v.push(value);
            a.dimensions = vec![v.len() as u32];
        }
    }
}

/// Finds the entry index for `k`, or `None`.
fn dict_find(interp: &mut Interpreter, this: Handle, k: &Value) -> Option<usize> {
    table_find(interp, this, DICT_BUCKETS, DICT_NEXT, DICT_KEYS, k)
}

fn table_find(
    interp: &mut Interpreter,
    this: Handle,
    buckets_slot: usize,
    next_slot: usize,
    keys_slot: usize,
    k: &Value,
) -> Option<usize> {
    let buckets = field_handle(interp, this, buckets_slot);
    if buckets.is_null() {
        return None;
    }
    let next = field_handle(interp, this, next_slot);
    let keys = field_handle(interp, this, keys_slot);
    let bucket = bucket_of(interp, buckets, k);
    let entries = element_count(interp, keys);

    let mut current = int_at(interp, buckets, bucket) - 1;
    // Bounded by the entry count: a corrupted chain cannot loop forever.
    for _ in 0..=entries {
        if current < 0 {
            return None;
        }
        let index = current as usize;
        let candidate = element_at(interp, keys, index)?;
        if values_equal(interp, &candidate, k) {
            return Some(index);
        }
        current = int_at(interp, next, index) - 1;
    }
    None
}

/// Appends a new entry and links it into its bucket. The caller has already
/// established that the key is absent.
fn dict_insert(interp: &mut Interpreter, this: Handle, k: Value, v: Value) {
    let values = field_handle(interp, this, DICT_VALUES);
    with_values(interp, values, |vs| vs.push(v));
    table_insert(interp, this, DICT_BUCKETS, DICT_NEXT, DICT_KEYS, k);
}

/// Appends `k` to the key array and links it into the bucket table, growing
/// the table when the load factor passes one.
fn table_insert(
    interp: &mut Interpreter,
    this: Handle,
    buckets_slot: usize,
    next_slot: usize,
    keys_slot: usize,
    k: Value,
) {
    let keys = field_handle(interp, this, keys_slot);
    let index = element_count(interp, keys);
    with_values(interp, keys, |vs| vs.push(k.clone()));

    let buckets = field_handle(interp, this, buckets_slot);
    let next = field_handle(interp, this, next_slot);
    let bucket = bucket_of(interp, buckets, &k);
    let head = int_at(interp, buckets, bucket);
    push_int(interp, next, head);
    set_int(interp, buckets, bucket, index as i32 + 1);

    if index + 1 > element_count(interp, buckets) {
        rehash(interp, this, buckets_slot, next_slot, keys_slot);
    }
}

/// Doubles the bucket table and rebuilds every chain.
fn rehash(
    interp: &mut Interpreter,
    this: Handle,
    buckets_slot: usize,
    next_slot: usize,
    keys_slot: usize,
) {
    let keys = field_handle(interp, this, keys_slot);
    let all = elements(interp, keys);
    let size = (all.len() * 2).max(INITIAL_BUCKETS);

    let int_type = interp.loader.primitive_type(rustclr_core::Primitive::Int32);
    let buckets = interp.alloc_array(int_type, size);
    let next = interp.alloc_array(int_type, all.len());

    for (index, k) in all.iter().enumerate() {
        let bucket = bucket_of(interp, buckets, k);
        let head = int_at(interp, buckets, bucket);
        set_int(interp, next, index, head);
        set_int(interp, buckets, bucket, index as i32 + 1);
    }

    set_field(interp, this, buckets_slot, Value::Obj(buckets));
    set_field(interp, this, next_slot, Value::Obj(next));
}

// -- HashSet<T> --------------------------------------------------------------

const HASHSET_BUCKETS: usize = 0;
const HASHSET_NEXT: usize = 1;
const HASHSET_ITEMS: usize = 2;

fn register_hash_set(interp: &mut Interpreter) {
    for ctor in [".ctor()", ".ctor(int)", ".ctor/0", ".ctor/1"] {
        interp.register_native(key("HashSet`1", ctor), |i, a| {
            let this = receiver(i, a)?;
            reset_table(i, this, HASHSET_BUCKETS, HASHSET_NEXT, &[HASHSET_ITEMS]);
            if let Some(source) = a.get(1).cloned() {
                for item in sequence_values(i, &source) {
                    if set_find(i, this, &item).is_none() {
                        table_insert(i, this, HASHSET_BUCKETS, HASHSET_NEXT, HASHSET_ITEMS, item);
                    }
                }
            }
            Ok(None)
        });
    }

    interp.register_native(key("HashSet`1", "Add(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let item = arg(i, a, 1)?;
        if set_find(i, this, &item).is_some() {
            return Ok(Some(Value::I32(0)));
        }
        table_insert(i, this, HASHSET_BUCKETS, HASHSET_NEXT, HASHSET_ITEMS, item);
        Ok(Some(Value::I32(1)))
    });

    interp.register_native(key("HashSet`1", "Contains(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let item = arg(i, a, 1)?;
        Ok(Some(Value::I32(set_find(i, this, &item).is_some() as i32)))
    });

    interp.register_native(key("HashSet`1", "Remove(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let item = arg(i, a, 1)?;
        let Some(index) = set_find(i, this, &item) else {
            return Ok(Some(Value::I32(0)));
        };
        let mut items = elements(i, field_handle(i, this, HASHSET_ITEMS));
        items.remove(index);
        reset_table(i, this, HASHSET_BUCKETS, HASHSET_NEXT, &[HASHSET_ITEMS]);
        for item in items {
            table_insert(i, this, HASHSET_BUCKETS, HASHSET_NEXT, HASHSET_ITEMS, item);
        }
        Ok(Some(Value::I32(1)))
    });

    interp.register_native(key("HashSet`1", "get_Count()"), |i, a| {
        let this = receiver(i, a)?;
        let items = field_handle(i, this, HASHSET_ITEMS);
        Ok(Some(Value::I32(element_count(i, items) as i32)))
    });

    interp.register_native(key("HashSet`1", "Clear()"), |i, a| {
        let this = receiver(i, a)?;
        reset_table(i, this, HASHSET_BUCKETS, HASHSET_NEXT, &[HASHSET_ITEMS]);
        Ok(None)
    });

    interp.register_native(key("HashSet`1", "GetEnumerator()"), |i, a| {
        let this = receiver(i, a)?;
        let items = field_handle(i, this, HASHSET_ITEMS);
        Ok(Some(new_enumerator(i, "HashSet`1+Enumerator", items)))
    });
}

fn set_find(interp: &mut Interpreter, this: Handle, item: &Value) -> Option<usize> {
    table_find(interp, this, HASHSET_BUCKETS, HASHSET_NEXT, HASHSET_ITEMS, item)
}

// -- Queue<T> and Stack<T> ---------------------------------------------------
//
// The two differ only in which end they take from. Native bindings are plain
// function pointers, so the shared behaviour lives in helpers that take that
// choice as an argument, and each type registers thin entry points.

fn register_queue(interp: &mut Interpreter) {
    register_linear_shared(interp, "Queue`1");
    interp.register_native(key("Queue`1", "Enqueue(!0)"), linear_add);
    interp.register_native(key("Queue`1", "Dequeue()"), |i, a| linear_take(i, a, true));
    interp.register_native(key("Queue`1", "Peek()"), |i, a| linear_peek(i, a, true));
    interp.register_native(key("Queue`1", "ToArray()"), |i, a| linear_to_array(i, a, true));
    interp.register_native(key("Queue`1", "GetEnumerator()"), |i, a| {
        linear_enumerator(i, a, true, "Queue`1+Enumerator")
    });
}

fn register_stack(interp: &mut Interpreter) {
    register_linear_shared(interp, "Stack`1");
    interp.register_native(key("Stack`1", "Push(!0)"), linear_add);
    interp.register_native(key("Stack`1", "Pop()"), |i, a| linear_take(i, a, false));
    interp.register_native(key("Stack`1", "Peek()"), |i, a| linear_peek(i, a, false));
    interp.register_native(key("Stack`1", "ToArray()"), |i, a| linear_to_array(i, a, false));
    interp.register_native(key("Stack`1", "GetEnumerator()"), |i, a| {
        linear_enumerator(i, a, false, "Stack`1+Enumerator")
    });
}

/// The members a queue and a stack implement identically.
fn register_linear_shared(interp: &mut Interpreter, type_name: &str) {
    for ctor in [".ctor()", ".ctor(int)", ".ctor/0", ".ctor/1"] {
        interp.register_native(key(type_name, ctor), |i, a| {
            let this = receiver(i, a)?;
            let array = i.alloc_value_array(0);
            set_field(i, this, 0, Value::Obj(array));
            if let Some(source) = a.get(1) {
                let seed = sequence_values(i, source);
                with_values(i, array, |v| *v = seed);
            }
            Ok(None)
        });
    }

    interp.register_native(key(type_name, "get_Count()"), |i, a| {
        let this = receiver(i, a)?;
        let array = field_handle(i, this, 0);
        Ok(Some(Value::I32(element_count(i, array) as i32)))
    });

    interp.register_native(key(type_name, "Contains(!0)"), |i, a| {
        let this = receiver(i, a)?;
        let array = field_handle(i, this, 0);
        let item = arg(i, a, 1)?;
        Ok(Some(Value::I32(index_of(i, array, &item).is_some() as i32)))
    });

    interp.register_native(key(type_name, "Clear()"), |i, a| {
        let this = receiver(i, a)?;
        let array = field_handle(i, this, 0);
        with_values(i, array, |v| v.clear());
        Ok(None)
    });
}

/// `Enqueue` and `Push` both append; only the removing end differs.
fn linear_add(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let this = receiver(interp, args)?;
    let array = field_handle(interp, this, 0);
    let item = arg(interp, args, 1)?;
    with_values(interp, array, |v| v.push(item));
    Ok(None)
}

fn empty_error(from_front: bool) -> ExecutionError {
    invalid_operation(if from_front { "Queue empty." } else { "Stack empty." })
}

fn linear_take(
    interp: &mut Interpreter,
    args: &[Value],
    from_front: bool,
) -> ExecResult<Option<Value>> {
    let this = receiver(interp, args)?;
    let array = field_handle(interp, this, 0);
    if element_count(interp, array) == 0 {
        return Err(empty_error(from_front));
    }
    let taken = with_values(interp, array, |v| {
        if from_front {
            v.remove(0)
        } else {
            v.pop().expect("checked non-empty")
        }
    });
    Ok(Some(taken.unwrap_or(Value::Null)))
}

fn linear_peek(
    interp: &mut Interpreter,
    args: &[Value],
    from_front: bool,
) -> ExecResult<Option<Value>> {
    let this = receiver(interp, args)?;
    let array = field_handle(interp, this, 0);
    let count = element_count(interp, array);
    if count == 0 {
        return Err(empty_error(from_front));
    }
    let index = if from_front { 0 } else { count - 1 };
    Ok(Some(element_at(interp, array, index).unwrap_or(Value::Null)))
}

/// A queue yields front-first, a stack top-first — so a stack reverses.
fn linear_order(interp: &mut Interpreter, this: Handle, from_front: bool) -> Vec<Value> {
    let array = field_handle(interp, this, 0);
    let mut values = elements(interp, array);
    if !from_front {
        values.reverse();
    }
    values
}

fn linear_to_array(
    interp: &mut Interpreter,
    args: &[Value],
    from_front: bool,
) -> ExecResult<Option<Value>> {
    let this = receiver(interp, args)?;
    let values = linear_order(interp, this, from_front);
    let copy = interp.alloc_value_array(0);
    with_values(interp, copy, |v| *v = values);
    Ok(Some(Value::Obj(copy)))
}

/// Enumerates a snapshot, so a queue's enumerator is not disturbed by the
/// `Dequeue` that a loop body might perform.
fn linear_enumerator(
    interp: &mut Interpreter,
    args: &[Value],
    from_front: bool,
    enumerator_type: &str,
) -> ExecResult<Option<Value>> {
    let this = receiver(interp, args)?;
    let values = linear_order(interp, this, from_front);
    let snapshot = interp.alloc_value_array(0);
    with_values(interp, snapshot, |v| *v = values);
    Ok(Some(new_enumerator(interp, enumerator_type, snapshot)))
}


// -- KeyValuePair<K,V> -------------------------------------------------------

fn register_key_value_pair(interp: &mut Interpreter) {
    interp.register_native(key("KeyValuePair`2", ".ctor(!0,!1)"), |i, a| {
        let this = receiver(i, a)?;
        let k = arg(i, a, 1)?;
        let v = arg(i, a, 2)?;
        set_field(i, this, 0, k);
        set_field(i, this, 1, v);
        Ok(None)
    });
    interp.register_native(key("KeyValuePair`2", "get_Key()"), |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(field(i, this, 0)))
    });
    interp.register_native(key("KeyValuePair`2", "get_Value()"), |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(field(i, this, 1)))
    });
    interp.register_native(key("KeyValuePair`2", "Deconstruct(!0&,!1&)"), |i, a| {
        let this = receiver(i, a)?;
        let k = field(i, this, 0);
        let v = field(i, this, 1);
        for (slot, value) in [(1usize, k), (2usize, v)] {
            if let Some(Value::Ref(target)) = a.get(slot) {
                let target = target.clone();
                i.store_indirect_public(target, value)?;
            }
        }
        Ok(None)
    });
    interp.register_native(key("KeyValuePair`2", "ToString()"), |i, a| {
        let this = receiver(i, a)?;
        let k = field(i, this, 0);
        let v = field(i, this, 1);
        let text = format!("[{}, {}]", display(i, &k), display(i, &v));
        Ok(Some(string_value(i, &text)))
    });
}

pub(crate) fn new_key_value_pair(interp: &mut Interpreter, k: Value, v: Value) -> Value {
    let Some(type_id) = named_type(interp, "KeyValuePair`2") else {
        return Value::Null;
    };
    let handle = interp.alloc_object(type_id);
    set_field(interp, handle, 0, k);
    set_field(interp, handle, 1, v);
    Value::Obj(handle)
}

// -- enumerators -------------------------------------------------------------

const ENUM_SOURCE: usize = 0;
const ENUM_INDEX: usize = 1;
const ENUM_CURRENT: usize = 2;

pub(crate) fn named_type(interp: &Interpreter, name: &str) -> Option<TypeId> {
    interp.loader.registry.find_type_by_name(&format!("{GENERIC}.{name}"))
}

/// Allocates an enumerator positioned before the first element.
///
/// `source` is whatever the enumerator walks: a values array for the
/// sequence-shaped collections, or the dictionary itself, whose enumerator
/// pairs the parallel key and value arrays.
pub(crate) fn new_enumerator(interp: &mut Interpreter, type_name: &str, source: Handle) -> Value {
    let Some(type_id) = named_type(interp, type_name) else {
        return Value::Null;
    };
    let handle = interp.alloc_object(type_id);
    set_field(interp, handle, ENUM_SOURCE, Value::Obj(source));
    set_field(interp, handle, ENUM_INDEX, Value::I32(-1));
    set_field(interp, handle, ENUM_CURRENT, Value::Null);
    Value::Obj(handle)
}

fn new_view(interp: &mut Interpreter, type_name: &str, source: Handle) -> Value {
    let Some(type_id) = named_type(interp, type_name) else {
        return Value::Null;
    };
    let handle = interp.alloc_object(type_id);
    set_field(interp, handle, 0, Value::Obj(source));
    Value::Obj(handle)
}

fn register_enumerators(interp: &mut Interpreter) {
    // Every sequence-shaped enumerator walks a values array directly.
    for type_name in [
        "List`1+Enumerator",
        "HashSet`1+Enumerator",
        "Queue`1+Enumerator",
        "Stack`1+Enumerator",
        "Dictionary`2+KeyCollection+Enumerator",
        "Dictionary`2+ValueCollection+Enumerator",
    ] {
        interp.register_native(key(type_name, "MoveNext()"), |i, a| {
            let this = receiver(i, a)?;
            let source = field_handle(i, this, ENUM_SOURCE);
            let next = field(i, this, ENUM_INDEX).as_i32().unwrap_or(-1) + 1;
            let count = element_count(i, source) as i32;
            set_field(i, this, ENUM_INDEX, Value::I32(next));
            if next >= count {
                set_field(i, this, ENUM_CURRENT, Value::Null);
                return Ok(Some(Value::I32(0)));
            }
            let current = element_at(i, source, next as usize).unwrap_or(Value::Null);
            set_field(i, this, ENUM_CURRENT, current);
            Ok(Some(Value::I32(1)))
        });
        register_enumerator_tail(interp, type_name);
    }

    // The dictionary enumerator yields a pair, so it reads two arrays at once.
    interp.register_native(key("Dictionary`2+Enumerator", "MoveNext()"), |i, a| {
        let this = receiver(i, a)?;
        let dict = field_handle(i, this, ENUM_SOURCE);
        let keys = field_handle(i, dict, DICT_KEYS);
        let values = field_handle(i, dict, DICT_VALUES);
        let next = field(i, this, ENUM_INDEX).as_i32().unwrap_or(-1) + 1;
        set_field(i, this, ENUM_INDEX, Value::I32(next));
        if next >= element_count(i, keys) as i32 {
            set_field(i, this, ENUM_CURRENT, Value::Null);
            return Ok(Some(Value::I32(0)));
        }
        let k = element_at(i, keys, next as usize).unwrap_or(Value::Null);
        let v = element_at(i, values, next as usize).unwrap_or(Value::Null);
        let pair = new_key_value_pair(i, k, v);
        set_field(i, this, ENUM_CURRENT, pair);
        Ok(Some(Value::I32(1)))
    });
    register_enumerator_tail(interp, "Dictionary`2+Enumerator");
}

fn register_enumerator_tail(interp: &mut Interpreter, type_name: &str) {
    interp.register_native(key(type_name, "get_Current()"), |i, a| {
        let this = receiver(i, a)?;
        Ok(Some(field(i, this, ENUM_CURRENT)))
    });
    interp.register_native(key(type_name, "Reset()"), |i, a| {
        let this = receiver(i, a)?;
        set_field(i, this, ENUM_INDEX, Value::I32(-1));
        set_field(i, this, ENUM_CURRENT, Value::Null);
        Ok(None)
    });
    // `foreach` compiles to try/finally with a `Dispose` in the finally. These
    // enumerators hold no resource, so it has nothing to do — but it must
    // resolve, or every `foreach` fails on the way out of the loop.
    interp.register_native(key(type_name, "Dispose()"), |_, _| Ok(None));
}

// -- EqualityComparer<T> and Comparer<T> -------------------------------------
//
// A record's compiler-generated `Equals` does not compare fields directly: it
// calls `EqualityComparer<T>.Default.Equals(left.Field, right.Field)` for each
// one. Records therefore do not work at all without these two types, however
// simple the record is.

fn register_comparers(interp: &mut Interpreter) {
    interp.register_native(key("EqualityComparer`1", "get_Default()"), |i, _a| {
        Ok(Some(default_comparer(i, "EqualityComparer`1")))
    });
    interp.register_native(key("Comparer`1", "get_Default()"), |i, _a| {
        Ok(Some(default_comparer(i, "Comparer`1")))
    });

    interp.register_native(key("EqualityComparer`1", "Equals(!0,!0)"), |i, a| {
        let x = arg(i, a, 1)?;
        let y = arg(i, a, 2)?;
        Ok(Some(Value::I32(equals_dispatch(i, &x, &y)? as i32)))
    });
    interp.register_native(key("EqualityComparer`1", "GetHashCode(!0)"), |i, a| {
        let x = arg(i, a, 1)?;
        // .NET hash codes are 32-bit and unspecified; folding the 64-bit hash
        // keeps equal values hashing equally, which is the only guarantee.
        Ok(Some(Value::I32(value_hash(i, &x) as i32)))
    });
    interp.register_native(key("Comparer`1", "Compare(!0,!0)"), |i, a| {
        let x = arg(i, a, 1)?;
        let y = arg(i, a, 2)?;
        Ok(Some(Value::I32(compare_values(i, &x, &y)?)))
    });
}

/// The cached `Default` instance, allocated on first use.
fn default_comparer(interp: &mut Interpreter, type_name: &str) -> Value {
    let Some(type_id) = named_type(interp, type_name) else {
        return Value::Null;
    };
    let slot = interp
        .loader
        .registry
        .ty(type_id)
        .static_fields
        .iter()
        .copied()
        .find(|f| interp.loader.registry.field(*f).name == DEFAULT_COMPARER_FIELD);
    let Some(slot) = slot else {
        // No cache slot: correctness does not depend on identity, so a fresh
        // instance still behaves correctly.
        return Value::Obj(interp.alloc_object(type_id));
    };
    if let Value::Obj(h) = interp.loader.static_value(slot) {
        if !h.is_null() {
            return Value::Obj(*h);
        }
    }
    let handle = interp.alloc_object(type_id);
    *interp.loader.static_value_mut(slot) = Value::Obj(handle);
    Value::Obj(handle)
}

/// Equality that honours a user-declared `Equals` override.
///
/// A record whose field is itself a record must compare that field by *its*
/// generated `Equals`, not by reference. Structural comparison alone would
/// report two equal nested records as different, so a managed override is
/// called when the receiver declares one.
fn equals_dispatch(interp: &mut Interpreter, x: &Value, y: &Value) -> ExecResult<bool> {
    if let Value::Obj(h) = x {
        if !h.is_null() && interp.heap.get_as::<ClrString>(*h).is_none() {
            if let Some(type_id) = interp.type_of(*h) {
                if let Some(method) = find_equals(interp, type_id) {
                    let result = interp.invoke(method, vec![x.clone(), y.clone()])?;
                    return Ok(result.and_then(|v| v.as_i32()).unwrap_or(0) != 0);
                }
            }
        }
    }
    Ok(values_equal(interp, x, y))
}

/// Finds a user-declared `Equals` taking one argument, if any.
fn find_equals(interp: &Interpreter, type_id: TypeId) -> Option<MethodId> {
    for t in interp.loader.registry.base_chain(type_id) {
        for m in &interp.loader.registry.ty(t).methods {
            let info = interp.loader.registry.method(*m);
            if info.name == "Equals"
                && info.signature.params.len() == 1
                && matches!(info.kind, MethodKind::Il(_))
            {
                return Some(*m);
            }
        }
    }
    None
}

/// Ordering for `Comparer<T>.Default`: numeric for numbers, ordinal for
/// strings. Anything else is refused rather than ordered arbitrarily.
fn compare_values(interp: &mut Interpreter, x: &Value, y: &Value) -> ExecResult<i32> {
    if let (Some(a), Some(b)) = (as_number(x), as_number(y)) {
        return Ok(match a.partial_cmp(&b) {
            Some(core::cmp::Ordering::Less) => -1,
            Some(core::cmp::Ordering::Greater) => 1,
            _ => 0,
        });
    }
    if let (Value::Obj(a), Value::Obj(b)) = (x, y) {
        let (Some(sa), Some(sb)) = (
            interp.heap.get_as::<ClrString>(*a).map(|s| s.units.clone()),
            interp.heap.get_as::<ClrString>(*b).map(|s| s.units.clone()),
        ) else {
            return Err(invalid_operation(
                "Comparer<T>.Default on this runtime orders numbers and strings only.",
            ));
        };
        return Ok(match sa.cmp(&sb) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Greater => 1,
            core::cmp::Ordering::Equal => 0,
        });
    }
    Err(invalid_operation(
        "Comparer<T>.Default on this runtime orders numbers and strings only.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interpreter() -> Interpreter {
        let mut i = Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
        crate::install(&mut i);
        i
    }

    #[test]
    fn equal_values_hash_equally_across_widths() {
        let i = interpreter();
        assert!(values_equal(&i, &Value::I32(7), &Value::I64(7)));
        assert_eq!(value_hash(&i, &Value::I32(7)), value_hash(&i, &Value::I64(7)));
        // A double holding an integral value must agree with the integer.
        assert!(values_equal(&i, &Value::F(7.0), &Value::I32(7)));
        assert_eq!(value_hash(&i, &Value::F(7.0)), value_hash(&i, &Value::I32(7)));
    }

    #[test]
    fn strings_compare_and_hash_by_content_not_identity() {
        let mut i = interpreter();
        let a = Value::Obj(i.alloc_string("hello"));
        // A second allocation of the same text, bypassing the intern table.
        let b = Value::Obj(i.alloc_clr_string(ClrString {
            units: "hello".encode_utf16().collect(),
        }));
        assert_ne!(a, b, "distinct handles");
        assert!(values_equal(&i, &a, &b));
        assert_eq!(value_hash(&i, &a), value_hash(&i, &b));
    }

    #[test]
    fn the_collection_types_are_registered() {
        let i = interpreter();
        for k in [
            "System.Collections.Generic.List`1::Add(!0)",
            "System.Collections.Generic.List`1::GetEnumerator()",
            "System.Collections.Generic.List`1+Enumerator::MoveNext()",
            "System.Collections.Generic.Dictionary`2::TryGetValue(!0,!1&)",
            "System.Collections.Generic.HashSet`1::Add(!0)",
            "System.Collections.Generic.Queue`1::Enqueue(!0)",
            "System.Collections.Generic.Stack`1::Push(!0)",
        ] {
            assert!(i.has_native(k), "missing native binding: {k}");
        }
    }
}
