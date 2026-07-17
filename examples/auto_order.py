#!/usr/bin/env python3
"""Auto-sorted groups (M12): the server orders nodes by bus connections.

In scsynth, execution order is the client's burden: a node that reads an
audio bus must be placed *after* the nodes writing it, or you get silence
(buses clear every block). This demo builds a classic chain —

    source (sine -> bus 16)  ->  fx (halves bus 16 in place)  ->  master
                                                   (bus 16 -> hardware out)

— deliberately **backwards** (master first, source last) inside a normal
group: you hear nothing. Then a single `/g_sortMode 1` makes the group
auto-sorted: the server infers source -> fx -> master from the buses each
def reads/writes, reorders the nodes, and the chain becomes audible. The
inferred graph is printed before and after with `/g_dumpGraph`.

Run a server first (`cargo run --release`), then:

    python3 examples/auto_order.py

See `docs/auto-order.md` for the analysis rules (controls as bus indexes,
dynamic barriers, feedback cycles).
"""

import sys
import time

import json_client as osc  # the stdlib-only OSC helpers next to this file

GROUP = 100


def defs() -> dict[str, bytes]:
    """The three defs of the chain, as /d_recv JSON blobs."""
    src = osc.SynthDefBuilder("src")
    sine = src.add("Mul", src.add("Sine", src.control("freq", 330.0)), 0.2)
    src.add("Out", 16, sine)

    fx = osc.SynthDefBuilder("fx")
    halved = fx.add("Mul", fx.add("In", 16), 0.5)
    fx.add("ReplaceOut", 16, halved)  # insert fx: consumes and rewrites 16

    master = osc.SynthDefBuilder("master")
    bus = master.add("In", 16)
    master.add("Out", 0, bus)
    master.add("Out", 1, bus)

    return {b.name: b.blob() for b in (src, fx, master)}


def dump_graph(client: "osc.Client", label: str):
    client.send("/g_dumpGraph", GROUP)
    _, args = client.reply(quiet=True)
    print(f"--- inferred graph {label} ---")
    print(args[1].decode() if isinstance(args[1], bytes) else args[1])


def main():
    client = osc.Client()
    for name, blob in defs().items():
        client.send("/d_recv", blob)
        addr, _ = client.reply(quiet=True)
        assert addr == "/done", f"def {name} rejected"

    client.send("/g_new", GROUP, 0, 0)

    # Backwards on purpose: the reader first, the source last.
    print("adding master, fx, source — in the WRONG order, manual group")
    client.send("/s_new", "master", 1001, 1, GROUP)
    client.send("/s_new", "fx", 1002, 1, GROUP)
    client.send("/s_new", "src", 1003, 1, GROUP)
    time.sleep(0.3)
    dump_graph(client, "before sorting")
    print("listen: 2 seconds of... nothing (master reads an empty bus)")
    time.sleep(2.0)

    print("/g_sortMode 100 1  ->  the server reorders by bus dependencies")
    client.send("/g_sortMode", GROUP, 1)
    time.sleep(0.3)
    dump_graph(client, "after sorting")
    print("listen: the 330 Hz chain is alive (source -> fx -> master)")
    time.sleep(2.0)

    # From now on the order maintains itself: a new source dropped at the
    # head still sounds, because every change re-sorts.
    print("adding a second source at the HEAD of the group (freq 495)")
    client.send("/s_new", "src", 1004, 0, GROUP, "freq", 495.0)
    time.sleep(2.0)

    client.send("/g_freeAll", GROUP)
    client.send("/n_free", GROUP)
    print("done — same commands, no /n_before juggling anywhere.")


if __name__ == "__main__":
    try:
        main()
    except (TimeoutError, OSError):
        sys.exit("no reply — is the server running? (cargo run --release)")
