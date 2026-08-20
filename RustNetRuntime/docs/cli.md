# rustnet — toolchain reference

Build, run, inspect and verify assemblies on RustCLR.

```bash
cargo build --release        # produces target/release/rustnet
```

---

## run

Execute an assembly's entry point on RustCLR.

```bash
rustnet run <assembly> [--stats] [--trace] [--max-instructions N]
                       [--no-jit] [--jit-threshold N] [-- args...]
```

| | |
| --- | --- |
| `--stats` | Print execution, code-generation and heap counters when the program exits |
| `--trace` | Report what was loaded before running |
| `--max-instructions N` | Stop after N IL instructions — a runaway loop fails predictably |
| `--no-jit` | Interpret everything. Output must be identical; if it is not, that is a code-generation bug |
| `--jit-threshold N` | Calls before a method is compiled. Default 32; `1` compiles on first call |
| `-- args...` | Everything after `--` reaches the program as `Main`'s arguments |

```console
$ rustnet run Conformance.dll --stats --jit-threshold 1
checks=136 failures=0

─── execution ──────────────────────────────
  wall clock                  88.147 ms
  IL instructions              799,420
  throughput                 9,069,189 instr/s
  managed calls                 84,266
  native calls                  41,893
  compiled calls                    13
  methods compiled                   8
  code emitted               2,040 bytes
  peak frame depth                  16
─── heap ───────────────────────────────────
  collector                 mark-sweep
  allocations                   41,508
  bytes allocated            2,627,093
  collections                        0
  objects reclaimed                  0
  live objects                  41,508
```

The three code-generation rows appear only when the JIT is enabled.
`compiled calls` counts entries into machine code; `methods compiled` is how
many distinct methods the backend took.

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

## jit

Report what the native code generator can compile, and why it declines the rest.

```bash
rustnet jit <assembly>
```

```console
$ rustnet jit Benchmarks.dll
Code generation for Benchmarks
Backend: x86-64 baseline

  JIT  Benchmarks.Program::FibIterative(int)  (226 bytes)
  JIT  Benchmarks.Program::Collatz(int)  (277 bytes)
  JIT  Benchmarks.Program::Mix(long)  (195 bytes)
  JIT  Benchmarks.Program::Gcd(int,int)  (131 bytes)

  --   Benchmarks.Program::Fib(int): uses call
  --   Benchmarks.Program::Sieve(int): local 0 is not an integer
  --   Benchmarks.Program::Exceptions(int): has exception handling
  --   Node::.ctor(): is an instance method

4 compiled, 20 interpreted, 829 bytes emitted.
```

**A declined method is not a failure.** It is interpreted, exactly as it was
before the backend existed. The reasons are listed because they are actionable:
they are the backend's to-do list, ordered by what a real program actually hits.

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
