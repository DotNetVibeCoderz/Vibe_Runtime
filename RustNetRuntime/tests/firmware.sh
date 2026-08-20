#!/usr/bin/env bash
# Builds every board firmware, on every architecture.
#
# `tests/embedded.sh` proves the crates compile without `std`. This proves the
# firmwares that *use* them still link — which is a different failure: a change
# to a shared type breaks a board long before it breaks the host build, and
# nobody notices until they reach for the hardware.
#
# It builds; it does not flash. The captured runs live in docs/logs/.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

printf '%-30s %-16s %-30s %s\n' "BOARD" "CORE" "TARGET" "RESULT"
printf '%-30s %-16s %-30s %s\n' "-----------------------------" \
  "---------------" "-----------------------------" "------"

failures=0
skipped=0

# board | core | dir | toolchain | extra cargo args
BOARDS=(
  "ESP32-WROOM-32|Xtensa LX6|esp32-demo|+esp|--no-default-features --features esp32 --target xtensa-esp32-none-elf -Z build-std=core,alloc"
  "ESP32-C3|RISC-V 32|esp32-demo||--no-default-features --features esp32c3 --target riscv32imc-unknown-none-elf"
  "Meadow F7 Micro|Arm Cortex-M7|meadow-f7||"
  "Nucleo-F401RE|Arm Cortex-M4F|stm32f4||--no-default-features --features nucleo-f401re"
  "Netduino 3 WiFi|Arm Cortex-M4F|stm32f4||--no-default-features --features netduino3-f427vi"
  "Raspberry Pi Pico|Arm Cortex-M0+|rp2040||"
  "Sipeed Maix Go|RISC-V 64|k210||"
)

for entry in "${BOARDS[@]}"; do
  IFS='|' read -r board core dir toolchain args <<< "$entry"

  # The target is either named in the args or comes from .cargo/config.toml.
  target=$(sed -n 's/.*--target \([^ ]*\).*/\1/p' <<< "$args")
  if [[ -z "$target" ]]; then
    target=$(sed -n 's/^target = "\(.*\)"/\1/p' "$ROOT/embedded/$dir/.cargo/config.toml" | head -1)
  fi

  # Xtensa needs the forked toolchain; skip cleanly rather than fail if espup
  # has not been run on this machine.
  if [[ -n "$toolchain" ]] && ! rustup toolchain list 2>/dev/null | grep -q "^${toolchain#+}"; then
    printf '%-30s %-16s %-30s %s\n' "$board" "$core" "$target" "skipped — no ${toolchain#+} toolchain"
    skipped=$((skipped + 1))
    continue
  fi

  if (cd "$ROOT/embedded/$dir" && cargo $toolchain build --release $args) \
      >/tmp/firmware.$$ 2>&1; then
    printf '%-30s %-16s %-30s %s\n' "$board" "$core" "$target" "ok"
  else
    printf '%-30s %-16s %-30s %s\n' "$board" "$core" "$target" "FAILED"
    grep -E '^error' /tmp/firmware.$$ | head -5 | sed 's/^/    /'
    failures=$((failures + 1))
  fi
done
rm -f /tmp/firmware.$$

echo
if [[ $failures -gt 0 ]]; then
  echo "$failures firmware build(s) failed."
  exit 1
fi
if [[ $skipped -gt 0 ]]; then
  echo "All available firmwares built. $skipped skipped."
  exit 0
fi
echo "All seven firmwares built."
