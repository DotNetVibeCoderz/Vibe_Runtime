//! Intrinsics: Rust natives substituted for methods that have an IL body.
//!
//! A native binding is a method's only implementation. An intrinsic is a
//! second, faster one for a method whose IL stays the definition — so each
//! here handles the shape it was written for and **declines** everything
//! else (`Ok(None)`), and the IL runs. It never guesses at a case it did not
//! anticipate.
//!
//! Every one is checked against the IL it replaces by
//! `crates/rustclr-bcl/tests/intrinsics.rs`, which runs the same workload with
//! intrinsics on and off and fails on any difference, and every one has to
//! earn its place on a benchmark row (`docs/benchmarks.md`). `RUSTCLR_INTRINSICS=0`
//! or `rustnet run --no-intrinsics` turns them all off.
//!
//! ## The channel ring
//!
//! `BufferedChannel<T>` (`managed/RustBCL.Concurrent/Channels.cs`) keeps its
//! items in a ring: `_ring`, `_head`, `_count`. Every item a channel carries
//! goes through `PushBack` and `PopFront`, and each is a dozen field and
//! element accesses — each one a trip through the heap's lock when
//! interpreted. Here each is one look at the object and one at the array.
//! Growing the ring, and a ring of structs (whose `default` this would have
//! to build), are declined.

use rustclr_core::gc::Handle;
use rustclr_core::{ArrayStorage, ClrArray, ClrObject, ExecResult, Interpreter, Value};

const CHANNEL: &str = "System.Threading.Channels.BufferedChannel`1";

pub fn register(interp: &mut Interpreter) {
    interp.register_intrinsic(alloc::format!("{CHANNEL}::PopFront()"), pop_front);
    interp.register_intrinsic(alloc::format!("{CHANNEL}::PushBack(!0)"), push_back);
}

/// The receiver's `_ring`, `_head` and `_count`, and their slots.
struct Ring {
    this: Handle,
    ring: Handle,
    head: i32,
    count: i32,
    head_slot: usize,
    count_slot: usize,
}

fn ring_of(interp: &mut Interpreter, args: &[Value]) -> Option<Ring> {
    let this = args.first()?.as_handle().filter(|h| !h.is_null())?;
    let ty = interp.type_of(this)?;
    let ring_slot = interp.field_slot_named(ty, "_ring")?;
    let head_slot = interp.field_slot_named(ty, "_head")?;
    let count_slot = interp.field_slot_named(ty, "_count")?;
    let (ring, head, count) = interp
        .heap
        .with::<ClrObject, _>(this, |o| {
            Some((o.fields.get(ring_slot)?.as_handle()?, o.fields.get(head_slot)?.as_i32()?, o.fields.get(count_slot)?.as_i32()?))
        })
        .flatten()?;
    if ring.is_null() {
        return None;
    }
    Some(Ring { this, ring, head, count, head_slot, count_slot })
}

/// The zero an element of this storage is reset to; `None` for a ring of
/// structs, whose zero is a struct this does not build.
fn zero_of(storage: &ArrayStorage) -> Option<Value> {
    Some(match storage {
        ArrayStorage::Values(_) => return None,
        ArrayStorage::Refs(_) => Value::Null,
        ArrayStorage::I64(_) | ArrayStorage::U64(_) => Value::I64(0),
        ArrayStorage::F32(_) | ArrayStorage::F64(_) => Value::F(0.0),
        _ => Value::I32(0),
    })
}

fn set_field(interp: &Interpreter, this: Handle, slot: usize, value: i32) {
    interp.heap.with_mut::<ClrObject, _>(this, |o| o.fields[slot] = Value::I32(value));
}

/// `var item = _ring[_head]; _ring[_head] = default; _head = (_head + 1) % _ring.Length; _count--; return item;`
/// Public so `tests/intrinsics.rs` can build a deliberately wrong variant on it.
pub fn pop_front(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Option<Value>>> {
    let Some(r) = ring_of(interp, args) else { return Ok(None) };
    let taken = interp
        .heap
        .with_mut::<ClrArray, _>(r.ring, |a| {
            let len = a.len();
            let index = usize::try_from(r.head).ok().filter(|i| *i < len)?;
            let zero = zero_of(&a.storage)?;
            let item = a.storage.get(index)?;
            a.storage.set(index, &zero);
            Some((item, len))
        })
        .flatten();
    let Some((item, len)) = taken else { return Ok(None) };
    let next = ((r.head as i64 + 1) % len as i64) as i32;
    interp.heap.with_mut::<ClrObject, _>(r.this, |o| {
        o.fields[r.head_slot] = Value::I32(next);
        o.fields[r.count_slot] = Value::I32(r.count.wrapping_sub(1));
    });
    Ok(Some(Some(item)))
}

/// `_ring[(_head + _count) % _ring.Length] = item; _count++;` — when the ring
/// has room. A full ring grows, and that is left to the IL.
fn push_back(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Option<Value>>> {
    let Some(item) = args.get(1) else { return Ok(None) };
    let Some(r) = ring_of(interp, args) else { return Ok(None) };
    let stored = interp
        .heap
        .with_mut::<ClrArray, _>(r.ring, |a| {
            let len = a.len();
            zero_of(&a.storage)?;
            if len == 0 || r.head < 0 || r.count < 0 || r.count as usize >= len {
                return None;
            }
            let index = (r.head as usize + r.count as usize) % len;
            a.storage.set(index, item).then_some(())
        })
        .flatten();
    if stored.is_none() {
        return Ok(None);
    }
    set_field(interp, r.this, r.count_slot, r.count + 1);
    Ok(Some(None))
}
