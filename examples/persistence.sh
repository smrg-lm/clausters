#!/usr/bin/env bash
# Def persistence demo: defs loaded over OSC are saved to a data directory and
# reloaded automatically when the server restarts, so a client need not
# re-send its instrument library every session.
#
# Persistence is on by default; this script points it at a throwaway directory
# with --data-dir. The two server sessions run in sequence (the first exits on
# /quit before the second starts) so they share that directory across a real
# restart. Needs the `faust` feature and `oscsend` (liblo-tools).
#
#   cargo build --release --features faust
#   examples/persistence.sh
set -euo pipefail

PORT=57110
DATA_DIR="${CLAUSTERS_DATA_DIR:-/tmp/clausters-defs-demo}"
SERVER=./target/release/clausters

if [ ! -x "$SERVER" ]; then
    echo "build first: cargo build --release --features faust" >&2
    exit 1
fi

rm -rf "$DATA_DIR"
echo "data directory: $DATA_DIR"

# A fixed-frequency sine in raw Faust (0 inputs, 1 output). The recursive
# `+ : (_ <: _ - floor)` builds a 0..1 phasor: `(_ <: _ - floor)` splits its one
# input so the fractional part stays a single-input block (`_ - floor(_)` would
# be two inputs and fail to compose). 44100 matches the server sample rate.
SINE='process = sin(6.283185307179586 * ((+(440.0/44100.0) : (_ <: _ - floor)) ~ _)) * 0.2;'

echo
echo "=== session 1: define 'psine' and quit (it gets persisted) ==="
"$SERVER" --data-dir "$DATA_DIR" &
PID=$!
sleep 1.0
oscsend localhost $PORT /d_faust ss psine "$SINE"
sleep 0.5                       # let the async compile + persist finish
oscsend localhost $PORT /quit
wait $PID

echo
echo "persisted files:"
ls -1 "$DATA_DIR/faustdefs"      # psine.json (source of truth) + psine.<sha>.bc (cache)

echo
echo "=== session 2: restart WITHOUT re-sending the def — it reloads itself ==="
"$SERVER" --data-dir "$DATA_DIR" &
PID=$!
sleep 1.0                        # the def reloads in the background; give it a moment
oscsend localhost $PORT /s_new siii psine 3001 1 0   # instantiate the reloaded def
echo "you should hear 440 Hz — the def survived the restart"
sleep 1.5
oscsend localhost $PORT /n_free i 3001
oscsend localhost $PORT /quit
wait $PID

echo
echo "done. (delete a .bc or bump libfaust and it recompiles from the .json;"
echo " '/d_free psine' would remove both files.)"
