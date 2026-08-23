# Benchmarks

RustCLR against the reference .NET runtime, on the same assembly.

```bash
cd benchmarks
bash run.sh
```

---

## Results

Windows 11, x64. `rustnet` built with `cargo build --release`; the benchmark
assembly built `-c Release` with tiered compilation **off**, so .NET runs at
full JIT speed rather than warming up. Best of three runs, wall clock including
process start.

| Workload | .NET (ms) | RustCLR (ms) | Ratio | What it stresses |
| --- | ---: | ---: | ---: | --- |
| `fib` | 111 | **77** | **0.7×** | Recursive call overhead, fib(27) — **compiled** |
| `fields` | 124 | **95** | **0.8×** | 3M field reads and writes — **compiled, scalarised** |
| `noop` | 84 | **73** | **0.9×** | Process start — nothing else |
| `sieve` | 93 | **85** | **0.9×** | 1M-element sieve over `bool[]` — **compiled** |
| `matrix` | 105 | **92** | **0.9×** | 120×120 multiply over `double[]` — **compiled** |
| `sort` | 123 | **109** | **0.9×** | Quicksort over 200k ints — **compiled, recursive** |
| `inlined` | 111 | **97** | **0.9×** | Arithmetic factored into helpers — **compiled, inlined** |
| `virtual` | 118 | 129 | 1.1× | 2M virtual calls — **compiled, devirtualised** |
| `exceptions` | 144 | 159 | 1.1× | 50k throws through `try`/`catch`/`finally` |
| `kernels` | 96 | 142 | 1.5× | Integer arithmetic in helpers that loop — **compiled, called** |
| `alloc` | 117 | 199 | 1.7× | 300k allocations with collection pressure — **compiled** |
| `arrays` | 135 | 244 | 1.8× | An `int[]` passed in and walked — **compiled** |
| `strings` | 102 | 203 | 2.0× | 20k concatenations, `Length`, `IndexOf` |

Every row's checksum is compared between the two runtimes before it is timed;
a mismatch prints `MISMATCH` instead of a number, because a faster wrong answer
is not a result.

The top of that table needs reading carefully rather than triumphantly. `noop`
is 73 ms, and every row down to `sort` lands within about 35 ms of that floor
— so
much of what they measure is **starting the process**, not computing.

Their own history is the honest comparison, and it is where the work shows:

| Row | was | now | what changed |
| --- | ---: | ---: | --- |
| `sieve`, `matrix` | ~1,500 ms | 85–92 ms | tiering compiles a method whose body loops |
| `fields` | 3,173 ms | 95 ms | scalar replacement retires the object |
| `virtual` | 1,923 ms | 129 ms | guarded devirtualisation |
| `alloc` | 1,093 ms | 199 ms | safe points, then compiled field access — the loop stopped being interpreted |
| `kernels` | 350 ms | 142 ms | the transition stopped doing needless work, then compiled methods could call each other |
| `inlined` | 477 ms | 97 ms | the same, then the inliner learned to flatten more than one level |

### What `kernels` and `inlined` actually measured

Both are compiled and both were slow, which looked like a code-quality problem
and was not — three times over, and none of the three was about the emitted
instructions.

The first was the **transition**. `kernels` made 240,009 managed calls of which
239,969 were calls *into* compiled code, and interpreted only 2.2M instructions.
Each transition built an allocation context — three types resolved by name,
three array constructions searched for — for methods that never allocate.

The second was **inlining depth**. `inlined`'s driving loop calls `Step`, which
is built from three smaller helpers. A one-level inliner splices a leaf into its
caller; `Step` is not a leaf, so the loop could not compile at all and paid
400,031 transitions. Flattening a callee's own calls before splicing it took
that to one.

The third was that **a compiled method could not call another one**. `kernels`
factors its work into helpers that *loop*, so they cannot be spliced at any
depth — the only way to compile the loop around them was to give compiled code
a calling convention. That took the row from 266 ms to 142 ms, and `sort` with
it, since `Sort` calls `QuickSort`.

### Where `alloc` used to spend its time

`alloc` was the slowest row in the suite for a long time, and the obvious
explanations were both wrong. Deleting each candidate is what settled it:

| Experiment | Result |
| --- | ---: |
| Baseline (interpreted) | 589 ms |
| Every `new Node()` hands back the *same* object | 548 ms |
| Field reads and writes become no-ops | 630 ms |
| The whole of `newobj` short-circuited | 506 ms |

Removing allocation entirely saved nothing. Removing field access saved
nothing. Only the whole `newobj` path moved it, by about 14% — and the other
86% was simply the interpreter executing 9.6M IL instructions.

So the answer was never a faster allocator; it was to stop interpreting the
loop. That needed two things the backend refused at the time: allocation inside
a loop, which needed safe points, and field access on a heap object, which
needed a way to reach a field without knowing the heap's layout. With both
built, the row stopped being interpreted: **199 ms**, 1.7× .NET.

### A worked example of measuring a change

Pinning a method's two busiest locals and arguments in callee-saved registers
looked like it did nothing. A single run of each build put `kernels` at 363 ms
against 350 ms and `inlined` at 483 ms against 477 ms — no better, possibly
worse, and the emitted code grew by about 10%.

That reading was wrong twice over. The first measurement was taken while the
transition cost above still dominated, so there was nothing for better code to
improve. And one run per build cannot resolve a 5% difference on a machine that
drifts by ten.

Measured properly — both binaries in one session, alternating, best of eleven:

| Row | pinned | unpinned | |
| --- | ---: | ---: | ---: |
| `fields` | 69 ms | 89 ms | **−22%** |
| `sieve` | 94 ms | 103 ms | **−8%** |
| `virtual` | 102 ms | 112 ms | **−8%** |
| `fib` | 79 ms | 82 ms | −3% |
| `matrix` | 89 ms | 89 ms | 0% |
| `kernels` | 252 ms | 252 ms | 0% |
| `inlined` | 297 ms | 298 ms | 0% |

The pattern is consistent once seen: pinning pays where the *whole loop* is
compiled and the register is reused across iterations, and costs nothing where
short compiled leaves are called from an interpreted loop. Intermediate sample
sizes gave `kernels` at +2% and `inlined` at +9% — both of which vanished at
eleven samples, which is what a difference that is really zero looks like on
the way there.

### How much of a difference is real

**About ten percent of run-to-run drift, on this machine, within one session.**
That is measured, not assumed: back when `fields` was interpreted it gave
2,856 ms, then 3,097 ms, then 3,200 ms across one afternoon on the same binary,
drifting slower as the machine warmed. Two full suite runs an hour apart put every row 5–10% above the table
above.

So a 5% difference between two runs of this suite means nothing. Before
attributing one to a change, measure both binaries **in the same session,
interleaved** — which is how the shared-heap cost in
[limitations.md](limitations.md) was measured, and why that comparison is
trustworthy where a comparison against this table would not be.

The check that catches a real regression is the reverse experiment: remove the
suspect code and measure again. A per-call lookup added for generic type
arguments looked like an 8% cost on `virtual` until deleting it measured
*slower* still — the drift was the whole signal.

`noop` is the row to distrust most: it is process start and nothing else, so it
measures the machine rather than the runtime. Registering the full BCL rather
than the console-only subset accounts for about 3 ms of it, measured with
`--bcl minimal`.

---

## Reading these numbers

**Subtract the `noop` row.** Both figures include process start, which is now
most of RustCLR's time on the faster half of the table. `fields` at 95 ms sits
22 ms above a 73 ms floor: the 3M field accesses it performs account for that
22 ms, not for 95. Reading any sub-1× ratio as "faster at arithmetic" would be
wrong — it mostly says RustCLR starts quickly, having no JIT to warm up.

**Every row is compiled or native now.** That was not true for most of this
file's history, and the sections above record how each row stopped being
interpreted. What is left is worth stating plainly:

* `strings` and `exceptions` were never interpreted in the interesting sense —
  that work happens in native Rust inside RustBCL.
* `arrays` and `alloc` are compiled but still the slowest ratios, and both are
  dominated by something other than the emitted code: array element access
  costs a bounds check per element, and `alloc` spends its time in the
  allocator and the collector rather than in the loop around them.
* `kernels` is compiled *and* calls compiled helpers. It is the row that
  required the calling convention, because its helpers loop and so can never be
  spliced by the inliner at any depth.

**What compilation is worth, where it can be isolated.** These are measured by
running the same workload with `--no-jit`, so the comparison is the same code
either way:

| Workload | interpreted | compiled | speedup |
| --- | ---: | ---: | ---: |
| `arrays` — an `int[]` passed in and walked | 18,401 ms | 205 ms | **89.8×** |
| `fields` — 3M reads and writes of an object's fields | 3,123 ms | 120 ms | **26.0×** |
| `virtual` — 2M calls through a base reference | 1,923 ms | 96 ms | **20.0×** |
| `kernels` — arithmetic written out longhand | 2,971 ms | 269 ms | **11.0×** |

`arrays` is the widest because element access is exactly where the interpreter
pays most: a handle resolution and a `Value` for every element, against a bounds
check and a scaled-index load.

**Inlining and the calling convention do different jobs, and the contrast is
the point.** `inlined` factors the same arithmetic as `kernels` into small
static helpers, which is what code in the wild looks like:

| | interpreted | compiled, `--no-inline` | compiled |
| --- | ---: | ---: | ---: |
| `inlined` | 1,629 ms | 1,148 ms | **400 ms** |

Inlining is worth nothing at all on `kernels` — its helpers contain loops, so
none are eligible at any depth — and that is precisely why the calling
convention had to exist. One optimisation compiles code that was *factored*;
the other compiles code that was factored into things too big to splice.

```bash
rustnet jit benchmarks/Benchmarks/bin/Release/net10.0/Benchmarks.dll

# The two figures above, isolated:
rustnet run --no-jit    …/Benchmarks.dll -- inlined   # interpreted
rustnet run --no-inline …/Benchmarks.dll -- inlined   # compiled, nothing spliced
rustnet run            …/Benchmarks.dll -- inlined    # compiled and inlined
```

**RustCLR starts in half the time.** Its tiering compiles nothing until a method
has been called 32 times, and emits a few hundred bytes when it does, so there
is no warm-up to pay for: 65 ms against 126 ms. For a CLI tool that runs briefly and exits, or a
microcontroller that has no room for a code cache, that matters more than steady
-state throughput.

**Exceptions are a wash, and strings are close.** Throwing is cheap here because
RustCLR does not build a full managed stack trace object on every throw, and
string work is close because `Concat`, `Length` and `IndexOf` are native Rust —
the interpreter dispatches once and the work happens at native speed. The same
is true of anything that spends its time inside RustBCL rather than in IL.

**Field access is the worst case, and that is the right shape.** `fields` does
almost nothing per instruction, so the interpreter's per-instruction overhead is
essentially the whole measurement. It is the honest upper bound on what a JIT
would recover.

---

## What would change these

[Milestone 4](../Plan.md) is a native code generator. The analysis it needs
already exists — `rustclr-jit` computes basic blocks, per-instruction stack
depth and which methods a simple backend could take. Every workload above
except `noop` and `exceptions` is dominated by interpreter dispatch, which is
exactly what compilation removes.

The tiering design means a partial backend helps immediately:
`Compiler::can_compile` decides per method, so an emitter that only handles leaf
integer methods would already take `fib`, `sieve` and `fields`.

---

## Running them yourself

```bash
cargo build --release              # rustnet must be a release build
cd benchmarks
bash run.sh                        # all workloads, best of 3

RUNS=10 bash run.sh                # more runs, less noise
WORKLOADS="sieve sort" bash run.sh # just these two
```

The harness refuses to report a figure when the two runtimes disagree on the
result, and warns loudly if it finds only a debug build of `rustnet` — a debug
interpreter is several times slower again and would make the comparison
meaningless.

The workloads live in `benchmarks/Benchmarks/Program.cs`. They use arrays and
explicit loops — no LINQ, no generic collections — because a benchmark that only
runs on one of the two runtimes measures nothing useful. That was a hard
constraint when they were written; since Milestone 2 both would run, but keeping
the workloads allocation-light is what makes them measure the interpreter rather
than the collection implementations.
