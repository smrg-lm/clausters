#!/usr/bin/env bash
# Render every subject on the UGen bench to its own WAV, one file per row.
#
# `examples/audition.py U6 --wav x.wav` writes one file with the whole
# milestone in it, back to back. This writes `whitenoise.wav`, `pinknoise.wav`
# and so on instead — the form you want when comparing two rows, scrubbing one,
# or dropping a handful into an editor. The file name is the subject's handle,
# so `saw.wav` is what `audition.py saw` plays.
#
# Everything renders **offline**, so it needs no audio hardware and no running
# server. It is a bench, not a test: nothing here asserts, and CI never runs it.
#
# Usage:
#   scripts/render-all.sh                      # -> ./render/*.wav, 2 s each
#   scripts/render-all.sh /tmp/ugens           # somewhere else
#   scripts/render-all.sh /tmp/ugens U2        # one milestone only
#   scripts/render-all.sh /tmp/ugens all 4.0   # four seconds each
#   scripts/render-all.sh /tmp/ugens U6 2 --amp 0.05
#
# Trailing arguments go to `audition.py` untouched, so `--amp` and `--set`
# work here too. A few rows are not audio-ranged (`Phasor` counts to its end,
# `PulseCount` counts triggers, `Integrator` accumulates) and the per-row report
# flags them as clipping — lower `--amp` for those rather than reading it as a
# failure.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"

# Same interpreter rule as refresh-bin.sh: the workspace-root .venv first, then
# one beside the package, then plain python3. The example puts clients/python on
# sys.path itself, so any of the three works.
python=""
for venv in "$root/.venv" "$root/clients/python/.venv"; do
    [ -x "$venv/bin/python" ] || continue
    python="$venv/bin/python"
    break
done
[ -n "$python" ] || python="$(command -v python3)"

out="${1:-$root/render}"
target="${2:-all}"
secs="${3:-2.0}"
[ $# -gt 3 ] && shift 3 || shift $#

subjects="$root/tests/common/subjects.json"
[ -f "$subjects" ] || { echo "no subject table at $subjects" >&2; exit 1; }

# The handles, read from the same table the asserts and the ear both use. `all`
# takes every milestone; anything else is treated as one milestone key (U1..U8)
# and an unknown one leaves the list empty, which is reported below.
names="$($python - "$subjects" "$target" <<'PY'
import json, sys
table = json.load(open(sys.argv[1], encoding="utf-8"))
want = sys.argv[2]
rows = [(k, v) for k, v in sorted(table.items()) if not k.startswith("_")]
if want != "all":
    rows = [(k, v) for k, v in rows if k == want]
print("\n".join(s["name"] for _, v in rows for s in v))
PY
)"

if [ -z "$names" ]; then
    echo "nothing to render for '$target' (expected a milestone U1..U8, or all)" >&2
    exit 1
fi

mkdir -p "$out"
count=0
first=""
for name in $names; do
    "$python" "$root/examples/audition.py" "$name" \
        --secs "$secs" --wav "$out/$name.wav" "$@" >/dev/null
    printf '  %-14s -> %s\n' "$name" "$out/$name.wav"
    [ -n "$first" ] || first="$name"
    count=$((count + 1))
done

echo
echo "$count files in $out (${secs}s each)"
echo "listen: pw-play $out/$first.wav"
echo "all of them: for f in $out/*.wav; do echo \"\$f\"; pw-play \"\$f\"; done"
