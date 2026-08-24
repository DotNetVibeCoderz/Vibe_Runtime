#!/usr/bin/env bash
# Re-render every screenshot the README and docs use.
#
# Two kinds, both regenerated from the real thing rather than photographed:
#
#   * the desktop tools, drawn headlessly by their own UI toolkits;
#   * the device programs, whose *canvas* is written out as a PNG by
#     `rustnet run --capture-display`. That is the framebuffer the board would
#     show, so a screenshot cannot drift from what the program draws.
#
# A device program loops forever, which is why each one is given an instruction
# budget: the run stops mid-frame and the canvas is captured as it stood. The
# budgets below are chosen to land in a particular scene and are the only
# arbitrary numbers here — if a program's timings change, a picture shows a
# different moment rather than becoming wrong.
set -u

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
images="docs/images"
mkdir -p "$images"

rustnet="target/release/rustnet"
[ -x "$rustnet" ] || rustnet="target/debug/rustnet"
[ -x "$rustnet" ] || rustnet="target/debug/rustnet.exe"
[ -x "$rustnet" ] || { echo "build rustnet first: cargo build"; exit 1; }

failures=0

# capture <name> <assembly> <instruction-budget>
capture() {
  local name="$1" assembly="$2" budget="$3"
  if [ ! -f "$assembly" ]; then
    echo "SKIP  $name — $assembly is not built"
    return
  fi
  if "$rustnet" run --board host --max-instructions "$budget" \
       --capture-display "$images/$name.png" "$assembly" >/dev/null 2>&1 \
     || [ -f "$images/$name.png" ]; then
    # The run itself is expected to end on the budget, so its exit status says
    # nothing; whether the file appeared is the actual result.
    echo "ok    $images/$name.png"
  else
    echo "FAIL  $name"
    failures=$((failures + 1))
  fi
}

echo "-- device programs ------------------------------------------------"
show=samples/Showcase/bin/Release/net10.0/Showcase.dll
capture sample-showcase-starfield "$show"  5000000
capture sample-showcase-cube      "$show"  8000000
capture sample-showcase-fire      "$show" 40000000
capture sample-kiosk              samples/Kiosk/bin/Release/net10.0/Kiosk.dll 12000000

echo
echo "-- desktop tools --------------------------------------------------"
dotnet run --project src/CodeGen -c Release -- --screenshot "$images" 2>&1 | sed 's/^/      /'
dotnet run --project tools/RustNet.Workbench -c Release -- --screenshot "$images" 2>&1 | sed 's/^/      /'

echo
if [ "$failures" -eq 0 ]; then
  echo "All screenshots rendered into $images."
else
  echo "$failures screenshot(s) failed."
  exit 1
fi
