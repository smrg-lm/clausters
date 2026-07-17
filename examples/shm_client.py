#!/usr/bin/env python3
"""Shared-memory client demo (M14): no sockets anywhere.

Start the server with a segment first:

    cargo run --release -- --shm /dev/shm/clausters

then:

    python3 examples/shm_client.py

Everything below talks to the server through mapped memory only (pure
stdlib `mmap`): commands and replies travel a byte ring, and the **data
plane** — the sample clock and the 1024 control buses — is read and written
directly, no command and no round trip at all. The audible part: a synth
whose amplitude comes from control bus 7 via `InCtl`, faded by writing that
bus straight into shared memory while it plays.
"""

import json
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))
import json_client as osc  # OSC encode/decode helpers (stdlib)
from clausters import ShmClient

SEGMENT = "/dev/shm/clausters"


def main():
    c = ShmClient(SEGMENT)
    print(f"attached to {SEGMENT}: {c.sample_rate:.0f} Hz")

    # Data plane: the clock advances block by block with zero commands sent.
    t0 = c.clock
    time.sleep(0.25)
    print(f"sample clock: {t0} -> {c.clock} (+{c.clock - t0} in 0.25 s)")

    # Command plane: a /status round trip through the ring.
    addr, args = osc.decode(c.request(osc.message("/status")))
    print(f"  <- {addr} {args}")

    # A def whose amplitude is control bus 7 (InCtl): the fade below happens
    # entirely in the data plane.
    d = osc.SynthDefBuilder("shmsine")
    amp = d.add("InCtl", 7)
    sine = d.add("Mul", d.add("Mul", d.add("Sine", 330), 0.3), amp)
    d.add("Out", 0, sine)
    d.add("Out", 1, sine)
    addr, _ = osc.decode(c.request(osc.message("/d_recv", d.blob())))
    assert addr == "/done", addr

    c.ctl_set(7, 0.0)  # start silent
    c.send(osc.message("/s_new", "shmsine", 4000, 1, 0))
    print("fading in and out by writing control bus 7 in shared memory:")
    for v in [0.2, 0.5, 1.0, 0.5, 0.1, 0.0]:
        c.ctl_set(7, v)
        print(f"  bus 7 = {v}  (readback {c.ctl_get(7):.2f})")
        time.sleep(0.5)
    c.send(osc.message("/n_free", 4000))
    print("done — the server is still running (quit it with /quit or Ctrl-C).")
    c.close()


if __name__ == "__main__":
    try:
        main()
    except (FileNotFoundError, ValueError) as e:
        sys.exit(f"{e}\nstart the server first: cargo run --release -- --shm {SEGMENT}")
