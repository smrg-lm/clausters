#!/usr/bin/env python3
"""The material in shared memory: a take is edited without a message.

Start a server with a segment first:

    cargo run --release -- --shm /dev/shm/clausters

then:

    python3 examples/shm_material.py

What it shows: a pool buffer's **samples are a file a peer maps**, not a
message it asks for. The buffer is allocated over the ring (allocation has
semantics beyond the samples, so it stays a command), and from there every
sample this script writes goes straight into the memory the engine reads —
`/buffer_setRange` is never sent, and neither is `/buffer_get`. The audible
part is the same buffer played twice: once as the tone written into the cells,
then again after a hand-written fade-out that no command carried.

The one line to hold on to: what a peer may write is **material**, samples it
already has. Every *operation* over samples — a gain, a fade, a reverse, a
render — is the server's verb and is asked for over the ring, however easy
mapped memory makes the other thing.
"""

import math
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))
import json_client as osc  # OSC encode/decode helpers (stdlib)
from clausters.ipc import ShmClient

SEGMENT = "/dev/shm/clausters"
BUFNUM = 0
SECONDS = 2.0
HZ = 440.0


def main():
    c = ShmClient(SEGMENT)
    rate = c.sample_rate
    frames = int(SECONDS * rate)
    print(f"attached to {SEGMENT}: {rate:.0f} Hz")

    # Allocation is a command: it is shape and lifetime, which is exactly what
    # does not travel through the mapping.
    addr, args = osc.decode(
        c.request(osc.message("/buffer_alloc", BUFNUM, frames, 1))
    )
    assert addr == "/done", f"{addr} {args}"

    # The directory says where the material is. It is not in the segment: a
    # ten-minute stereo take is 230 MB and the segment is sized once at boot,
    # so each buffer's samples are their own file beside it.
    info = c.buffer_info(BUFNUM)
    print(f"buffer {BUFNUM}: {info}")

    with c.map_buffer(BUFNUM) as take:
        # A tone, written one sample at a time into the server's own memory.
        # Nothing is sent, and there is nothing to wait for.
        step = 2.0 * math.pi * HZ / rate
        for i in range(take.frames):
            take.samples[i] = 0.3 * math.sin(step * i)
        print(f"wrote {take.frames} frames with no message sent")

        # Play it. This part is a command, because sounding a buffer is a node
        # in the tree and has nothing to do with where the samples live.
        d = osc.SynthDefBuilder("shmplay")
        d.add("Out", 0, d.add("PlayBuf", BUFNUM, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0))
        addr, _ = osc.decode(c.request(osc.message("/def_send", "synth", d.blob())))
        assert addr == "/done", addr
        c.send(osc.message("/synth_new", "shmplay", 4100, 1, 0))
        print(f"playing the take ({SECONDS:.0f}s) — a plain tone")
        time.sleep(SECONDS + 0.2)
        c.send(osc.message("/node_free", 4100))

        # Now edit it in place, and play the same buffer again. The samples the
        # second reader plays are the samples this loop just stored: same
        # buffer, no reallocation, no command, no reply.
        for i in range(take.frames):
            take.samples[i] *= 1.0 - i / take.frames
        print("faded the take out by writing its cells; playing the same buffer")
        c.send(osc.message("/synth_new", "shmplay", 4101, 1, 0))
        time.sleep(SECONDS + 0.2)
        c.send(osc.message("/node_free", 4101))

        # And say so, since the samples said nothing: every other client
        # holding a picture of this take learns the span changed.
        c.send(osc.message("/buffer_touch", BUFNUM, 0, 0, take.frames))

    # Freed with `request`, not `send`: it is asynchronous like every other
    # buffer command, and a script that exits without reading its `/done`
    # leaves that reply in the ring for whoever attaches next.
    addr, _ = osc.decode(c.request(osc.message("/buffer_free", BUFNUM)))
    assert addr == "/done", addr
    print("done — the server is still running (quit it with /server_quit or Ctrl-C).")
    c.close()


if __name__ == "__main__":
    try:
        main()
    except (FileNotFoundError, ValueError) as e:
        sys.exit(f"{e}\nstart the server first: cargo run --release -- --shm {SEGMENT}")
