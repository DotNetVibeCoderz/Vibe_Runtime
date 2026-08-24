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
                       [--no-jit] [--jit-threshold N] [--board NAME]
                       [--bcl full|minimal] [-- args...]
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
checks=361 failures=0

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

### --board

```bash
rustnet run --board host app.dll     # the simulator
rustnet run --board none app.dll     # explicit: no hardware (the default)
```

Which board `RustNet.Hal` talks to. Without it there is no board, and a program
that reaches for a pin is told so rather than ignored — only `Board.Name` and
`Board.IsPresent` answer either way.

An unknown name lists what the build has instead of falling back to the
simulator. That is deliberate: a program that quietly ran against a simulation
when it was meant to drive a relay has done something worse than fail. A real
chip installs its own board from its firmware, so this flag is for the desktop.

```
$ rustnet run --board bogus app.dll
rustnet: unknown board `bogus`. This build has: host (the simulator), none.
A real chip installs its own board from its firmware, not from this flag.
```

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

## profile

Runs a program under the sampling profiler and reports where the time went and
what was allocated.

```bash
rustnet profile app.dll                       # the default: 1 ms sampling, top 15
rustnet profile app.dll --top 30              # more rows
rustnet profile app.dll --period-us 200       # sample five times as often
rustnet profile app.dll --json                # for a script or an IDE
rustnet profile app.dll --jit -- workload     # with the code generator on
```

### What it prints

Four sections. **Where the time went** ranks methods by *self* time — time with
that method on top of the stack — and then again by *inclusive* time, which is
the method plus everything it called. The two orders rarely agree, and the
difference is usually the answer:

```
  method                                            self    share          calls
  Benchmarks.Program::Allocate(int)               5.85 s   100.0%              1

  by inclusive time (the method and everything it called)
  Benchmarks.Program::Allocate(int)               5.85 s   100.0%  ############
  Benchmarks.Program::Main(string[])              5.85 s   100.0%  ############
```

**What was allocated** ranks types by bytes. Arrays are charged to their
element type — `System.Int32[]`, not `System.Array`, because "45% arrays" is
not something anybody can act on. **Memory** and **run** carry the heap totals,
collection pauses, and how many samples the numbers rest on.

### How it samples

The interpreter samples **itself**. Every 512 instructions it reads the clock,
and if a period has passed it walks its own frame stack and charges the
*measured* elapsed time to what it finds. Three things follow:

- **No sampling thread.** Reading another thread's frame stack means stopping
  it first, and the only machinery for that is the collector's. Sampling from
  inside also means the stack is never caught mid-update.
- **A late sample does not shorten the run.** `Thread::sleep(1ms)` on Windows
  is anywhere from 1 to 15 ms; a profiler that assumed its period was honoured
  would report a program as fifteen times faster than it was. Charging what
  the clock actually says removes the question.
- **It works on a microcontroller.** The clock is `Host::monotonic_nanos`,
  which on a board is a hardware timer, and there is no second thread to spare.

Calls are **counted**, not sampled — `fib(20)` reports exactly 21,891 — because
a call is a rare enough event that an increment on it distorts nothing.

### What it cannot see

- **Wall clock, not CPU time.** Time blocked in a `Task.Delay` or a lock is
  charged to the method that waited. That is the right answer for "why did this
  take so long" and the wrong one for "what is burning the processor".
- **One thread.** A thread the program starts has its own interpreter and no
  profiler, so its time and its allocations are not attributed. The heap totals
  in the memory section still include them, which is why the two can differ on
  a threaded program.
- **Compiled code runs no IL.** `rustnet profile` therefore interprets
  everything unless `--jit` is given. With `--jit`, a compiled method's time is
  charged to the interpreted frame that called it and its allocations to
  nothing — a plausible-looking wrong answer, which is why it is not the
  default and why the command says so on stderr when you ask for it.

A profile is a **shape**, not a benchmark: use `rustnet run --stats` for how
fast something is, and this for where the time is going.

### From inside the program

The counters the profiler reports are the same ones a program reads about
itself, so the two cannot drift apart:

```csharp
long before = GC.GetTotalAllocatedBytes(false);
int collections = GC.CollectionCount(0);
// ... work ...
Console.WriteLine($"{GC.GetTotalAllocatedBytes(false) - before} bytes");
Console.WriteLine($"{GC.CollectionCount(0) - collections} collections");
```

`GC.GetAllocatedBytesForCurrentThread()` answers the *process* total: there is
one heap and no per-thread allocation context. `GC.CollectionCount` ignores its
generation argument, because the default collector is not generational and
`GC.MaxGeneration` says so.

---

## debug

Runs a program under the debugger hooks. Two front ends over the same hooks:
by default it prints every stop, and with `--dap` it speaks the Debug Adapter
Protocol so an editor drives it.

```bash
rustnet debug app.dll --break Program.cs:42   # stop at a source line
rustnet debug app.dll --break Triple:2        # stop before IL_0002 of Triple
rustnet debug app.dll --step                  # step into, instruction by instruction
rustnet debug app.dll --dap                   # speak DAP over stdin/stdout
```

### Breakpoints

A breakpoint is either `File.cs:line` or `Method:offset`; which one it is falls
out of whether the name ends in `.cs`. Both split on the **last** colon, so a
full Windows path works — `C:\src\Program.cs:42` splits before `42`, not after
`C`.

`File.cs:line` needs a **portable PDB** beside the assembly, which the .NET SDK
writes by default. Without one the command says so and only `Method:offset`
works. A line is resolved to *every* place it has code, because a lambda and
its enclosing method share lines and arming only one would silently miss the
other. A line with **no** code is refused rather than moved to a nearby line
that has some:

```
$ rustnet debug app.dll --break Program.cs:1
no code on Program.cs:1
```

`Method:offset` matches a name **exactly**; an ambiguous one lists the
candidates and stops rather than picking among them — asking for `Triple` and
silently getting `TripledAsync` is the kind of plausible wrong answer this
runtime refuses everywhere else. `rustnet disasm <assembly> Type.Method` prints
the offsets.

### What a stop shows

Every frame with its source position where the PDB has one, and locals by name
where the PDB names them:

```
─── Breakpoint · Conformance.Program::Triple(int) · Program.cs:2189 · 3 frame(s)
  args    I32(9)
     at Conformance.Program::TwoLevels(int) · Program.cs:2194
     at Conformance.Program::Main() · Program.cs:2110
```

A compiler-generated slot the PDB does not name is printed by index (`[3]=…`)
rather than given an invented name — the DAP adapter calls the same slot
`local3`, because a variables pane wants a name.

`--step` steps into with a budget, then detaches and runs to completion, so a
real program terminates rather than printing forever.

### --dap

`rustnet debug app.dll --dap` turns the process into a **debug adapter**:
DAP requests and responses as JSON over stdin and stdout, framed with
`Content-Length:` headers exactly as LSP is. VSCode and Rider can attach.
Implemented: `initialize`, `setBreakpoints` (by source line), `configurationDone`,
`launch`/`attach`, `threads`, `stackTrace`, `scopes`, `variables`, `continue`,
`next`, `stepIn`, `stepOut`, `disconnect`/`terminate`.

**There is no editor extension yet.** The adapter is a stdio DAP server and
nothing more; an editor reaches it either through a small extension that
contributes the debug type and points at the executable, or through any client
that can launch an arbitrary adapter:

```js
// The whole of what an extension has to contribute.
vscode.debug.registerDebugAdapterDescriptorFactory("rustnet", {
  createDebugAdapterDescriptor: (session) =>
    new vscode.DebugAdapterExecutable("rustnet", [
      "debug", "--dap", session.configuration.program,
    ]),
});
```

Three things are worth stating rather than discovering:

- **stdout belongs to the protocol.** The debugged program's console arrives as
  `output` events, which is where an editor shows it anyway. `Console.ReadLine`
  returns `null`: stdin carries the editor's requests, and handing the program
  a line of protocol would be worse than telling it there is no console.
- **While the program runs, no requests are serviced.** The runtime's hook is
  synchronous — it is called with the program stopped and returns the command
  to resume with — so an editor's pause button does nothing until the next
  breakpoint. Servicing it would mean reading the stream from another thread
  and interrupting the interpreter, which is a different design.
- **One thread is debugged**: the one that runs `Main`. A thread the program
  starts gets its own interpreter and no debugger, so a breakpoint inside it
  never fires. Its console *is* redirected, so what it prints still reaches the
  editor.

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
