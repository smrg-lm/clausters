#!/usr/bin/env python3
"""Drive a whole group with one `/n_set` — scsynth group semantics.

A command addressed to a **group** transfers the named parameters down its
subtree to every synth that has a control with that name (recursing through
subgroups, stopping at each synth). So one `server.set(group, {...})` reaches
every voice in the group at once — the cheapest way to move a parameter across
a bank of nodes without naming each one.

Here three `default` voices share a group; a single `/n_set` on the group
ramps the amplitude of all three together. Renders offline (NRT), so it needs
the embed library once:

    cargo build --release --features embed,realtime

then:

    python3 examples/group_set.py [out.wav]
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))

from clausters.base import OscNrtInterface, Routine, TempoClock
from clausters.defs import Server

SR = 48000.0
FREQS = (220.0, 277.0, 330.0)  # an A-ish triad


def play(server):
    # A group holding three voices, all starting silent (amp 0).
    bank = server.group()
    for freq in FREQS:
        server.synth("default", {"freq": freq, "amp": 0.0}, target=bank.id)

    # One /n_set on the GROUP ramps every voice's amp at once (propagation):
    # no per-voice bookkeeping, the server fans it out to the subtree. Live you
    # would write `server.set(bank, {"amp": amp})`; to *schedule* it on the
    # clock (here, an offline score) use `send_bundle`, which stamps each
    # message with the routine's logical beat.
    for amp in (0.06, 0.12, 0.18, 0.0):
        server.send_bundle(("/n_set", bank.id, "amp", amp))
        yield 0.4

    server.send_bundle(("/n_free", bank.id))  # frees the group and its subtree


def main():
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)
    clock.play(Routine(lambda: play(server)))
    clock.render()
    path = sys.argv[1] if len(sys.argv) > 1 else None
    stats = server.render(sample_rate=SR, channels=2, path=path)

    peak = max(stats.peak)
    print(f"rendered {stats.frames} frames ({stats.duration:.3f} s) | peak {peak:.3f}")
    if peak < 0.05:
        sys.exit("unexpectedly quiet: the group /n_set did not reach the voices")
    print("one /n_set on the group drove all three voices.")

    if path:
        print(f"wrote {path} — listen with: pw-play {path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
