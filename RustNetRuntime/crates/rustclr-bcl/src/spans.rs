//! `Span<T>` and `ReadOnlySpan<T>` over an array.
//!
//! A span is a window: something to look at, an offset into it, and a length.
//! This runtime can represent all three when the thing being looked at is a
//! managed array, which is what collection expressions, `AsSpan` and the
//! spread operator produce. It cannot when the memory is raw — `stackalloc`
//! has nowhere to point — so that still refuses. See `docs/limitations.md`.
//!
//! # Two shapes
//!
//! A span value here is a handle to one of two things:
//!
//! * a **string**, which is the older representation and the one the `s + c`
//!   concatenation path uses — there the string *is* the span, and
//!   [`crate::strings`] reads it directly;
//! * a **window object**, with the array, the start and the length in its
//!   three slots.
//!
//! Every accessor below takes both, because a `ReadOnlySpan<char>` really can
//! arrive as either and refusing one of them would break string building.

use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;

use rustclr_core::{
    ByRef, ClrArray, ClrExceptionKind, ClrString, ExecResult, ExecutionError, Interpreter, RawPtr,
    Value,
};
use rustclr_gc::Handle;

use crate::collections::{field, set_field};
use crate::support::arg;

const SPAN: &str = "System.Span`1";
const READONLY_SPAN: &str = "System.ReadOnlySpan`1";
const MEMORY: &str = "System.Memory`1";
const READONLY_MEMORY: &str = "System.ReadOnlyMemory`1";

/// Slots of a span window object.
const SOURCE: usize = 0;
/// Start, in *elements* of the source array — or in bytes when `WIDTH` says the
/// source is raw memory.
const START: usize = 1;
const LENGTH: usize = 2;
/// Bytes per element, and zero when the source is a typed array whose own
/// element width already says it. Non-zero means `stackalloc` memory, where the
/// buffer is bytes and only `T` knows how many make an element.
const WIDTH: usize = 3;

pub fn register(interp: &mut Interpreter) {
    for name in [SPAN, READONLY_SPAN, MEMORY, READONLY_MEMORY] {
        // `new Span<T>(T[])`. The typed key is tried before the arity one, so
        // this wins over the `.ctor/1` that `strings` registers for the
        // character form without displacing it.
        for ctor in [".ctor(!0[])", ".ctor(!0[],int,int)"] {
            interp.register_native(key(name, ctor), |i, a| {
                let array = arg(i, a, 1)?;
                let Some(handle) = array.as_handle().filter(|h| !h.is_null()) else {
                    return Err(ExecutionError::null_reference());
                };
                let available = array_length(i, handle)?;
                let (start, length) = if a.len() > 3 {
                    (arg(i, a, 2)?.as_i32().unwrap_or(0), arg(i, a, 3)?.as_i32().unwrap_or(0))
                } else {
                    (0, available)
                };
                bounds(start, length, available)?;
                let window = new_window(i, handle, start, length)?;
                write_this(i, a, window)
            });
        }

        // `(Span<T>)array` and `array.AsSpan()` are the same window.
        interp.register_native(key(name, "op_Implicit(!0[])"), |i, a| {
            let Some(handle) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()) else {
                return Ok(Some(Value::Null));
            };
            match array_length(i, handle) {
                Ok(length) => Ok(Some(Value::Obj(new_window(i, handle, 0, length)?))),
                // Already a span, or a string standing for one: identity.
                Err(_) => Ok(Some(Value::Obj(handle))),
            }
        });

        interp.register_native(key(name, "get_Length()"), |i, a| {
            let (_, _, length) = window_of(i, a, 0)?;
            Ok(Some(Value::I32(length)))
        });
        interp.register_native(key(name, "get_IsEmpty()"), |i, a| {
            let (_, _, length) = window_of(i, a, 0)?;
            Ok(Some(Value::I32((length == 0) as i32)))
        });

        // The indexer returns `ref T`, so the caller does its own `ldind` or
        // `stind`. A path to the element is exactly what `ByRef` is for.
        interp.register_native(key(name, "get_Item(int)"), |i, a| {
            let (source, start, length) = window_of(i, a, 0)?;
            let index = arg(i, a, 1)?.as_i32().unwrap_or(0);
            if index < 0 || index >= length {
                return Err(out_of_range());
            }
            match source {
                Source::Array(array) => Ok(Some(Value::Ref(ByRef::ArrayElement {
                    array,
                    index: (start + index) as u32,
                }))),
                // A `ref T` into raw memory is a pointer. The caller does its
                // own `ldind`/`stind` on it, and those already know the width
                // from the instruction — which is why nothing here has to.
                Source::Raw { buffer, width } => Ok(Some(Value::Ptr(RawPtr {
                    buffer,
                    offset: (start + index * width) as i64,
                }))),
                // A span over a string is read-only and has no element to
                // point at, so the character comes back by value. Only a
                // writer would notice, and a `ReadOnlySpan` has none.
                Source::Text(units) => {
                    let unit = units.get((start + index) as usize).copied().unwrap_or(0);
                    Ok(Some(Value::I32(unit as i32)))
                }
            }
        });

        for member in ["Slice(int)", "Slice(int,int)"] {
            interp.register_native(key(name, member), |i, a| {
                let (source, start, length) = window_of(i, a, 0)?;
                let offset = arg(i, a, 1)?.as_i32().unwrap_or(0);
                let taken = if a.len() > 2 {
                    arg(i, a, 2)?.as_i32().unwrap_or(0)
                } else {
                    length - offset
                };
                bounds(offset, taken, length)?;
                match source {
                    Source::Array(array) => {
                        Ok(Some(Value::Obj(new_window(i, array, start + offset, taken)?)))
                    }
                    Source::Raw { buffer, width } => Ok(Some(Value::Obj(new_raw_window(
                        i,
                        buffer,
                        start + offset * width,
                        taken,
                        width,
                    )?))),
                    Source::Text(_) => Err(ExecutionError::MissingImplementation(
                        "slicing a span over a string".into(),
                    )),
                }
            });
        }

        interp.register_native(key(name, "CopyTo/1"), |i, a| {
            let (from, from_start, from_length) = window_of(i, a, 0)?;
            let (to, to_start, to_length) = window_of(i, a, 1)?;
            if from_length > to_length {
                return Err(ExecutionError::exception(
                    ClrExceptionKind::Argument,
                    "Destination is too short.",
                ));
            }
            let (Source::Array(from), Source::Array(to)) = (from, to) else {
                return Err(ExecutionError::MissingImplementation(
                    "copying to or from a span over a string".into(),
                ));
            };
            for n in 0..from_length {
                let value = element(i, from, from_start + n);
                i.heap.with_mut::<ClrArray, _>(to, |arr| {
                    arr.storage.set((to_start + n) as usize, &value)
                });
            }
            Ok(None)
        });

        interp.register_native(key(name, "ToArray()"), |i, a| {
            let (source, start, length) = window_of(i, a, 0)?;
            let Source::Array(array) = source else {
                return Err(ExecutionError::MissingImplementation(
                    "ToArray on a span over a string".into(),
                ));
            };
            let values: Vec<Value> = (0..length).map(|n| element(i, array, start + n)).collect();
            let copy = i.alloc_value_array(values.len());
            for (n, v) in values.into_iter().enumerate() {
                i.heap.with_mut::<ClrArray, _>(copy, |arr| arr.storage.set(n, &v));
            }
            Ok(Some(Value::Obj(copy)))
        });
    }

    // -- reinterpreting references ---------------------------------------
    //
    // `Unsafe.As`, `Unsafe.Add` and `MemoryMarshal.CreateReadOnlySpan` are how
    // .NET 10 lowers `params` and `Task.WaitAll(a, b)`: the arguments go into
    // an `InlineArray<N><T>` buffer, and a span is made over it.
    //
    // A managed reference here is a *path* to a slot, so "reinterpret" and
    // "advance" mean something this runtime can do exactly: `As` keeps the
    // path, and `Add` walks it to the nth field of whatever it names.

    for member in ["As(!!0&)", "As/1", "AsRef(!!0&)", "AsRef/1"] {
        interp.register_native(key("System.Runtime.CompilerServices.Unsafe", member), |_i, a| {
            // Identity: the path is unchanged, only the type it is read at.
            Ok(Some(args_raw(a, 0).unwrap_or(Value::Null)))
        });
    }

    for member in ["Add(!!0&,int)", "Add/2"] {
        interp.register_native(key("System.Runtime.CompilerServices.Unsafe", member), |i, a| {
            let step = arg(i, a, 1)?.as_i32().unwrap_or(0);
            let base = args_raw(a, 0).unwrap_or(Value::Null);
            Ok(Some(advance(base, step)?))
        });
    }

    for member in ["CreateReadOnlySpan/2", "CreateSpan/2"] {
        interp.register_native(key("System.Runtime.InteropServices.MemoryMarshal", member), |i, a| {
            let length = arg(i, a, 1)?.as_i32().unwrap_or(0).max(0);
            let base = args_raw(a, 0).unwrap_or(Value::Null);
            span_over_reference(i, base, length)
        });
    }

    // `Memory<T>` and its span are the same window, so this is the identity.
    for name in [MEMORY, READONLY_MEMORY] {
        for member in ["get_Span()", "get_Memory()"] {
            interp.register_native(key(name, member), |i, a| Ok(Some(arg(i, a, 0)?)));
        }
    }

    // `array.AsSpan()` / `AsMemory()`, with or without a range. Static, so
    // argument zero is the array rather than a receiver.
    for member in [
        "AsSpan/1", "AsSpan/2", "AsSpan/3", "AsMemory/1", "AsMemory/2", "AsMemory/3",
    ] {
        interp.register_native(key("System.MemoryExtensions", member), |i, a| {
            let Some(handle) = arg(i, a, 0)?.as_handle().filter(|h| !h.is_null()) else {
                return Ok(Some(Value::Null));
            };
            let available = array_length(i, handle)?;
            let start = if a.len() > 1 { arg(i, a, 1)?.as_i32().unwrap_or(0) } else { 0 };
            let length = if a.len() > 2 {
                arg(i, a, 2)?.as_i32().unwrap_or(0)
            } else {
                available - start
            };
            bounds(start, length, available)?;
            Ok(Some(Value::Obj(new_window(i, handle, start, length)?)))
        });
    }

    // `ReadOnlySpan<char> x = ['a', 'b']` compiles to a blob in the image plus
    // this call — the same shape as `InitializeArray`, handed back as a span
    // rather than written into an array the caller already made.
    interp.register_native(
        "System.Runtime.CompilerServices.RuntimeHelpers::CreateSpan/1",
        create_span,
    );

    // `Unsafe.InitBlock` and `Unsafe.CopyBlock` are `initblk` and `cpblk` under
    // another name, so they go to the same place the instructions do rather
    // than to a second implementation that could drift from it.
    interp.register_native("System.Runtime.CompilerServices.Unsafe::InitBlock/3", |i, a| {
        let count = arg(i, a, 2)?.as_i64().unwrap_or(0).max(0) as usize;
        let fill = arg(i, a, 1)?.as_i32().unwrap_or(0) as u8;
        let to = arg(i, a, 0)?;
        i.fill_block_public(to, fill, count)?;
        Ok(None)
    });
    interp.register_native("System.Runtime.CompilerServices.Unsafe::CopyBlock/3", |i, a| {
        let count = arg(i, a, 2)?.as_i64().unwrap_or(0).max(0) as usize;
        let from = arg(i, a, 1)?;
        let to = arg(i, a, 0)?;
        i.copy_block_public(to, from, count)?;
        Ok(None)
    });
}

/// `new Span<T>(void* pointer, int length)` — a span over `stackalloc` memory.
///
/// The window records the element width, because the buffer cannot supply it:
/// `localloc` allocates bytes, and `T` is what says how many of them make an
/// element. The width comes from the call site's `TypeSpec`, which is the only
/// place it is written down once framework generics are erased.
pub(crate) fn raw_span(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let Some(Value::Ptr(pointer)) = args.get(1).cloned() else {
        return Err(ExecutionError::null_reference());
    };
    let length = arg(interp, args, 2)?.as_i32().unwrap_or(0);

    let Some(element) = interp.current_type_arguments().first().copied() else {
        return Err(ExecutionError::Unsupported(
            "a span over stackalloc memory whose element type the call site did not name".into(),
        ));
    };
    let width = interp.size_of_public(element).max(1) as i32;

    let window = new_raw_window(interp, pointer.buffer, pointer.offset as i32, length, width)?;
    write_this(interp, args, window)
}

fn key(type_name: &str, member: &str) -> &'static str {
    Box::leak(format!("{type_name}::{member}").into_boxed_str())
}

/// Every element of a span, as values.
///
/// `Task.WaitAll(a, b)` arrives holding one of these rather than an array —
/// .NET 10 lowers it through an `InlineArray2<Task>` — so anything that used to
/// read a params array has to read a span too.
pub(crate) fn values(interp: &mut Interpreter, span: &Value) -> Option<Vec<Value>> {
    let handle = span.as_handle().filter(|h| !h.is_null())?;
    // Only a window object is a span here; an array or a string reaching this
    // is something else's argument, and the caller has its own way to read it.
    let width = field(interp, handle, WIDTH).as_i32().unwrap_or(-1);
    if width < 0 || interp.heap.with::<ClrArray, _>(handle, |_| ()).is_some() {
        return None;
    }
    let (source, start, length) = window_of(interp, &[span.clone()], 0).ok()?;
    let Source::Array(array) = source else { return None };
    Some((0..length).map(|n| element(interp, array, start + n)).collect())
}

/// Argument `n` without dereferencing it.
///
/// [`arg`] loads through a managed pointer, which is right almost everywhere
/// and wrong here: these take the *reference* as their subject.
fn args_raw(args: &[Value], n: usize) -> Option<Value> {
    args.get(n).cloned()
}

/// Walks a reference `step` elements along.
///
/// A reference into an array moves by index. A reference to anything else is
/// read as naming the first element of a buffer whose elements are its fields,
/// which is what an `InlineArray<N><T>` is: `Unsafe.As` hands over the buffer
/// and `Unsafe.Add(n)` is asking for its nth slot.
fn advance(base: Value, step: i32) -> ExecResult<Value> {
    Ok(match base {
        Value::Ref(ByRef::ArrayElement { array, index }) => Value::Ref(ByRef::ArrayElement {
            array,
            index: (index as i32 + step).max(0) as u32,
        }),
        Value::Ref(ByRef::StructField { base, slot }) => Value::Ref(ByRef::StructField {
            base,
            slot: (slot as i32 + step).max(0) as u32,
        }),
        Value::Ref(other) => Value::Ref(ByRef::StructField {
            base: alloc::boxed::Box::new(other),
            slot: step.max(0) as u32,
        }),
        Value::Ptr(p) => Value::Ptr(p.offset_by(step as i64)),
        _ => {
            return Err(ExecutionError::Unsupported(
                "advancing something that is not a reference".into(),
            ))
        }
    })
}

/// `MemoryMarshal.CreateReadOnlySpan(ref first, length)`.
///
/// **A copy, not a view.** .NET makes a window onto the caller's storage; this
/// reads `length` elements out and makes a window onto an array of them. The
/// two differ only if the source changes while the span is alive, and the
/// callers that reach this — `params` lowering and `Task.WaitAll(a, b)` — build
/// the buffer immediately before and never touch it again. A `ReadOnlySpan`
/// cannot be written through, so the copy cannot be observed from that side
/// either. Anything that needs a genuine view over a struct's fields is not
/// served by this.
fn span_over_reference(
    interp: &mut Interpreter,
    base: Value,
    length: i32,
) -> ExecResult<Option<Value>> {
    let mut values = Vec::with_capacity(length as usize);
    for n in 0..length {
        let element = advance(base.clone(), n)?;
        values.push(match element {
            Value::Ref(r) => interp.load_indirect_public(r)?,
            Value::Ptr(p) => interp.read_pointer(p, 8)?,
            other => other,
        });
    }
    let array = interp.alloc_value_array(values.len());
    for (n, v) in values.into_iter().enumerate() {
        interp.heap.with_mut::<ClrArray, _>(array, |a| a.storage.set(n, &v));
    }
    Ok(Some(Value::Obj(new_window(interp, array, 0, length)?)))
}

/// What a span is looking at.
enum Source {
    /// A typed array; `start` counts elements.
    Array(Handle),
    /// Raw `stackalloc` bytes; `start` counts bytes and `width` gives the
    /// element size, because the buffer itself only knows about bytes.
    Raw { buffer: Handle, width: i32 },
    /// UTF-16 code units, which is what a span over a string indexes.
    Text(Vec<u16>),
}

fn element(interp: &mut Interpreter, array: Handle, index: i32) -> Value {
    interp
        .heap
        .with::<ClrArray, _>(array, |a| a.storage.get(index as usize))
        .flatten()
        .unwrap_or(Value::Null)
}

/// Reads argument `index` as a span: source, start and length.
fn window_of(
    interp: &mut Interpreter,
    args: &[Value],
    index: usize,
) -> ExecResult<(Source, i32, i32)> {
    let value = arg(interp, args, index)?;
    let Some(handle) = value.as_handle().filter(|h| !h.is_null()) else {
        // `default(Span<T>)` is empty rather than absent, exactly as in .NET.
        return Ok((Source::Array(Handle::NULL), 0, 0));
    };

    // A string standing for a span over its own characters.
    if let Some(units) = interp.heap.with::<ClrString, _>(handle, |s| s.units.clone()) {
        let length = units.len() as i32;
        return Ok((Source::Text(units), 0, length));
    }
    // A bare array: the whole of it.
    if let Ok(length) = array_length(interp, handle) {
        return Ok((Source::Array(handle), 0, length));
    }
    // A window object.
    let start = field(interp, handle, START).as_i32().unwrap_or(0);
    let length = field(interp, handle, LENGTH).as_i32().unwrap_or(0);
    let source = field(interp, handle, SOURCE).as_handle().unwrap_or(Handle::NULL);
    let width = field(interp, handle, WIDTH).as_i32().unwrap_or(0);
    let source = if width > 0 {
        Source::Raw { buffer: source, width }
    } else {
        Source::Array(source)
    };
    Ok((source, start, length))
}

fn array_length(interp: &mut Interpreter, handle: Handle) -> ExecResult<i32> {
    interp
        .heap
        .with::<ClrArray, _>(handle, |a| a.len() as i32)
        .ok_or_else(|| ExecutionError::MissingImplementation("not an array".into()))
}

fn new_window(
    interp: &mut Interpreter,
    array: Handle,
    start: i32,
    length: i32,
) -> ExecResult<Handle> {
    let Some(type_id) = interp.loader.registry.find_type_by_name(SPAN) else {
        return Err(ExecutionError::MissingImplementation(
            "Span`1 is not registered".into(),
        ));
    };
    let window = interp.alloc_object(type_id);
    set_field(interp, window, SOURCE, Value::Obj(array));
    set_field(interp, window, START, Value::I32(start));
    set_field(interp, window, LENGTH, Value::I32(length));
    set_field(interp, window, WIDTH, Value::I32(0));
    Ok(window)
}

/// A window onto raw memory: start is in bytes and `width` is the element size.
fn new_raw_window(
    interp: &mut Interpreter,
    buffer: Handle,
    start: i32,
    length: i32,
    width: i32,
) -> ExecResult<Handle> {
    let window = new_window(interp, buffer, start, length)?;
    set_field(interp, window, WIDTH, Value::I32(width));
    Ok(window)
}

/// Writes a constructed span through `this`, which is a managed pointer.
///
/// `newobj` reads the same slot back out of the cell it made, so returning
/// nothing serves both the `newobj` and the `ldloca; call .ctor` shapes.
fn write_this(
    interp: &mut Interpreter,
    args: &[Value],
    window: Handle,
) -> ExecResult<Option<Value>> {
    match args.first() {
        Some(Value::Ref(target)) => {
            interp.store_indirect_public(target.clone(), Value::Obj(window))?;
            Ok(None)
        }
        _ => Ok(Some(Value::Obj(window))),
    }
}

fn bounds(start: i32, length: i32, available: i32) -> ExecResult<()> {
    if start < 0 || length < 0 || start + length > available {
        return Err(out_of_range());
    }
    Ok(())
}

fn out_of_range() -> ExecutionError {
    ExecutionError::exception(
        ClrExceptionKind::IndexOutOfRange,
        "Index was outside the bounds of the span.",
    )
}

/// How many bytes of an RVA field's blob actually belong to it.
///
/// Roslyn gives every array initialiser a field whose type is a synthetic
/// struct named `__StaticArrayInitTypeSize=N` — one per distinct size, and the
/// size is in the name because that is the only place it is written down
/// without reading `ClassLayout`. `None` when the name says nothing, which
/// leaves the caller with the whole blob and the same behaviour as before.
fn blob_size(
    interp: &Interpreter,
    assembly: rustclr_core::AssemblyId,
    token: rustclr_core::metadata::Token,
) -> Option<usize> {
    let field = interp
        .loader
        .resolve_field_token(interp.loader.assembly(assembly), token)?;
    // Not `field_type`: `<PrivateImplementationDetails>` and its nested structs
    // are not loaded as runtime types, so that stays `INVALID`. The signature
    // still names the `TypeDef`, which resolves on its own.
    let signature = interp.loader.registry.field(field).signature.clone();
    let rustclr_core::metadata::TypeSig::ValueType(type_token) = signature.unwrap_modifiers()
    else {
        return None;
    };
    let type_id = interp
        .loader
        .resolve_type_token(interp.loader.assembly(assembly), *type_token)?;
    let name = &interp.loader.registry.ty(type_id).name;
    // `__StaticArrayInitTypeSize=4_Align=2` — the trailing `=` belongs to the
    // alignment, so take the digits after `Size=` rather than the last field.
    // Reading the wrong one gave a two-character span a length of one.
    let digits = name.split_once("Size=")?.1;
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    digits[..end].parse().ok()
}

/// `RuntimeHelpers.CreateSpan<T>(fieldHandle)`.
fn create_span(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let raw = arg(interp, args, 0)?.as_i64().unwrap_or(0) as u32;
    let token = rustclr_core::metadata::Token(raw);
    let Some(assembly) = interp.current_assembly() else {
        return Ok(Some(Value::Null));
    };
    let Some(data) = interp.loader.field_initial_data(assembly, token).map(|d| d.to_vec()) else {
        return Ok(Some(Value::Null));
    };

    // The blob runs to the end of its section, because metadata does not record
    // an RVA field's length — `InitializeArray` gets away with that because the
    // array it fills bounds the copy, and a span has nothing to bound it. The
    // size comes from the field's *type*: Roslyn emits one synthetic struct per
    // distinct size and puts the size in its name.
    let size = blob_size(interp, assembly, token).unwrap_or(data.len());
    let data = &data[..size.min(data.len())];

    // The element width is not in the field handle either. `CreateSpan` is
    // emitted for a collection expression of a primitive, and `char` is the one
    // this runtime meets — two bytes each, little-endian, exactly as
    // `InitializeArray` reads them.
    let count = data.len() / 2;
    let array = interp.alloc_value_array(count);
    for (n, pair) in data.chunks_exact(2).enumerate() {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        interp
            .heap
            .with_mut::<ClrArray, _>(array, |a| a.storage.set(n, &Value::I32(unit as i32)));
    }
    Ok(Some(Value::Obj(new_window(interp, array, 0, count as i32)?)))
}
