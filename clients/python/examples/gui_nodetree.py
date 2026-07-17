#!/usr/bin/env python3
"""A live node-tree view: the GUI mirroring the audio server's node graph.

The first half of the G8 pair of read-only views. A ``nodetree`` widget shows
the audio server's tree -- its groups, synths, def names and control values --
and keeps it current: the GUI host mirrors the tree over its **client leg**
(``/g_queryTree``), refreshing the moment a node is created or freed
(``/n_go``/``/n_end`` notifications) and on a low-rate poll that catches
``/n_set`` control changes. Nothing in this script pushes the tree to the GUI;
the host reads it from the server itself.

Three processes cooperate, as in ``gui_meters.py``: the **audio server**, the
**GUI host** (which must be started with ``--server`` so it can query the tree),
and this **script**, which only builds nodes and edits their controls.

Start the audio server (from the repo root)::

    cargo run

Start the windowed GUI host attached to it (from ``clients/gui``)::

    cargo run --bin clausters-gui -- --server 127.0.0.1:57110 -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_nodetree.py

A window opens showing the live tree. Watch a synth's ``freq`` sweep (an
``/n_set`` each tick) and a third synth come and go (a group child appearing and
disappearing) -- both update in the view with no message from this script to the
host. Close the window, or wait, to end.
"""

import math
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.defs.node import AddAction
from clausters.gui import GuiHost, nodetree, window


def scene() -> dict:
    """A window whose only widget is a node tree rooted at the root group (0)."""
    return window(
        nodetree(10, group=0, controls=True, label="node tree"),
        title="Live node tree", w=420, h=520,
    )


def main():
    with Session.live() as session:  # UDP to 127.0.0.1:57110
        server = session.server
        server.add_synthdef(SynthDef(
            "beep", out(0.0, sine(control("freq", 220.0)) * control("amp", 0.1))))
        group = server.group()
        sweeper = server.synth("beep", {"freq": 220.0}, target=group.id, action=AddAction.TAIL)
        server.synth("beep", {"freq": 330.0}, target=group.id, action=AddAction.TAIL)

        with GuiHost() as gui:  # 127.0.0.1:57210 by default
            gui.define(1, scene())
            print("the window mirrors the server's node tree; watch freq sweep and "
                  "a synth come and go; close the window to stop")

            extra = None
            start = time.monotonic()
            while time.monotonic() - start < 30.0:
                t = time.monotonic() - start
                # /n_set: sweep one synth's freq -> its control value updates live
                # in the tree (no /n_set notification, so the host's poll shows it).
                server.set(sweeper, {"freq": 330.0 + 220.0 * math.sin(t)})
                # A third synth appears and disappears -> the host catches the
                # /n_go and /n_end and the tree grows then shrinks.
                if extra is None and int(t) % 4 == 2:
                    extra = server.synth("beep", {"freq": 550.0, "amp": 0.08},
                                         target=group.id, action=AddAction.TAIL)
                elif extra is not None and int(t) % 4 == 0:
                    server.free(extra)
                    extra = None

                msg = gui.poll(timeout=0.1)
                if msg is not None and msg[0] == "/gui_closed":
                    print("window closed")
                    break
            server.free(group)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
