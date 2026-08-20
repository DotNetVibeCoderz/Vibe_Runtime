#!/usr/bin/env bash
# Installs RustNetRuntime on Linux or macOS.
#
#   ./install.sh                 install for the current user
#   ./install.sh --system        install for everyone (needs root)
#   ./install.sh --prefix DIR    install somewhere specific
#   ./install.sh --uninstall     remove a previous installation
#
# A user install needs no privileges and is the default. Nothing is written
# outside the chosen prefix.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PREFIX="$HOME/.local"
MODE="user"
UNINSTALL=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --system) PREFIX="/usr/local"; MODE="system"; shift ;;
    --prefix) PREFIX="${2:?--prefix needs a directory}"; MODE="custom"; shift 2 ;;
    --uninstall) UNINSTALL=true; shift ;;
    -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

BIN="$PREFIX/bin"
LIB="$PREFIX/lib/rustnet"
SHARE="$PREFIX/share/rustnet"
DESKTOP="$PREFIX/share/applications"

# ── Uninstall ────────────────────────────────────────────────────────────────
if $UNINSTALL; then
  echo "Removing RustNetRuntime from $PREFIX…"
  rm -f "$BIN/rustnet" "$BIN/codegen"
  rm -rf "$LIB" "$SHARE"
  rm -f "$DESKTOP/rustnet-codegen.desktop"
  echo "Done. Your projects and settings were not touched."
  exit 0
fi

# ── Preconditions ────────────────────────────────────────────────────────────
if [[ "$MODE" == "system" && "$(id -u)" != "0" ]]; then
  echo "error: --system needs root. Re-run with sudo, or drop the flag for a user install." >&2
  exit 1
fi

[[ -f "$HERE/bin/rustnet" ]] || {
  echo "error: bin/rustnet is missing — run this from inside an unpacked package." >&2
  exit 1
}

echo "Installing RustNetRuntime into $PREFIX"
echo

# ── Files ────────────────────────────────────────────────────────────────────
mkdir -p "$BIN" "$LIB" "$SHARE"

install -m 0755 "$HERE/bin/rustnet" "$BIN/rustnet"
echo "  toolchain    $BIN/rustnet"

if [[ -d "$HERE/codegen" ]]; then
  rm -rf "$LIB/codegen"
  cp -r "$HERE/codegen" "$LIB/codegen"
  chmod +x "$LIB/codegen/CodeGen" 2>/dev/null || true

  # A launcher rather than a symlink: CodeGen resolves its own directory for
  # app.config, and a symlink would point it at the wrong place.
  cat > "$BIN/codegen" <<LAUNCHER
#!/usr/bin/env bash
exec "$LIB/codegen/CodeGen" "\$@"
LAUNCHER
  chmod +x "$BIN/codegen"
  echo "  IDE          $BIN/codegen"
fi

cp -r "$HERE/docs" "$SHARE/" 2>/dev/null || true
cp -r "$HERE/samples" "$SHARE/" 2>/dev/null || true
cp "$HERE/README.md" "$HERE/README.id.md" "$HERE/LICENSE" "$SHARE/" 2>/dev/null || true
echo "  docs         $SHARE/docs"
echo "  samples      $SHARE/samples"

# ── Desktop entry, on Linux with a graphical session ─────────────────────────
if [[ "$(uname -s)" == "Linux" && -d "$HERE/codegen" ]]; then
  mkdir -p "$DESKTOP"
  cat > "$DESKTOP/rustnet-codegen.desktop" <<ENTRY
[Desktop Entry]
Type=Application
Name=CodeGen
GenericName=IDE for RustNetRuntime
Comment=Write C# and run it on RustCLR
Exec=$BIN/codegen %f
Terminal=false
Categories=Development;IDE;
Keywords=csharp;dotnet;rust;runtime;
ENTRY
  echo "  menu entry   $DESKTOP/rustnet-codegen.desktop"
fi

# ── PATH ─────────────────────────────────────────────────────────────────────
echo
if [[ ":$PATH:" != *":$BIN:"* ]]; then
  echo "  $BIN is not on your PATH. Add it:"
  echo
  case "$(basename "${SHELL:-bash}")" in
    zsh)  echo "    echo 'export PATH=\"$BIN:\$PATH\"' >> ~/.zshrc && source ~/.zshrc" ;;
    fish) echo "    fish_add_path $BIN" ;;
    *)    echo "    echo 'export PATH=\"$BIN:\$PATH\"' >> ~/.bashrc && source ~/.bashrc" ;;
  esac
  echo
fi

# ── The one dependency we do not ship ────────────────────────────────────────
if ! command -v dotnet >/dev/null 2>&1; then
  echo "  Note: the .NET SDK was not found."
  echo "        RustCLR runs IL; it does not compile C#. Install the SDK from"
  echo "        https://dotnet.microsoft.com/download to build projects."
  echo
fi

echo "Installed. Try it:"
echo
echo "    rustnet capabilities"
echo "    rustnet run $SHARE/samples/UserDirectory/bin/Release/net9.0/UserDirectory.dll"
echo
echo "Built by Gravicode Studios, led by Kang Fadhil."
