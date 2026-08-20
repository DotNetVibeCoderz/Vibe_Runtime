# rustnet — toolchain reference

Build, run, inspect and verify assemblies on RustCLR.

```bash
cargo build --release        # produces target/release/rustnet
```

---

## run

Execute an assembly's entry point on RustCLR.

```bash
rustnet run <assembly> [--stats] [--trace] [--max-instructions N] [-- args...]
```

| | |
| --- | --- |
| `--stats` | Print execution and heap counters when the program exits |
| `--trace` | Report what was loaded before running |
| `--max-instructions N` | Stop after N IL instructions — a runaway loop fails predictably |
| `-- args...` | Everything after `--` reaches the program as `Main`'s arguments |

```console
$ rustnet run Conformance.dll --stats
checks=37 failures=0

─── execution ──────────────────────────────
  wall clock                  10.505 ms
  IL instructions              19,617
  throughput                 1,867,343 instr/s
  managed calls                 2,167
  native calls                    103
  peak frame depth                 16
─── heap ───────────────────────────────────
  collector               mark-sweep
  allocations                     194
  bytes allocated              10,023
  collections                       0
  objects reclaimed                 0
  live objects                    194
```

An unhandled exception prints the managed stack trace, the way a .NET host does,
and exits with 1.

---

## info

Summarise an assembly's metadata without running it.

```bash
rustnet info <assembly> [--verbose]
```

Reports the assembly name and version, the runtime version string, machine
architecture, whether the image is IL-only, the entry point, row counts for the
metadata tables, and referenced assemblies. `--verbose` adds every type and its
methods.

Useful for answering "what does this thing actually reference?" before trying to
run it.

---

## disasm

Disassemble method bodies to IL.

```bash
rustnet disasm <assembly> [filter]
```

`filter` is a case-insensitive substring match on `Type.Method`.

```console
$ rustnet disasm HelloWorld.dll Program.Add

.method HelloWorld.Program.Add  // maxstack 8, 0 locals, 0 EH clauses
  IL_0000:  ldarg.0
  IL_0001:  ldarg.1
  IL_0002:  add
  IL_0003:  ret
```

Methods with no IL body are listed with why — `internal call (RustBCL)`,
`P/Invoke`, `abstract`, `runtime provided`.

---

## verify

Load an assembly and report everything that will not work.

```bash
rustnet verify <assembly>
```

Two kinds of finding:

**IL problems** — a method whose IL fails verification: a branch into the middle
of an instruction, an evaluation-stack underflow, a stack deeper than the method
header declared.

**Unresolved members** — a framework method the assembly references that RustBCL
does not implement. These are the ones that matter when porting; each is printed
with the exact binding key.

```console
$ rustnet verify MyApp.dll
Verifying MyApp
  BCL  no native implementation for System.Linq.Enumerable::Where(...)
  BCL  no native implementation for System.Collections.Generic.List`1::Add(...)

2 problem(s) found.
```

A clean report is a good predictor that the program will run. Exit code is 0
when nothing was found, 1 otherwise, so it fits in a build script.

---

## build

Compile a C# project with the .NET SDK, optionally running it here afterwards.

```bash
rustnet build [project] [-c Release] [--run]
```

RustCLR does not compile C# — this shells out to `dotnet build` and then, with
`--run`, locates the output assembly and executes it on RustCLR. One command
from source to a RustCLR run.

---

## capabilities

Print what this runtime implements.

```bash
rustnet capabilities
```

The output is generated from the runtime itself — the collector name, the frame
limit, the P/Invoke argument ceiling and the number of registered BCL bindings
are read from live objects, not from a hand-maintained list. It cannot drift
from the code.

---

## Exit codes

| | |
| --- | --- |
| `0` | Success, or the program's own exit code |
| `1` | The program threw, or `verify` found problems |
| `2` | The command line was wrong |
