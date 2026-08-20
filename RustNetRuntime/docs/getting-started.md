# Getting started

*[Bahasa Indonesia →](id/memulai.md)*

---

## What you need

| | |
| --- | --- |
| Rust | 1.85 or newer (`rustup` recommended) |
| .NET SDK | 10 — RustCLR consumes IL, so you still need Roslyn to compile C# |

Check both:

```bash
cargo --version
dotnet --version
```

---

## Build the runtime

```bash
cargo build --release
```

That produces `target/release/rustnet`, the toolchain binary. Put it on your
PATH or use the full path in the commands below.

Run the test suite to confirm the build is sound:

```bash
cargo test --workspace
```

You should see 116 tests pass.

---

## Run your first program on RustCLR

The repository ships a small fixture:

```bash
cd tests/fixtures/HelloWorld
dotnet build -c Release
rustnet run bin/Release/net10.0/HelloWorld.dll
```

```
Hello from RustCLR
42
120
```

That output came from a Rust runtime executing IL that Roslyn produced. Add
`--stats` to see what it cost:

```bash
rustnet run bin/Release/net10.0/HelloWorld.dll --stats
```

---

## Run the conformance suite

This is the interesting one — the same assembly on both runtimes:

```bash
cd tests/fixtures/Conformance
dotnet build -c Release

dotnet bin/Release/net10.0/Conformance.dll
rustnet run bin/Release/net10.0/Conformance.dll
```

Both print `checks=80 failures=0`. If they ever differ, that is a runtime bug
and the differing check names it. `tests/fixtures/ModernSyntax/` is the same
idea for modern C# features and prints `checks=35 failures=0`.

---

## Bring your own program

Any C# project that stays inside the supported subset will run. The fastest way
to find out is to ask:

```bash
cd path/to/your/project
dotnet build -c Release
rustnet verify bin/Release/net10.0/YourApp.dll
```

`verify` lists every member your program references that RustCLR cannot yet
supply, and every method whose IL fails verification. A clean report means it
should run:

```bash
rustnet run bin/Release/net10.0/YourApp.dll
```

If `verify` reports missing framework members, that is expected for anything
using `async`, `Span<T>` or a framework type RustBCL has not implemented yet —
see [limitations.md](limitations.md). Generic collections and LINQ do run.

---

## Launch CodeGen

```bash
dotnet run --project src/CodeGen
```

On first launch there is no project open and Jack is asleep — he needs a
provider.

**Wake Jack up.** Open **Edit → Settings → Providers** and fill in one:

| Provider | What to enter |
| --- | --- |
| Claude | API key from console.anthropic.com |
| OpenAI | API key from platform.openai.com |
| Gemini | API key from aistudio.google.com |
| Ollama | No key. Set the endpoint to your local server, e.g. `http://localhost:11434/v1`, and pick a model you have pulled |

Pick the active provider at the top of the chat panel; the model list updates to
match.

**Optional: web search.** Add a Tavily API key under **Settings → Tools** to
give Jack `search_internet` and `scrape_web_page`.

**Optional: point at the toolchain.** CodeGen looks for `rustnet` in
`target/release`, then `target/debug`, then PATH. If yours lives elsewhere, set
**Settings → Toolchain → rustnet path**.

Every one of these lands in `app.config` next to the executable. You can edit
that file directly if you prefer; the dialog and the file are the same store.

---

## Make something

**File → New Project** offers a blank console project or one of fourteen
templates. Pick *Sensor Gateway* under IoT — it is written to stay inside the IL
subset RustCLR runs.

Then, in the chat panel:

> Add a rolling median alongside the mean, and run it on RustCLR.

Jack will read `Gateway.cs`, edit it, build, and run. The tools he used are
listed under his reply, the build output appears in the log panel, and the
status bar picks up the runtime counters from the run.

---

## Keyboard

| | |
| --- | --- |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save all |
| `Ctrl+G` | Go to line |
| `Ctrl+K` | Format code |
| `Ctrl+B` | Build |
| `F5` / `Ctrl+F5` | Run on .NET / Run on RustCLR |
| `Ctrl+J` | Show or hide the chat panel |
| `Ctrl+Enter` | Send a message to Jack |

---

## Where to go next

- [CodeGen guide](codegen.md) — the IDE in detail
- [Toolchain reference](cli.md) — every `rustnet` command
- [Architecture](architecture.md) — how the runtime is put together
- [Advanced C# features](advanced-features.md) — the measured support matrix
- [Limitations](limitations.md) — what does not work yet, and why
