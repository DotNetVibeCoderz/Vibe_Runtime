#!/usr/bin/env bash
# Benchmarks RustCLR against the reference .NET runtime.
#
# Each workload runs in its own process so wall clock includes startup — that is
# what a user of a CLI tool actually waits for. Every run's checksum is compared
# between the two runtimes; a mismatch means the comparison is meaningless and
# the harness says so instead of printing a number.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
PROJECT="$HERE/Benchmarks"
ASSEMBLY="$PROJECT/bin/Release/net9.0/Benchmarks.dll"

RUNS="${RUNS:-3}"
WORKLOADS="${WORKLOADS:-noop fib sieve strings matrix sort alloc virtual exceptions fields}"

# Prefer a release build of the toolchain; fall back to debug with a warning.
RUSTNET="$ROOT/target/release/rustnet"
[[ -x "$RUSTNET.exe" ]] && RUSTNET="$RUSTNET.exe"
if [[ ! -x "$RUSTNET" ]]; then
  RUSTNET="$ROOT/target/debug/rustnet"
  [[ -x "$RUSTNET.exe" ]] && RUSTNET="$RUSTNET.exe"
  if [[ -x "$RUSTNET" ]]; then
    echo "warning: using a debug build of rustnet — figures will be far slower than release" >&2
    echo "         build it with: cargo build --release" >&2
    echo >&2
  else
    echo "error: rustnet not found. Run: cargo build --release" >&2
    exit 1
  fi
fi

if [[ ! -f "$ASSEMBLY" ]]; then
  echo "Building the benchmark assembly…"
  dotnet build "$PROJECT" -c Release --nologo -v q || exit 1
fi

# Milliseconds of wall clock for one command, taking the best of $RUNS.
best_ms() {
  local best=""
  for _ in $(seq "$RUNS"); do
    local start end elapsed
    start=$(date +%s%N)
    "$@" > /dev/null 2>&1
    end=$(date +%s%N)
    elapsed=$(( (end - start) / 1000000 ))
    if [[ -z "$best" || $elapsed -lt $best ]]; then best=$elapsed; fi
  done
  echo "$best"
}

printf '%s\n' "RustNetRuntime benchmarks"
printf '%s\n' "  best of $RUNS runs, wall clock including process start"
printf '%s\n' "  rustnet: $RUSTNET"
printf '\n'
printf '| %-12s | %10s | %10s | %9s | %s\n' "Workload" ".NET (ms)" "RustCLR" "Ratio" "Checksum"
printf '| %-12s | %10s | %10s | %9s | %s\n' "------------" "---------" "---------" "--------" "--------"

for workload in $WORKLOADS; do
  expected=$(dotnet "$ASSEMBLY" "$workload" 2>/dev/null)
  actual=$("$RUSTNET" run "$ASSEMBLY" -- "$workload" 2>/dev/null | tail -1)

  if [[ "$expected" != "$actual" ]]; then
    printf '| %-12s | %10s | %10s | %9s | MISMATCH (.NET %s vs RustCLR %s)\n' \
      "$workload" "-" "-" "-" "${expected:-none}" "${actual:-none}"
    continue
  fi

  dotnet_ms=$(best_ms dotnet "$ASSEMBLY" "$workload")
  rustclr_ms=$(best_ms "$RUSTNET" run "$ASSEMBLY" -- "$workload")

  if [[ "$dotnet_ms" -gt 0 ]]; then
    ratio=$(awk "BEGIN { printf \"%.1fx\", $rustclr_ms / $dotnet_ms }")
  else
    ratio="-"
  fi

  printf '| %-12s | %10s | %10s | %9s | %s\n' \
    "$workload" "$dotnet_ms" "$rustclr_ms" "$ratio" "${expected#* }"
done

printf '\n'
printf '%s\n' "Ratio is RustCLR / .NET — lower is better. RustCLR interprets IL;"
printf '%s\n' ".NET compiles it to machine code, so a large ratio is expected."
