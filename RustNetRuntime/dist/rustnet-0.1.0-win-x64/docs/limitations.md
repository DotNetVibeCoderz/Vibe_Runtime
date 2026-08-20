# Limitations

What RustCLR does not do yet, and why. Being straight about this is more useful
than a feature list — and `rustnet capabilities` prints the same information
from the runtime itself, so it cannot drift from the code.

---

## Generics are erased

**What happens:** a generic type is loaded as its open definition and every type
argument becomes `object`. `List<int>` and `List<string>` are the same runtime
type, and `MethodSpec` tokens do not resolve to instantiated methods.

**What breaks:** most real programs. Generic collections, `IEnumerable<T>`,
LINQ, anything with a constrained generic call.

**Why it is like this:** instantiating generics properly means one runtime type
per closed construction, a shared open definition, generic virtual dispatch, and
`constrained.` handling on value types. It is the single largest piece of
remaining work, and doing it half-way would give wrong answers rather than
missing ones.

**Workaround today:** arrays instead of `List<T>`, explicit loops instead of
LINQ. The templates marked *runs on RustCLR* are written this way.

This is [Milestone 2](../Plan.md).

---

## async and await do not run

**What happens:** the compiler-generated state machine loads fine, but nothing
drives `MoveNext`, and `Task` is not implemented.

**Why it is like this:** the scheduler exists — `rustclr-sched` has a lock-free
run queue, channels and a thread pool, all tested — but the managed side is not
wired to it. The missing piece is recognising `IAsyncStateMachine` and
implementing `Task`, `TaskCompletionSource` and the awaiter pattern in RustBCL.

This is [Milestone 3](../Plan.md).

---

## Exception filters are not evaluated

**What happens:** `catch (Exception e) when (e.Message.Contains("x"))` is treated
as non-matching. The exception passes it by rather than being caught.

**Why it is like this:** a filter runs managed code during the first unwind pass,
before the stack is unwound. That needs a re-entrant execution mode mid-dispatch.
Treating filters as non-matching lets the exception escape to an outer handler —
noisy, but correct — rather than swallowing it, which would be silently wrong.

`try`/`catch`/`finally` without a filter works, including nesting, rethrow, and
`finally` blocks running during unwind.

---

## There is no native code generator

**What happens:** every method is interpreted. `rustclr-jit` provides the
`Compiler` trait, the IL verifier and basic-block analysis, but
`InterpreterTier` is the only implementation and it reports every method as
interpreted.

**What this costs:** roughly 1.8 million IL instructions per second on the
conformance suite. Fine for scripts, tools and IoT control loops; not fine for
anything numerically heavy.

The tiering design already accounts for a partial backend —
`Compiler::can_compile` decides per method, so the first emitter only has to
handle leaf integer methods to be useful. This is [Milestone 4](../Plan.md).

---

## Reflection is minimal

`GetType()` returns a type *name*, not a `System.Type` object. There is no
member enumeration, no attribute reading, no `Activator.CreateInstance`.

`ldtoken` pushes the raw metadata token, which is enough for
`RuntimeHelpers.InitializeArray` — the mechanism behind `new int[] { 1, 2, 3 }` —
but not for anything that inspects types at run time.

This is [Milestone 5](../Plan.md).

---

## Unimplemented IL instructions

These are decoded and then reported as unsupported, rather than being silently
skipped:

`localloc` · `cpblk` · `initblk` · `arglist` · `mkrefany` · `refanyval` ·
`refanytype` · `calli` · `jmp`

Multi-dimensional arrays parse but only single-dimension zero-based arrays are
allocated and indexed.

---

## Interop constraints

**Argument count.** Up to six. Raising it means adding arms to the dispatch
table, because each arity needs its own concrete function type.

**Mixed argument kinds.** A call taking both integers and floats is refused. The
table does not model per-position ABI slots, and mis-ordering registers is
undefined behaviour — an error is strictly better than a guess.

**Strings marshal as UTF-8 `char*`.** Wide-string marshalling is not
implemented. A declaration expecting `wchar_t*` will receive the wrong encoding,
so it is refused rather than mis-encoded.

**Structs do not cross the boundary.** Only strings and blittable primitives.

---

## Embedded targets are designed for, not proven

The core crates are written to be `no_std`-friendly, the collector has an
`embedded` profile, the Cargo `embedded` profile optimises for size, and the
metadata reader recognises RISC-V and Arm machine types.

**Nothing has been flashed to real hardware.** Until the core crates build for
`thumbv7em-none-eabihf` and `riscv32imc-unknown-none-elf` without `std`, and the
`iot-gateway` template runs on an actual ESP32, this is a design intent rather
than a claim. This is [Milestone 6](../Plan.md).

---

## CodeGen

**Format Code** is a brace-depth reindenter, not a C# formatter. It fixes
indentation and trailing whitespace; it does not wrap lines, sort usings or
normalise spacing. Anything more needs Roslyn.

**Chat streaming** is not incremental for Claude. The tool loop needs a complete
message before it can run a tool, so the panel shows a working indicator rather
than partial text.

**Image attachments** are passed as file paths, not inlined bitmaps. Jack's
tools read from disk.

**Mobile templates** produce touch-first Avalonia layouts that run on the
desktop. Deploying to Android or iOS needs the Avalonia mobile heads added to
the project — the template says so.

---

## How to find out for yourself

```bash
rustnet capabilities              # what the runtime implements
rustnet verify <assembly>         # what your program would hit
```

`verify` is the honest answer for any specific program: it names every framework
member referenced but not implemented, before you run anything.
