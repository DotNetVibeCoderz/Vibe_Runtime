# CodeGen

*[Bahasa Indonesia →](id/codegen.md)*

The IDE for RustNetRuntime, and its assistant — Jack, The Code Bender.

---

## The workspace

![The three-pane workspace](images/codegen-main.png)

**Explorer** (left) shows the open project. `bin`, `obj`, `.git`, `target` and
dotfolders are hidden — they are never what you want to edit. Double-click a
file to open it.

**Editor** (centre) is AvaloniaEdit with TextMate highlighting, line numbers you
can toggle (**View → Line Numbers**), and one tab per open file. A dot after the
filename means unsaved changes.

**Chat** (right) is Jack. Resize it by dragging the divider, hide it with
`Ctrl+J`.

**Output** (bottom) carries build and run output as it arrives, not after the
command finishes.

**Status bar** shows state, caret position, and the runtime readout:

```
IL 4,812   HEAP 3.1 KB   GC 0
```

Those are real counters, parsed from the last `rustnet run --stats`. An IDE
whose subject is a runtime should show the runtime's own numbers.

---

## Working with Jack

![Jack editing code, with the tools he used listed under the reply](images/codegen-chat.png)

Jack has tools. He is not answering from memory and asking you to type — he
reads and writes the files in front of you.

### What he can do

**In the project**

| Tool | |
| --- | --- |
| `list_files` | See what exists |
| `read_file` | Read a file, with line numbers |
| `write_file` | Create a file, or replace one entirely |
| `edit_file` | Replace one exact block — preferred for small changes |
| `delete_file` | Remove a file |
| `search_project` | Find which files contain a string |
| `create_project` | Scaffold from a template |
| `list_templates` | See the template catalogue |

**The toolchain**

| Tool | |
| --- | --- |
| `build` | Compile with the .NET SDK |
| `run` | Build and run, on .NET or RustCLR |
| `verify_on_rustclr` | Report what RustCLR cannot resolve |
| `deploy` | Publish self-contained for a runtime identifier |
| `disassemble` | Show a method's IL |
| `run_command` | Anything else (can be switched off in Settings) |

**Outside**

| Tool | |
| --- | --- |
| `search_internet` | Web search via Tavily |
| `scrape_web_page` | Fetch a page as readable text |
| `math_calculation` | Arithmetic, evaluated rather than guessed |
| `current_date_time` | The clock |
| `date_difference` | Time between two dates |

Every path Jack touches is resolved inside the open project and refused if it
escapes. He decides *what* to change; he does not decide *where*.

### Asking well

Jack works best with a goal and a constraint:

> Add a rolling median to the gateway. Keep it inside the IL subset RustCLR
> runs — no async — then run it on RustCLR and show me the output.

He will read the file, edit it, build, run, and tell you which files he touched.
The line under his reply lists the tools he actually called, so you can see what
happened rather than trusting a summary.

`Ctrl+Enter` sends. **CLEAR** starts a fresh thread — useful when switching
tasks, since the whole conversation is context.

### Attachments

**📎 ATTACH** adds image paths to the next message. Jack receives them as paths,
not inlined bitmaps: his tools read from disk, so a path is more useful and
costs far fewer tokens.

---

## Providers

![Settings, with every value stored in app.config](images/codegen-settings.png)

Four providers, chosen at the top of the chat panel:

| | |
| --- | --- |
| **Claude** | Through the official Anthropic SDK, wrapped as a Semantic Kernel service |
| **OpenAI** | Semantic Kernel's OpenAI connector |
| **Gemini** | The same connector, pointed at Google's OpenAI-compatible endpoint |
| **Ollama** | The same connector again, pointed at your local server. No key needed |

Three of the four speak the OpenAI protocol, so they share one code path.
Anthropic speaks its own, so `AnthropicChatCompletionService` bridges it —
converting Semantic Kernel's chat history and kernel functions into Anthropic
messages and tools, and running the tool loop. Above that class all four look
identical, which is why the tools work the same whichever you pick.

**One difference worth knowing:** the *temperature* setting does not apply to
Claude. The current Anthropic API rejects sampling parameters on recent models,
so CodeGen does not send it. The Settings dialog says so next to the field
rather than letting you set a value that is silently dropped.

---

## Configuration

Everything lives in `app.config`, which the build copies to
`CodeGen.dll.config` next to the executable. That copy is what the app reads and
writes. The Settings dialog shows its full path at the top.

There is no second store — no registry keys, no hidden JSON in AppData. Edit the
file or edit the dialog; they are the same thing.

What is in it: active provider, temperature, max tokens, system prompt; per
provider model, key and endpoint; the Tavily key; whether shell commands are
allowed and the maximum file size Jack may read; toolchain paths; editor font,
size, tab width, line numbers and wrapping; panel widths and visibility; and the
last open project, so the workspace comes back as you left it.

---

## New projects

![The New Project dialog](images/codegen-new-project.png)

**Blank** gives a console project and an entry point. **From Template** gives
one of fourteen, spanning console, web, desktop, mobile, IoT and library work
across business, science, education and games.

The panel on the right previews exactly which files will be written and how to
run the result. Templates marked *runs on RustCLR* are written to stay inside
the IL subset the runtime executes today — explicit loops instead of LINQ,
arrays instead of generic collections.

Full catalogue: [templates.md](templates.md).

---

## Build, run, verify, deploy

| | |
| --- | --- |
| **Build** (`Ctrl+B`) | `dotnet build -c Release` |
| **Run on .NET** (`F5`) | Build, then run on the reference runtime |
| **Run on RustCLR** (`Ctrl+F5`) | Build, then run on RustCLR with `--stats` |
| **Verify on RustCLR** | Report members RustCLR cannot resolve |
| **Deploy** | Publish self-contained for this machine's runtime identifier |

Having both run buttons side by side is the point: the same assembly, two
runtimes, one keystroke apart. When they disagree, you have found a runtime bug
— and `verify` usually tells you why before you run.

Compiling always goes through the .NET SDK. RustCLR consumes IL; it does not
compile C#.

---

## Devices

**Devices** (`Ctrl+D`) is where boards are found, identified and flashed.

![Devices](images/codegen-devices.png)

### The gauge is the point

A board's identity here is its memory budget. Loading the runtime costs
**192,045 bytes** with console, strings and maths, or **260,702** with every
RustBCL binding — both measured with a counting allocator, not estimated. Every
board is drawn against those two marks on one scale, so two rows can be compared
by eye and a tier reads as arithmetic.

| Colour | Meaning |
| --- | --- |
| Patina | clears both marks — every binding fits |
| Amber | clears the first — console, strings and maths fit |
| Ember | clears neither — no program runs on this board |

The Nucleo-F401RE is the honest end of that range: 65,536 bytes of heap against
a 192,045-byte floor, short by 126,509. It still reads assemblies and collects
garbage, and it says so rather than failing inside an allocator.

### Scanning does not touch the board

**Scan** lists serial ports and debug probes and nothing else. It runs
automatically when the panel opens, because a passive list is safe to take
without asking.

**Identify** is separate for a reason: it talks to the chip, and `espflash
board-info` resets the part into its bootloader. That would be a surprising side
effect of opening a window.

Detection is honest about what it cannot know. A CH340 bridge looks the same
whether an ESP32 or an unrelated device is behind it, and a debug probe says
nothing about what is on the other end of SWD, so those report **possibly
connected** rather than a confident guess. The next step writes to flash; a
wrong board identification there is expensive.

### Deploy puts your program inside the firmware

These boards have no filesystem, so an application is not copied onto one — it
is compiled into the image the firmware runs. Each firmware's `build.rs` reads
`RUSTCLR_APP`:

```bash
RUSTCLR_APP=/path/to/MyApp.dll cargo build --release
```

Deploy sets that variable, builds the firmware and flashes it. Nothing in the
firmware crate is edited, so the same tree serves every deploy.

For a board on the reduced binding set, Deploy first runs the program on the
desktop with `rustnet run --bcl minimal` — the same 314 bindings the board
carries. If it fails there it would fail on the board, and nothing is flashed.

### What each board needs installed

| Transport | Tool | Boards |
| --- | --- | --- |
| USB-serial | `espflash` | ESP32, ESP32-C3 |
| SWD | `probe-rs` | Netduino 3 WiFi, Nucleo-F401RE, Maix Go |
| USB DFU | `dfu-util` | Meadow F7 |
| UF2 volume | none | Raspberry Pi Pico |

A missing tool shows as **tool missing** with the name to install, rather than
as a board that is not there.

The Meadow F7 is deliberately not flashed from here. DFU into internal flash
replaces its contents and is not reversible without a backup, so it is left to
the documented steps in `embedded/meadow-f7/README.md`.

---

## Verifying the templates

Templates carry a `RunsOnRustClr` flag and a minimum board tier, and both are
claims. `--verify-templates` turns them into a check:

```bash
dotnet bin/Release/net10.0/CodeGen.dll --verify-templates [id] [--keep]
```

It scaffolds each template, builds it, runs it on .NET and on RustCLR, and
compares the output byte for byte. Templates written for a board run against
`--bcl minimal`, because passing with all 836 bindings says nothing about
whether a 192 KB board could run them.

Web, desktop and mobile templates are built but not run — they need a host — and
a library has no entry point. Those are reported as skipped rather than counted
as passes.

It earns its keep. On its first run it found three IoT templates using
`string + char`, which .NET 10 lowers to a span-based concat that RustCLR does
not implement; a `Console.ReadLine()` in two templates that hung the runner
until stdin was closed; and a double-formatting difference where .NET switches
to scientific notation and Rust never does.

---

## Format code

**Format Code** (`Ctrl+K`) is a brace-depth reindenter, not a C# parser. It
fixes indentation and trailing whitespace and leaves everything else alone.
Anything more would need Roslyn, and silently rewriting code is worse than not
formatting it.

---

## Regenerating the screenshots

The images in this documentation are rendered from the real windows, headlessly:

```bash
dotnet run --project src/CodeGen -c Release -- --screenshot docs/images
```

A screenshot nobody can regenerate goes stale the first time the layout changes.
This one cannot.
