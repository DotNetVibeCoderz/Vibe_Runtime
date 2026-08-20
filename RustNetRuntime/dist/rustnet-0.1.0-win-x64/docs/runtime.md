# The runtime in depth

Notes on the parts of RustCLR whose behaviour is easy to get subtly wrong, and
what this implementation does about each.

For the shape of the system, read [architecture.md](architecture.md) first.

---

## Reading metadata safely

An assembly is untrusted input. The reader treats it that way: every read goes
through a bounds-checked cursor, and a failed read leaves the cursor untouched
so error paths cannot half-consume a structure.

Two details that bite:

**Row widths are per-image.** A table index is 2 bytes when its target has fewer
than 2¹⁶ rows and 4 bytes otherwise; heap indexes widen based on flags in the
`#~` header; coded indexes must fit both a tag and a row number in whatever
width they get. Hard-coding 45 row-size formulas is how these readers rot. Each
table is instead described as typed columns and sizes are computed from the
image, so a large assembly and a small one both decode correctly.

**Child lists are implicit.** Metadata records only where a type's field and
method lists *start*. The end is the next type's start — or the table's row
count for the last type. Getting this wrong silently attributes members to the
wrong type.

### A bug worth recording

The first version of the PE reader skipped 44 bytes (PE32) of the optional
header before `NumberOfRvaAndSizes`. The correct figure is 60, and 76 for PE32+.
The result was that *every* assembly failed to load, with an EOF at a plausible
offset. It was found by running the reader against a real Roslyn-produced
assembly rather than a hand-built fixture — which is why
`crates/rustclr-metadata/tests/real_assembly.rs` exists.

---

## The garbage collector

### Why handles

Objects are addressed by a generation-tagged handle, not a pointer:

```
Handle { index, generation } ──► Slot { generation, object, marked, pins }
```

A handle resolves only when the generations match. Reusing a slot bumps its
generation, so every handle to the previous occupant becomes invalid *and is
detected as invalid*.

The cost is one array load per dereference. What it buys: the object graph
needs no `unsafe`, the collector can reallocate freely without fixing up
interior pointers, and a use-after-free surfaces as `None` rather than as
corrupted memory. For a project whose stated reason to exist is memory safety,
that trade is the whole point.

### Iterative marking

The mark phase uses an explicit worklist, not recursion. A deeply linked
structure — a 200,000-node list is in the test suite — would otherwise overflow
the native stack, which on a microcontroller is a few kilobytes.

### Pinning

Native code holding an object across a P/Invoke needs it to stay put. `pin`
increments a count on the slot, and pinned objects are added to the root set at
collection time. Mark-sweep does not move objects, so pinning only has to
prevent reclamation.

### Roots

The interpreter supplies them: every frame's arguments, locals and evaluation
stack, plus all static fields, interned strings and cached string literals.
Collection only happens between instructions, where no interior state is
mid-update.

---

## Values on the evaluation stack

ECMA-335 III.1.1 defines a deliberately small set: `int32`, `int64`,
`native int`, `F`, object references, managed pointers, and unboxed value types.
Everything narrower — `bool`, `char`, `sbyte`, `short` and their unsigned forms
— widens to `int32` on load and truncates on store. There is no `Value::I8` for
that reason.

### Managed pointers are structural

```rust
enum ByRef {
    Local { frame, index },
    Arg { frame, index },
    Field { object, slot },
    Static { type_id, slot },
    ArrayElement { array, index },
}
```

An address into the GC heap would need collector fix-ups and could dangle. This
form is resolved on use, so it is always current.

It also makes value-type semantics work. C# calls an instance method on a struct
local by pushing `ldloca` — the receiver arrives as a `&`, not a value. Field
assignment on a struct local is `ldloca; ldc; stfld`, which means reading the
struct through the pointer, updating a slot, and writing it back. Missing that
was a real bug: struct field writes went to a null reference until `stfld`
learned to handle a `ByRef` receiver.

---

## Arithmetic

Binary numeric promotion follows III.1.5: the operand pair decides the result
type, integer arithmetic wraps unless the opcode carries `.ovf`, and division by
zero throws rather than trapping.

Two cases that are easy to miss:

- `int.MinValue / -1` overflows. The CLR throws `OverflowException`; the
  hardware would trap. Both `div` and the 64-bit form check for it.
- `int.MinValue % -1` is defined as `0`, not an overflow.

The `.un` comparison forms are unsigned for integers *and* "unordered counts as
true" for floats. That is how C# compiles `!(a < b)`, so getting it wrong breaks
ordinary float comparisons rather than exotic ones.

---

## Exception handling

IL has no structured exception syntax. It has a table of protected ranges and
two instructions, `leave` and `endfinally`. The runtime implements the state
machine that connects them:

**On `leave`** — collect every `finally` whose `try` range contains the current
offset but not the target, queue them innermost-first, clear the evaluation
stack (III.3.55 requires this), and run them in order before branching.

**On a throw** — for each frame, look for a `catch` whose type matches. Run any
intervening `finally` blocks first, then either enter the handler with the
exception as the sole stack entry, or pop the frame and continue.

**On `endfinally`** — advance the queue: run the next `finally`, branch to the
pending `leave` target, or resume propagating the in-flight exception.

Exception filters are the gap. Evaluating one means running managed code
mid-unwind, before the stack has been unwound. They are treated as non-matching,
which lets the exception reach an outer handler — noisy but correct, rather than
silently swallowed.

---

## Calls

**Static and instance calls** resolve the token to a method and push a frame.

**Virtual calls** use the receiver's runtime type. Vtables are laid out at load
time: a derived type inherits its base's slots, then either overrides an
inherited slot — matching name and signature, without `newslot` — or appends a
new one.

**Interface calls** cannot use a slot number, because the interface's numbering
means nothing on the implementing type. They match by name and signature across
the receiver's hierarchy.

**`constrained.`** on a value type boxes the receiver so a virtual call through
an interface reaches the boxed instance.

**Delegates** are constructed by the runtime from a target and a method pointer,
and `Invoke` walks the invocation list. A multicast delegate returns the last
invocation's result, as .NET specifies.

---

## Static constructors

A type's `.cctor` runs on first access to any of its static fields, or on first
instantiation. The state machine is `NotRun → Running → Done`, with `Running`
short-circuiting so a `.cctor` that touches its own type does not recurse
forever. A `.cctor` that throws moves the type to `Failed`, and every later
access rethrows — which is what .NET does.

---

## Strings

`ClrString` stores `Vec<u16>`, not a Rust `String`.

.NET defines `Length`, indexing and `Substring` in UTF-16 code units. Storing
UTF-8 would make each of those either wrong or O(n). The cost is that ASCII text
takes twice the memory; the benefit is that `"\u{1F600}".Length == 2` on RustCLR
exactly as on .NET.

Conversion to Rust strings happens only at the boundary — when writing to the
host, or marshalling across interop.

---

## The native BCL

`rustclr-bcl` registers Rust functions against canonical keys:

```
System.Console::WriteLine(string)      → writes through the Host trait
System.String::Concat(string,string)   → allocates a new ClrString
System.Math::Round(double)             → banker's rounding, as .NET does
```

Two keys are tried per call: the typed form
(`System.Console::WriteLine(string)`) and an arity fallback
(`System.Console::WriteLine/1`). The typed form separates overloads; the
fallback catches methods whose parameters are framework types whose tokens
differ per assembly — `RuntimeHelpers::InitializeArray/2` is the notable one.

Behaviour that is easy to get wrong and is deliberately not:

- `Math.Round` uses banker's rounding. Rust's `f64::round` rounds half away from
  zero, which disagrees with .NET on every `.5`.
- `Math.Abs(int.MinValue)` throws `OverflowException` rather than wrapping.
- `bool.ToString()` is `"True"`, capitalised.
- A `double` with no fractional part prints without a trailing `.0`.

---

## Output goes through the host

Console output, standard input and both clocks go through a `Host` trait, so an
embedder can redirect them. `SystemHost` writes to real stdio; `CaptureHost`
buffers everything, which is what the tests and CodeGen's output panel use.

That indirection is why the conformance suite can assert on a program's exact
output.
