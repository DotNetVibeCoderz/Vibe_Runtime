# RustNetRuntime

A .NET runtime written in Rust. **RustCLR** loads ordinary .NET assemblies (IL compiled by
Roslyn) and runs them with its own interpreter, JIT, garbage collector and base class
library, on desktops and on microcontrollers.

```bash
dotnet tool install -g RustNetRuntime
rustnet --version
```

```bash
dotnet new console -n Hello && cd Hello
dotnet build -c Release
rustnet run bin/Release/net10.0/Hello.dll
```

| Command | What it does |
| --- | --- |
| `rustnet run <app.dll>` | Runs an assembly on RustCLR (`--board host` simulates a board) |
| `rustnet build [project]` | `dotnet build`, then `--run` to run the result |
| `rustnet info`, `disasm`, `verify` | Inspect metadata, disassemble IL, check an assembly can run |
| `rustnet jit`, `profile` | What the JIT compiles; a sampling profile with the hot methods |
| `rustnet debug` | Breakpoints and stepping, or `--dap` for an editor |
| `rustnet capabilities` | What this build of the runtime supports |

The package carries the native toolchain for Windows, Linux and macOS on x64 and arm64.
You still need the .NET SDK to compile C#: RustCLR runs IL, it does not replace Roslyn.

Installers with the CodeGen IDE, and archives for every platform:
[releases](https://github.com/DotNetVibeCoderz/Vibe_Runtime/releases) ·
[documentation](https://github.com/DotNetVibeCoderz/Vibe_Runtime/tree/main/RustNetRuntime).

Built by Gravicode Studios, led by Kang Fadhil. · Dibuat oleh Gravicode Studios dipimpin oleh Kang Fadhil.
