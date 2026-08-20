#!/usr/bin/env bash
# Runs every advanced-feature probe on both runtimes and prints a support matrix.
#
# A feature counts as supported only when RustCLR produces the *same output* as
# .NET. Running without crashing is not enough — a wrong answer is worse than a
# clear failure.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
ASSEMBLY="$HERE/bin/Release/net10.0/AdvancedFeatures.dll"

RUSTNET="$ROOT/target/release/rustnet.exe"
[[ -x "$RUSTNET" ]] || RUSTNET="$ROOT/target/release/rustnet"
[[ -x "$RUSTNET" ]] || { echo "error: build rustnet first (cargo build --release)" >&2; exit 1; }
[[ -f "$ASSEMBLY" ]] || { echo "error: build the fixture first (dotnet build -c Release)" >&2; exit 1; }

PROBES="${PROBES:-async-await tpl threading gc dispose dispose-async span primary-ctor collection-expr collection-expr-spread collection-expr-span extension-members pinvoke marshalling unsafe-fixed unsafe-stackalloc linq pattern-matching records generated interceptor}"

printf '%-20s %-26s %s\n' "FEATURE" "RESULT" "DETAIL"
printf '%-20s %-26s %s\n' "-------------------" "-------------------------" "------"

supported=0
total=0

for probe in $PROBES; do
  total=$((total + 1))

  expected="$(dotnet "$ASSEMBLY" "$probe" 2>&1 | tail -1)"
  actual="$("$RUSTNET" run "$ASSEMBLY" -- "$probe" 2>&1 | tail -1)"

  if [[ "$expected" == "$actual" ]]; then
    supported=$((supported + 1))
    printf '%-20s %-26s %s\n' "$probe" "supported" "${expected#PASS * }"
  elif [[ "$expected" != PASS* ]]; then
    printf '%-20s %-26s %s\n' "$probe" "n/a (fails on .NET too)" "$(echo "$expected" | cut -c1-60)"
  else
    printf '%-20s %-26s %s\n' "$probe" "NOT supported" "$(echo "$actual" | cut -c1-70)"
  fi
done

printf '\n%s of %s probes produce identical output on both runtimes.\n' "$supported" "$total"
