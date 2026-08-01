#!/usr/bin/env python3
"""Naming groups: a mixer whose channels are addressed by path, not by id.

A group is the closest thing the node tree has to a DAW channel — a handle for
many nodes, a place in the order, a lifetime — but a bare group is called
``1002``, and nothing in a client's code says which channel that is. A group
**name** fixes exactly that: a referenceable label on top of the node id. The id
stays the identity (every command still addresses the group by id, and every
reply still reports it); the name is how you *refer* to the group, and it makes
the tree navigable by path.

This builds a small console:

    /mixer/drums  (voice -> bus 16)  \\
                                       -> /mixer  (sums 16, 17 into 18)
    /mixer/bass   (voice -> bus 17)  /
                                                  -> /master  (18 -> hardware)

and then drives it entirely by path — ``server.group_at("/mixer/drums")`` —
without a single node id written down. It also shows the two other places a name
comes back: ``print(tree)``, which draws the label next to the id, and
``dump_graph``, which quotes it. The ``/node_start`` and ``/node_end``
notifications carry it too, so a client watching the tree sees *which* channel
came up or went away.

The rules a name obeys, all enforced by the server: unique among siblings (the
same name under two different parents is the point — ``/mixer/drums`` and
``/master/drums`` are different channels), never all digits (an unnamed group
answers to its id in a path, so a numeric name would be ambiguous), and no
``/`` (the server composes the path; the client names one group at a time). A
name carried by ``/group_new`` is judged before the group exists, so a refused
label refuses the creation — you never end up with an anonymous group.

No Faust needed; a plain server build works.

    cargo build --release
    python3 examples/mixer_paths.py

Point it at a prebuilt binary with ``CLAUSTERS_BIN=/path/to/clausters``.
"""

import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.defs import (AddAction, Group, Server, ServerOptions, Synth,
                            SynthDef, control, in_, out, sine)

REPO = os.path.join(os.path.dirname(__file__), "..")
BIN = os.environ.get("CLAUSTERS_BIN", os.path.join(REPO, "target", "release", "clausters"))

# The private audio buses the console runs on: one per channel, one for the mix.
DRUMS_BUS, BASS_BUS, MIX_BUS = 16, 17, 18


def launch() -> subprocess.Popen:
    if not os.path.exists(BIN):
        sys.exit(f"server binary not found at {BIN}\n"
                 "build it with: cargo build --release  (or set CLAUSTERS_BIN)")
    return subprocess.Popen([BIN, "--no-persist"])


def connect(timeout: float = 8.0) -> Server:
    server = Server(options=ServerOptions())
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            server.query_info(timeout=0.3)
            return server
        except Exception:
            time.sleep(0.2)
    server.close()
    raise RuntimeError("server did not come up in time")


def send_defs(server: Server):
    """A voice that writes to a bus, a channel strip that sums one bus into
    another with a gain, and a master that lands the mix on the hardware."""
    SynthDef("voice", out(control("bus", 0.0),
                          sine(control("freq", 220.0)) * 0.2)).send(server)
    SynthDef("strip", out(control("to", 0.0),
                          in_(control("from", 0.0)) * control("amp", 1.0))).send(server)
    SynthDef("master", out(0.0, in_(control("from", 0.0)))).send(server)
    server.sync()


def build_console(server: Server):
    """The console, named as it is built: a group is born with its label, in
    the same message that creates it, so the tree is readable from the first
    message and never passes through an anonymous state."""
    mixer = Group("mixer", server=server)
    master = Group("master", target=mixer, action=AddAction.AFTER, server=server)

    for name, bus, freq in (("drums", DRUMS_BUS, 110.0), ("bass", BASS_BUS, 82.5)):
        # Each channel is a group of its own inside the mixer: the voice that
        # sounds it and the strip that sums it into the mix bus.
        channel = Group(name, target=mixer, action=AddAction.TAIL, server=server)
        Synth("voice", {"bus": bus, "freq": freq},
              target=channel, action=AddAction.TAIL, server=server)
        Synth("strip", {"from": bus, "to": MIX_BUS, "amp": 0.5},
              target=channel, action=AddAction.TAIL, server=server)

    Synth("master", {"from": MIX_BUS},
          target=master, action=AddAction.TAIL, server=server)
    time.sleep(0.2)   # let the commands apply before querying
    return mixer, master


def main():
    proc = launch()
    server = connect()
    try:
        send_defs(server)
        mixer, master = build_console(server)

        # The label comes back in every node record, so the tree reads as the
        # console it is instead of as a list of numbers.
        print("query_tree() — the names are part of the record:")
        print(server.query_tree())

        # And it resolves: a path in, a group handle out. From here on the
        # channel is driven by what it *is*, with no id written down.
        drums = server.group_at("/mixer/drums")
        bass = server.group_at("/mixer/bass")
        print(f"\n/mixer/drums resolves to node {drums.id}, /mixer/bass to {bass.id}")

        print("riding the drums fader by path (listen to the balance move)...")
        for amp in (0.9, 0.2, 0.5):
            drums.set({"amp": amp})   # reaches the strip inside the channel
            time.sleep(0.6)

        # A name is a label, not an identity: renaming changes nothing about
        # the node, and re-paths its whole subtree at once.
        mixer.rename("board")
        print(f"renamed /mixer to /board — same node {mixer.id}, new path:")
        print(f"  /board/drums -> {server.group_at('/board/drums').id}"
              f"   /mixer/drums -> {server.group_at('/mixer/drums')}")

        # The label is quoted in the debug dump too, next to the id.
        print(f"\ndump_graph({mixer.id}):")
        print(server.dump_graph(mixer.id), end="")

        # An unnamed group is still reachable: it contributes its id as the
        # path segment, so nothing falls out of addressing.
        anon = Group(target=master, action=AddAction.HEAD, server=server)
        time.sleep(0.1)
        print(f"\nan unnamed group under /master answers to its id: "
              f"/master/{anon.id} -> {server.group_at(f'/master/{anon.id}').id}")

        mixer.free()
        master.free()
    finally:
        try:
            server.quit()
        except Exception:
            pass
        server.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.terminate()


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
