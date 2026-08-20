#!/usr/bin/env bash
# Builds the bare-metal crates for every embedded target, without `std`.
#
# This exists because a "no_std-friendly" claim rots silently: nothing on a
# host build notices when a `std::` path creeps into a crate that is supposed
# to compile for a microcontroller. Running this catches it the same day.
#
# It builds; it does not run. No hardware is involved, and the docs say so.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
cd "$ROOT" || exit 1

# The crates that are meant to work without an operating system. `rustclr-core`
# is deliberately absent: it needs `HashMap`, file IO and a clock, and listing
# it here would make this script assert something untrue.
CRATES="${CRATES:-rustclr-metadata rustclr-gc}"

TARGETS="${TARGETS:-thumbv7em-none-eabihf thumbv6m-none-eabi riscv32imc-unknown-none-elf riscv64gc-unknown-none-elf}"

printf '%-34s %-20s %s\n' "TARGET" "CRATE" "RESULT"
printf '%-34s %-20s %s\n' "---------------------------------" "-------------------" "------"

failures=0
missing=0

for target in $TARGETS; do
  if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
    printf '%-34s %-20s %s\n' "$target" "-" "not installed — rustup target add $target"
    missing=$((missing + 1))
    continue
  fi
  for crate in $CRATES; do
    if cargo build -p "$crate" --no-default-features --target "$target" >/tmp/embedded.$$ 2>&1; then
      printf '%-34s %-20s %s\n' "$target" "$crate" "ok"
    else
      printf '%-34s %-20s %s\n' "$target" "$crate" "FAILED"
      grep -E '^error' /tmp/embedded.$$ | head -5 | sed 's/^/    /'
      failures=$((failures + 1))
    fi
  done
done
rm -f /tmp/embedded.$$

echo
if [[ $failures -gt 0 ]]; then
  echo "$failures build(s) failed."
  exit 1
fi
if [[ $missing -gt 0 ]]; then
  echo "All installed targets built. $missing target(s) were skipped."
  exit 0
fi
echo "All targets built without std."
