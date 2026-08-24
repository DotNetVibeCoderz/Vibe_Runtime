#!/usr/bin/env bash
# Every template must instantiate and build.
#
# A template that does not compile is not a template, and the way that gets
# noticed otherwise is somebody starting a project with one. Each is copied to
# a scratch directory with `__NAME__` replaced, exactly as `rustnet new` would,
# and built against the device libraries in this repo.
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
WORK="${TMPDIR:-/tmp}/rustclr-templates"
rm -rf "$WORK"; mkdir -p "$WORK"

printf '%-24s %s\n' "TEMPLATE" "RESULT"
printf '%-24s %s\n' "------------------------" "------"
failures=0
for dir in templates/*/; do
  name="$(basename "$dir")"
  [[ -f "$dir/Program.cs" ]] || continue
  # `graphics-primitives` -> `GraphicsPrimitives`, the identifier a project needs.
  safe="$(echo "$name" | awk -F- '{for(i=1;i<=NF;i++) printf toupper(substr($i,1,1)) substr($i,2)}')"
  out="$WORK/$name"; mkdir -p "$out"
  sed "s/__NAME__/$safe/g" "$dir/Program.cs" > "$out/Program.cs"
  sed "s/__NAME__/$safe/g" "$dir/__NAME__.csproj" > "$out/$safe.csproj"
  # `RustNetSdk` is the property the template declares for exactly this.
  # Setting it beats rewriting the file: the generated project stays
  # byte-identical to what `rustnet new` would produce.
  if (cd "$out" && dotnet build -c Release --nologo -v q -p:RustNetSdk="$ROOT" > build.log 2>&1); then
    printf '%-24s %s\n' "$name" "ok"
  else
    printf '%-24s %s\n' "$name" "FAILED"
    grep -m2 " error " "$out/build.log" | sed 's/^/    /'
    failures=$((failures + 1))
  fi
done

echo
if [[ $failures -eq 0 ]]; then
  echo "All templates build."
else
  echo "$failures template(s) failed."
  exit 1
fi
