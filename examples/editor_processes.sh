#!/usr/bin/env bash
# The three processes an autonomous editor is made of, and what each one is for.
#
#     ./examples/editor_processes.sh
#
# What it shows: **the samples outlive the process that plays them.** One
# server owns the segment and the takes in it (standing in for the editor's
# on-demand session — same code, no audio device needed to own samples); a
# second one attaches to the same segment, holds the audio device, and plays
# what the first owns. Then the player is killed and another is started, and
# the take is still there and still plays: nothing about the samples moved,
# because it never belonged to the player.
#
# The rule underneath, in one line: the rings are SPSC, so the first server on
# a segment claims the command plane and owns the samples, and any later one
# attaches to the data plane and serves its own clients over its own sockets.
# Samples never travel; allocation and lifetime always do, which is what
# /buffer_attach is.
#
# The last section shows the other side of that property: samples outliving
# its process means a segment left by an owner that was *killed* looks exactly
# like one being kept, so the claim answers that too -- creating a segment
# sweeps the ones whose owner no longer exists.
#
# Needs: a built server (cargo build --release) and an audio device for the
# player. Everything is on 127.0.0.1 and cleans up after itself.
set -u

here="$(cd "$(dirname "$0")/.." && pwd)"
bin="$here/target/release/clausters"
seg="/dev/shm/clausters-editor-demo-$$"
seg2="/dev/shm/clausters-editor-demo-$$-next"
owner_port=57410
player_port=57411
py="${PYTHON:-python3}"

[ -x "$bin" ] || { echo "build it first: cargo build --release"; exit 1; }

cleanup() {
  kill "${owner_pid:-0}" "${player_pid:-0}" "${player2_pid:-0}" "${owner2_pid:-0}" 2>/dev/null
  sleep 0.3
  rm -f "$seg" "$seg".buf* "$seg2" "$seg2".buf* 2>/dev/null
}
trap cleanup EXIT

echo "== the owner: it creates the segment and owns every take in it"
"$bin" --shm "$seg" --port "$owner_port" -v >/tmp/clausters-owner.log 2>&1 &
owner_pid=$!
sleep 1.5
grep -o "this server owns it" /tmp/clausters-owner.log || echo "(see /tmp/clausters-owner.log)"

echo
echo "== a take, allocated on the owner and written by a peer with no message"
PYTHONPATH="$here/clients/python:$here/examples" "$py" - "$seg" <<'PY'
import math, sys
from clausters.ipc import ShmClient
import json_client as osc

c = ShmClient(sys.argv[1])
addr, args = osc.decode(c.request(osc.message("/buffer_alloc", 0, int(1.5 * c.sample_rate), 1)))
assert addr == "/done", (addr, args)
with c.map_buffer(0) as take:
    step = 2 * math.pi * 330.0 / take.sample_rate
    for i in range(take.frames):
        take.samples[i] = 0.3 * math.sin(step * i) * (1.0 - i / take.frames)
    print(f"  wrote {take.frames} frames into the region beside the segment")
# Nothing else is sent: the owner holds the samples, and what sounds it is a
# def on the *player*, which is a different server and gets its own below.
c.close()
PY

play_it() {  # $1 = port, $2 = node id
  PYTHONPATH="$here/clients/python:$here/examples" "$py" - "$1" "$2" <<'PY'
import socket, sys, time
import json_client as osc
port, node = int(sys.argv[1]), int(sys.argv[2])
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.settimeout(2.0)
addr = ("127.0.0.1", port)
# Point this server at the take the owner published: it mapped the directory
# when it started, and this take was allocated afterwards.
s.sendto(osc.message("/buffer_attach", 0), addr)
print("   ", osc.decode(s.recv(4096)))
d = osc.SynthDefBuilder("demoplay")
d.add("Out", 0, d.add("PlayBuf", 0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0))
s.sendto(osc.message("/def_send", "synth", d.blob()), addr)
s.recv(4096)
s.sendto(osc.message("/synth_new", "demoplay", node, 1, 0), addr)
time.sleep(1.7)
s.sendto(osc.message("/node_free", node), addr)
PY
}

echo
echo "== the player: another process, attached to the same segment"
"$bin" --shm "$seg" --port "$player_port" --client-name clausters-player -v \
  >/tmp/clausters-player.log 2>&1 &
player_pid=$!
sleep 1.5
grep -o "attached to the shared segment.*" /tmp/clausters-player.log | head -1
play_it "$player_port" 5000
echo "  (that was the owner's take, played by a process that owns none of it)"

echo
echo "== kill the player and start another against the same segment"
kill "$player_pid"; wait "$player_pid" 2>/dev/null
"$bin" --shm "$seg" --port "$player_port" --client-name clausters-player -v \
  >/tmp/clausters-player2.log 2>&1 &
player2_pid=$!
sleep 1.5
play_it "$player_port" 5001
echo "  (same take, same samples: killing the player took no samples with it)"

echo
echo "== what a restart does NOT bring back is the routing"
echo "   the ports and the connections a person made live with the process."
echo "   --client-name is what makes them come back under the same name, so a"
echo "   patchbay can reconnect them; the samples needed nothing."

echo
echo "== and what a *killed owner* leaves behind is collected by the next one"
# SIGKILL, so nothing runs on the way out: the segment and one file per take
# stay in /dev/shm exactly as a crashed editor leaves them.
kill "${player2_pid}" 2>/dev/null; wait "$player2_pid" 2>/dev/null
kill -9 "$owner_pid"; wait "$owner_pid" 2>/dev/null
echo "   left behind: $(ls -1 "$seg" "$seg".buf* 2>/dev/null | wc -l) file(s), $(du -ch "$seg" "$seg".buf* 2>/dev/null | tail -1 | cut -f1) in a memory filesystem"
# An editor names its segment for its pid, so its next run is a new path --
# and creating one sweeps the segments whose owner no longer exists.
"$bin" --shm "$seg2" --port "$owner_port" -v >/tmp/clausters-owner2.log 2>&1 &
owner2_pid=$!
sleep 1.5
grep -o "swept the shared segment.*" /tmp/clausters-owner2.log || echo "   (nothing swept -- see /tmp/clausters-owner2.log)"
echo "   left behind now: $(ls -1 "$seg" "$seg".buf* 2>/dev/null | wc -l) file(s)"
echo "   (a claim naming a pid nobody answers to is what says the segment is dead;"
echo "    a segment being served, and the path being opened, are never touched)"
