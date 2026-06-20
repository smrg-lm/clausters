#!/usr/bin/env bash
# MIDI-standalone demo (M19): set up a SynthDef + a GraphDef + a MIDI binding
# ONCE over OSC, then every later boot comes up already bound and playable from
# a controller with no OSC programming at all. The binding (and an optional
# boot preset) live in the data directory next to the defs.
#
# Two server sessions run in sequence sharing one --data-dir across a real
# restart. Needs `oscsend` (liblo-tools). No `faust` feature required.
#
#   cargo build --release
#   examples/midi_standalone.sh [play-seconds]   # session-2 hold, default 0.5
set -euo pipefail

PORT=57110
DATA_DIR="${CLAUSTERS_DATA_DIR:-/tmp/clausters-midi-standalone}"
SERVER=./target/release/clausters

if [ ! -x "$SERVER" ]; then
    echo "build first: cargo build --release" >&2
    exit 1
fi
command -v oscsend >/dev/null || { echo "need oscsend (liblo-tools)" >&2; exit 1; }

rm -rf "$DATA_DIR"
echo "data directory: $DATA_DIR"

# A per-voice oscillator and a shared mixer, then a polyphonic GraphDef wiring
# them (the per-voice osc writes a private bus; the shared mixer reads it).
VTONE='{"name":"vtone","controls":[{"name":"out","default":0.0},{"name":"freq","default":440.0},{"name":"level","default":0.2}],"ugens":[{"kind":"SinOsc","inputs":[{"control":1}]},{"kind":"Mul","inputs":[{"ugen":0},{"control":2}]},{"kind":"Out","inputs":[{"control":0},{"ugen":1}]}]}'
VGAIN='{"name":"vgain","controls":[{"name":"in","default":0.0},{"name":"gain","default":0.3}],"ugens":[{"kind":"In","inputs":[{"control":0}]},{"kind":"Mul","inputs":[{"ugen":0},{"control":1}]},{"kind":"Out","inputs":[{"const":0.0},{"ugen":0}]}]}'
POLY='{"name":"poly","buses":[{"name":"mix","rate":"audio"}],"members":[{"def":"vgain","controls":{"in":"mix"}},{"def":"vtone","controls":{"out":"mix"},"voice":true}],"surface":{"gain":[{"member":0,"control":"gain"}],"freq":[{"member":1,"control":"freq"}],"amp":[{"member":1,"control":"level"}]},"defaults":{"gain":0.3,"amp":0.2}}'

echo
echo "=== session 1: define the instrument + bind MIDI channel 0, then quit ==="
"$SERVER" --data-dir "$DATA_DIR" &
PID=$!
sleep 1.0
oscsend localhost $PORT /d_recv s "$VTONE"
oscsend localhost $PORT /d_recv s "$VGAIN"
oscsend localhost $PORT /d_graph s "$POLY"
oscsend localhost $PORT /midi_bind is 0 poly     # channel 0 -> the GraphDef
sleep 0.3
oscsend localhost $PORT /quit
wait $PID

echo
echo "persisted files (the binding survives in midi.json):"
ls -1 "$DATA_DIR"
cat "$DATA_DIR/midi.json"

echo
echo "=== session 2: restart with --midi; the binding is back with no OSC ==="
"$SERVER" --midi clausters --data-dir "$DATA_DIR" &
PID=$!
sleep 1.0
# The restored binding already spawned its shared instance: one group at root.
oscsend localhost $PORT /g_queryTree i 0
echo "the channel-0 GraphDef binding is live again (no /d_* or /midi_bind sent)."
echo
echo "to actually PLAY it, route a controller into the server's MIDI input."
echo "the PipeWire-native path uses the JACK MIDI backend: build with"
echo "  --features midi-jack  and run the server under  pw-jack , then:"
echo "    pw-link -i                        # find the 'clausters' input port"
echo "    pw-link <your-controller> clausters:input_0"
echo "  (or wire it visually in qpwgraph)."
echo "on a plain-ALSA build instead, route the native ALSA-seq port with"
echo "aconnect (routing that ALSA port through PipeWire is what midi-jack fixes)."
echo "...then each note spawns a voice. CC/pitch-bend map via /midi_map."
sleep "${1:-0.5}"
oscsend localhost $PORT /quit
wait $PID

echo
echo "done. Tip: drop a boot.json in $DATA_DIR (e.g."
echo '  [{"graph":"poly","ports":{"gain":0.5}}] ) to also bring up standalone graphs.'
