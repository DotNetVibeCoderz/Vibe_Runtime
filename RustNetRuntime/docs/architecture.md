# Architecture

How a `.dll` on disk becomes a running program.

---

## The path a program takes

```
  Program.cs
      │  Roslyn (the .NET SDK — RustCLR does not compile C#)
      ▼
  Program.dll  ── PE container, ECMA-335 metadata, IL
      │
      ▼
  rustclr-metadata ── PE headers, section table, metadata streams,
      │               tables, signature blobs, method bodies
      ▼
  Loader ──────────► TypeRegistry   types · methods · fields · vtables
      │                    ▲
      ▼                    │
  Interpreter ─────────────┘
      │
      ├──► rustclr-gc        allocation, collection
      ├──► rustclr-bcl       framework methods, implemented in Rust
      └──► rustclr-interop   P/Invoke into native libraries
```

Every stage is a separate crate, and each one is useful on its own:
`rustclr-metadata` is a working assembly reader with no runtime attached, and
`rustclr-gc` is a collector with no knowledge of .NET.

---

## The decision that makes this work

A .NET program does not contain the framework. It contains *references* to it —
`System.Console::WriteLine(string)` appears in the metadata as a `MemberRef`
with no body anywhere in the file.

CoreCLR resolves those references against a managed CoreLib. RustCLR does not
have one, and reimplementing CoreLib in C# would defeat the point.

Instead, the loader turns every unresolved framework reference into an
**internal-call stub** keyed by a canonical name:

```
System.Console::WriteLine(string)
System.String::Concat(string,string)
System.Math::Sqrt(double)
```

`rustclr-bcl` registers a Rust function against each key. When the interpreter
reaches such a method it calls the Rust function directly.

The consequence is the whole premise of the project: **the contract C# was
compiled against is unchanged, and the implementation underneath it is Rust.**
No recompilation, no source changes, no shim assemblies.

When a key has no implementation, the runtime says exactly which one is missing
rather than failing vaguely — and `rustnet verify` reports them all before you
run anything.

---

## rustclr-metadata

Turns bytes into typed, bounds-checked views. It never allocates a managed
object or executes anything.

Two properties matter:

**Nothing panics on malformed input.** Every read goes through a bounds-checked
cursor, so a corrupt or hostile assembly produces a `MetadataError`. This is the
first place the Rust rewrite pays for itself — the equivalent C++ is hand-audited
pointer arithmetic.

**Row sizes are computed, not hard-coded.** ECMA-335 table rows vary in width:
an index column is 2 bytes when its target table has fewer than 2¹⁶ rows and 4
otherwise, and heap indexes widen based on flags in the `#~` header. Rather than
45 hand-written row-size formulas, each table is described as a list of typed
columns and sizes are derived from the actual image.

---

## rustclr-gc

**Handles, not pointers.** Objects are reached through a generation-tagged
handle table:

```
Handle ── index ──► Slot { generation, object, marked, pins }
             │
             └── generation must match, or the handle is invalid
```

Reusing a slot bumps its generation, so a stale handle is *detected* rather than
dereferenced. The cost is one array load per access; the benefit is that the
entire object graph is expressible without `unsafe`, and a use-after-free
becomes a `None` instead of undefined behaviour.

**Collection is a trait.** `Collector` is the seam that makes the GC
replaceable, as the requirements demand. `MarkSweep` is the default;
`NeverCollect` suits short-lived or hard-real-time programs. The runtime holds a
`Box<dyn Collector>` and never depends on which is installed.

**Marking is iterative.** A deep object graph must not blow the native stack —
a real failure mode on microcontrollers. The mark phase uses an explicit
worklist; a 200,000-deep linked list is a test case.

---

## rustclr-core

### Values

The evaluation stack holds the small set ECMA-335 III.1.1 defines: `int32`,
`int64`, `native int`, `F`, object references, managed pointers and unboxed
value types. Narrower integers widen to `int32` on load and truncate on store,
which is why there is no `Value::I8`.

Managed pointers (`&`) are represented *structurally*, not as addresses:

```rust
enum ByRef {
    Local { frame, index },
    Arg { frame, index },
    Field { object, slot },
    Static { type_id, slot },
    ArrayElement { array, index },
}
```

A raw pointer into the GC heap would have to be updated by the collector and
could dangle. This form is always safe to resolve and always current.

### The interpreter loop

The loop is **iterative**. A managed call pushes onto an explicit `Vec<Frame>`
rather than recursing into `execute`. Deep managed recursion therefore exhausts
a configurable frame budget and throws `StackOverflowException`, instead of
aborting the process by overflowing the native stack.

Each method is decoded once into a `CompiledMethod` — instructions, an
offset-to-index map, locals, exception clauses — and cached. That prepass is the
seam `rustclr-jit` plugs into.

### Exception handling

`try`/`catch`/`finally` in IL is not structured control flow. It is a table of
offset ranges plus the `leave` and `endfinally` instructions. The runtime
implements the state machine that connects them: `leave` queues every `finally`
between here and the target and runs them in order before branching; a thrown
exception searches each frame for a matching `catch`, running intervening
`finally` blocks as the stack unwinds.

---

## rustclr-sched

The substrate `Task` and `async`/`await` will sit on: a Michael–Scott lock-free
queue, a multi-producer multi-consumer channel over it, and a thread pool that
drains it. A panicking task faults its handle rather than killing a worker,
mirroring how .NET surfaces a faulted `Task`.

---

## rustclr-interop

Calling a C function whose signature is only known at run time normally needs
libffi. This bridge takes a different route: it classifies each argument into an
ABI slot (integer/pointer or floating point) and dispatches through a table of
concrete function types.

Shapes outside that table are **refused, not guessed**. A mismatched calling
sequence is undefined behaviour, and an error message is strictly better.

`unsafe` appears in exactly two places in the whole runtime — the platform
loader and the call dispatcher — and both are documented with their safety
contracts.

---

## rustclr-jit

The compilation seam, plus the analysis every backend needs first: basic-block
leaders, per-instruction stack depth, and the properties that decide whether a
method is compilable or inlinable.

`X64Backend` turns that analysis into x86-64 machine code for leaf methods doing
integer arithmetic. The evaluation stack is not simulated: because the verifier
knows the depth at every instruction, each stack slot has a statically known
home — the shallowest two in `r14` and `r15`, the rest spilled to the frame — so
most operations become a single register-to-register instruction.

`JitTier` decides *when*: a method is interpreted until it has been called 32
times, then compiled. `InterpreterTier` remains available and is what
`rustnet verify` runs.

Code memory is **write-xor-execute**. A page is mapped readable and writable,
filled, and only then flipped to readable and executable; it is never both at
once. All the `unsafe` in this crate is in `codepage.rs` and the one transmute
that calls a finished page.

The backend declines far more than it takes — anything with calls, arrays,
allocation or exception handling — and `rustnet jit <assembly>` reports which is
which. That is the tiering model working as intended rather than a gap: a
declined method runs exactly as it did before.

---

## Reflection

`System.Type` is an ordinary managed object carrying one field: the id of the
runtime type it describes. The interpreter interns one instance per type, which
is what makes `typeof(int) == typeof(int)` reference equality — a guarantee .NET
gives and real code depends on.

`ldtoken` resolves a *type* token where it executes rather than pushing the raw
number, because a metadata token means nothing outside the assembly that emitted
it and the handle may be consumed somewhere else entirely. Field and method
tokens stay raw; `RuntimeHelpers.InitializeArray` wants the token.

`MethodInfo`, `FieldInfo` and the rest are the same shape: the id of what they
describe plus the type they were reached through.

---

## What is not here

- **A C# compiler.** Roslyn does that. RustCLR consumes IL.
- **Managed CoreLib.** The framework is a contract implemented natively.
- **A backend for anything but leaf integer methods on x86-64.** Arrays, calls,
  AArch64 and RISC-V are Milestone 4's remaining half.

See [limitations.md](limitations.md) for the full list, or run
`rustnet capabilities`, which prints it from the runtime itself.
