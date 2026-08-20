# Packaging

Scripts that turn the repository into something installable.

| File | |
| --- | --- |
| `build.sh` | Builds a package for a runtime identifier |
| `install.sh` | Installs a package on Linux or macOS |
| `install.ps1` | Installs a package on Windows |

User-facing documentation: [docs/installation.md](../docs/installation.md) ·
[Bahasa Indonesia](../docs/id/instalasi.md).

---

## Build

```bash
./packaging/build.sh              # detects this machine
./packaging/build.sh linux-arm64
./packaging/build.sh win-x64 x86_64-pc-windows-msvc
```

The second argument overrides the Rust target, which is normally derived from
the runtime identifier.

Output lands in `dist/`:

```
dist/rustnet-0.1.0-win-x64/        the staged tree
dist/rustnet-0.1.0-win-x64.zip     the archive (.tar.gz elsewhere)
```

### What it does

1. `cargo build --release -p rustnet-cli` for the target, falling back to the
   host with a warning if the target is not installed.
2. `dotnet publish --self-contained` for CodeGen, so the IDE runs without a .NET
   runtime present.
3. Copies the docs, samples, licence and installers.
4. Writes a `VERSION` file recording the runtime id, Rust target and build time.
5. Archives it — `Compress-Archive` for Windows targets, `tar` otherwise.

### Degrading honestly

Two things can go missing, and both are reported rather than hidden:

- **No `dotnet`, or no support for the runtime identifier.** The script warns
  and ships a **toolchain-only** package. That is the right outcome for embedded
  and headless targets, where an Avalonia IDE has nowhere to draw.
- **The Rust target is not installed.** The script says so, prints the
  `rustup target add` line, and builds for the host — so you notice before
  shipping a binary for the wrong architecture.

---

## Install

Both installers default to a per-user prefix that needs no privileges, and both
support `--uninstall`.

```bash
./install.sh                        # ~/.local
sudo ./install.sh --system          # /usr/local
./install.sh --prefix /opt/rustnet
./install.sh --uninstall
```

```powershell
.\install.ps1                            # %LOCALAPPDATA%\RustNetRuntime
.\install.ps1 -System                    # %ProgramFiles%, needs elevation
.\install.ps1 -Prefix D:\Tools\RustNet
.\install.ps1 -Uninstall
```

### Why a launcher rather than a symlink

CodeGen resolves `app.config` next to its own assembly. A symlink on `PATH`
would leave it looking in the wrong directory and it would start with default
settings every time, silently. So `bin/codegen` is a two-line shim that runs the
real executable from its install directory.

### What uninstall does not touch

Your projects, and the `CodeGen.dll.config` holding your API keys. Removing a
tool should not delete the work done with it. The docs say so, and say how to
remove the keys deliberately if that is what you want.

---

## Testing a package

Install into a throwaway prefix, exercise it, then uninstall:

```bash
./install.sh --prefix /tmp/rustnet-test
/tmp/rustnet-test/bin/rustnet capabilities
/tmp/rustnet-test/bin/rustnet run \
  /tmp/rustnet-test/samples/UserDirectory/bin/Release/net9.0/UserDirectory.dll
./install.sh --prefix /tmp/rustnet-test --uninstall
```

The Windows equivalent uses `-Prefix`. Both leave `PATH` and the menu clean
afterwards — worth confirming, since a half-removing uninstaller is worse than
none.
