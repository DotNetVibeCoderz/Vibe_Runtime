# Installation

Packages for Windows, Linux and macOS, built from source with one command.

*[Bahasa Indonesia →](id/instalasi.md)*

---

## What a package contains

| | |
| --- | --- |
| `bin/rustnet` | The toolchain — run, inspect, disassemble, verify |
| `codegen/` | The IDE, self-contained: no .NET runtime needed to launch it |
| `docs/`, `samples/` | Documentation, sample data, worked examples |
| `install.sh`, `install.ps1` | The installers |

**One thing is not in the box: the .NET SDK.** RustCLR executes IL; it does not
compile C#. Roslyn does that, and it ships with the SDK. The installers check
for it and say so rather than letting your first build fail with a confusing
error.

---

## Install

### Linux and macOS

```bash
tar xzf rustnet-0.1.0-linux-x64.tar.gz
cd rustnet-0.1.0-linux-x64
./install.sh
```

Installs into `~/.local` — no root needed, nothing written outside the prefix.
The script prints the exact line to add to your shell profile if `~/.local/bin`
is not already on `PATH`.

| | |
| --- | --- |
| `./install.sh` | Per-user, into `~/.local` (default) |
| `sudo ./install.sh --system` | For everyone, into `/usr/local` |
| `./install.sh --prefix /opt/rustnet` | Somewhere specific |
| `./install.sh --uninstall` | Remove it again |

On Linux a desktop entry is added so CodeGen appears in your application menu.

### Windows

```powershell
Expand-Archive rustnet-0.1.0-win-x64.zip
cd rustnet-0.1.0-win-x64
.\install.ps1
```

Installs into `%LOCALAPPDATA%\RustNetRuntime`, adds `bin` to your user `PATH`,
and creates a Start Menu shortcut for CodeGen. No elevation required.

| | |
| --- | --- |
| `.\install.ps1` | Per-user (default) |
| `.\install.ps1 -System` | All users, into `%ProgramFiles%` — needs an elevated shell |
| `.\install.ps1 -Prefix D:\Tools\RustNet` | Somewhere specific |
| `.\install.ps1 -Uninstall` | Remove it again |

Open a new terminal afterwards so the `PATH` change takes effect.

### Verify

```bash
rustnet capabilities
rustnet run <install-prefix>/samples/UserDirectory/bin/Release/net9.0/UserDirectory.dll
```

---

## Build a package

```bash
./packaging/build.sh                 # for this machine
./packaging/build.sh linux-arm64     # for a Raspberry Pi
./packaging/build.sh win-x64
```

The script builds the toolchain with `cargo build --release`, publishes CodeGen
self-contained for the runtime identifier, assembles the tree, and produces
`dist/rustnet-<version>-<rid>.tar.gz` — or a `.zip` for Windows targets.

### Supported runtime identifiers

| Runtime id | Rust target | Notes |
| --- | --- | --- |
| `win-x64` | `x86_64-pc-windows-msvc` | |
| `win-arm64` | `aarch64-pc-windows-msvc` | |
| `linux-x64` | `x86_64-unknown-linux-gnu` | |
| `linux-arm64` | `aarch64-unknown-linux-gnu` | Raspberry Pi 4/5, Apple silicon VMs |
| `linux-arm` | `armv7-unknown-linux-gnueabihf` | Raspberry Pi 2/3 |
| `linux-riscv64` | `riscv64gc-unknown-linux-gnu` | |
| `osx-x64` | `x86_64-apple-darwin` | |
| `osx-arm64` | `aarch64-apple-darwin` | Apple silicon |

### Cross-compiling

Add the Rust target first:

```bash
rustup target add aarch64-unknown-linux-gnu
./packaging/build.sh linux-arm64
```

If the target is not installed the script says so and builds for the host
instead of failing silently — read the output before shipping the result.

Cross-compiling to Linux from a non-Linux host also needs a linker for that
target (`gcc-aarch64-linux-gnu` and friends). The usual answer is to build each
platform's package on that platform, in CI.

When the .NET SDK cannot publish for a runtime identifier, the script warns and
ships a **toolchain-only** package: `rustnet` without CodeGen. That is the right
outcome for embedded and headless targets, where an Avalonia IDE has nowhere to
draw.

---

## Building from source instead

You do not need a package to use this:

```bash
git clone <repository>
cd RustNetRuntime
cargo build --release            # target/release/rustnet
dotnet run --project src/CodeGen # the IDE
```

`cargo test --workspace` should report 111 passing tests.

---

## Removing it

Both installers take `--uninstall` / `-Uninstall`. They remove the binaries, the
`PATH` entry and the menu shortcut.

They deliberately do **not** touch your projects, or the `CodeGen.dll.config`
holding your API keys — those are yours. If you want the keys gone too, delete
the install prefix by hand after uninstalling.
