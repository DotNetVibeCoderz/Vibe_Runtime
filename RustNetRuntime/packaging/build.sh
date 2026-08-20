#!/usr/bin/env bash
# Builds a distributable RustNetRuntime package.
#
#   ./packaging/build.sh [rid] [rust-target]
#
# The package contains everything needed to run C# on RustCLR: the `rustnet`
# toolchain, a self-contained CodeGen (no .NET install required to launch the
# IDE), the documentation and the samples.
#
# A .NET SDK is still needed to *compile* C# — RustCLR consumes IL, it does not
# replace Roslyn. The installer says so rather than letting the first build fail
# with a confusing error.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed 's/.*"\(.*\)".*/\1/')"

# ── Target selection ─────────────────────────────────────────────────────────
detect_rid() {
  case "$(uname -s)" in
    Linux)  case "$(uname -m)" in
              x86_64) echo "linux-x64" ;;
              aarch64|arm64) echo "linux-arm64" ;;
              riscv64) echo "linux-riscv64" ;;
              *) echo "linux-x64" ;;
            esac ;;
    Darwin) case "$(uname -m)" in
              arm64) echo "osx-arm64" ;;
              *) echo "osx-x64" ;;
            esac ;;
    MINGW*|MSYS*|CYGWIN*) echo "win-x64" ;;
    *) echo "linux-x64" ;;
  esac
}

RID="${1:-$(detect_rid)}"

rust_target_for() {
  case "$1" in
    win-x64)       echo "x86_64-pc-windows-msvc" ;;
    win-arm64)     echo "aarch64-pc-windows-msvc" ;;
    linux-x64)     echo "x86_64-unknown-linux-gnu" ;;
    linux-arm64)   echo "aarch64-unknown-linux-gnu" ;;
    linux-arm)     echo "armv7-unknown-linux-gnueabihf" ;;
    linux-riscv64) echo "riscv64gc-unknown-linux-gnu" ;;
    osx-x64)       echo "x86_64-apple-darwin" ;;
    osx-arm64)     echo "aarch64-apple-darwin" ;;
    *)             echo "" ;;
  esac
}

RUST_TARGET="${2:-$(rust_target_for "$RID")}"
STAGE="$ROOT/dist/rustnet-$VERSION-$RID"
EXE=""
[[ "$RID" == win-* ]] && EXE=".exe"

echo "RustNetRuntime $VERSION"
echo "  runtime id   $RID"
echo "  rust target  ${RUST_TARGET:-host}"
echo

# ── Toolchain ────────────────────────────────────────────────────────────────
echo "Building the runtime and toolchain…"
if [[ -n "$RUST_TARGET" ]] && rustup target list --installed 2>/dev/null | grep -qx "$RUST_TARGET"; then
  cargo build --release --target "$RUST_TARGET" -p rustnet-cli
  RUSTNET="$ROOT/target/$RUST_TARGET/release/rustnet$EXE"
else
  if [[ -n "$RUST_TARGET" ]]; then
    echo "  note: target $RUST_TARGET is not installed; building for the host instead."
    echo "        add it with: rustup target add $RUST_TARGET"
  fi
  cargo build --release -p rustnet-cli
  RUSTNET="$ROOT/target/release/rustnet$EXE"
fi

[[ -f "$RUSTNET" ]] || { echo "error: $RUSTNET was not produced" >&2; exit 1; }

# ── Stage ────────────────────────────────────────────────────────────────────
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/docs" "$STAGE/samples"
cp "$RUSTNET" "$STAGE/bin/"

# ── CodeGen ──────────────────────────────────────────────────────────────────
# Self-contained so the IDE launches without a .NET runtime present. Skipped
# when the SDK cannot target this RID, which keeps a toolchain-only package
# possible on exotic targets.
if command -v dotnet >/dev/null 2>&1; then
  echo "Publishing CodeGen for $RID…"
  if dotnet publish "$ROOT/src/CodeGen" \
      -c Release -r "$RID" --self-contained true \
      -p:PublishSingleFile=false \
      -o "$STAGE/codegen" --nologo -v q; then
    echo "  CodeGen published."
  else
    echo "  warning: CodeGen could not be published for $RID; shipping the toolchain only." >&2
    rm -rf "$STAGE/codegen"
  fi
else
  echo "  note: no dotnet on PATH; shipping the toolchain only."
fi

# ── Content ──────────────────────────────────────────────────────────────────
cp "$ROOT/README.md" "$ROOT/README.id.md" "$ROOT/LICENSE" "$STAGE/"
cp -r "$ROOT/docs/." "$STAGE/docs/"
cp -r "$ROOT/samples/." "$STAGE/samples/"
cp "$HERE/install.sh" "$STAGE/" 2>/dev/null || true
cp "$HERE/install.ps1" "$STAGE/" 2>/dev/null || true

cat > "$STAGE/VERSION" <<META
RustNetRuntime $VERSION
runtime id: $RID
rust target: ${RUST_TARGET:-host}
built: $(date -u '+%Y-%m-%dT%H:%M:%SZ')
built by Gravicode Studios, led by Kang Fadhil
META

# ── Archive ──────────────────────────────────────────────────────────────────
cd "$ROOT/dist"
ARCHIVE="rustnet-$VERSION-$RID"
PACKAGE=""

# Windows users expect a .zip. `zip` on PATH is not always Info-ZIP, so try
# PowerShell first and verify the result rather than trusting the exit code.
if [[ "$RID" == win-* ]]; then
  rm -f "$ARCHIVE.zip"
  if command -v powershell.exe >/dev/null 2>&1; then
    powershell.exe -NoProfile -NonInteractive -Command       "Compress-Archive -Path '$ARCHIVE' -DestinationPath '$ARCHIVE.zip' -Force" >/dev/null 2>&1 || true
  fi
  if [[ ! -f "$ARCHIVE.zip" ]] && command -v zip >/dev/null 2>&1; then
    zip -qr "$ARCHIVE.zip" "$ARCHIVE" >/dev/null 2>&1 || true
  fi
  [[ -f "$ARCHIVE.zip" ]] && PACKAGE="$ARCHIVE.zip"
fi

if [[ -z "$PACKAGE" ]]; then
  rm -f "$ARCHIVE.tar.gz"
  tar czf "$ARCHIVE.tar.gz" "$ARCHIVE"
  PACKAGE="$ARCHIVE.tar.gz"
fi

[[ -f "$PACKAGE" ]] || { echo "error: no archive was produced" >&2; exit 1; }
SIZE=$(du -h "$PACKAGE" | cut -f1)
echo
echo "Package: dist/$PACKAGE ($SIZE)"
echo "Install: cd dist/$ARCHIVE && ./install.sh"
