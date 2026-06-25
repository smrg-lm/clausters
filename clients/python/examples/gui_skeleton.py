#!/usr/bin/env python3
"""Drive the headless ``clausters-gui`` host: build a GuiDef, read a widget back.

The smallest round trip over the widget protocol, the GUI counterpart of
``live_udp.py``. A GuiDef is built exactly the way a ``SynthDef``/``GraphDef``
is — a tree of ``{id, type, ...props, children}`` nodes serialized to JSON — and
sent in one ``/gui_def`` message; the host registers the tree and answers
``/gui_query`` with ``/gui_info``. There is no window yet (the host is a
skeleton at this milestone): this exercises the protocol and the dual-role host.

Start the host in one terminal (built from ``clients/gui``)::

    cd clients/gui && cargo run --bin clausters-gui -- -v

then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_skeleton.py

The host logs the parsed widget tree; this script prints the widget it reads
back over ``/gui_info``.
"""

import sys

from clausters.gui import GuiHost, knob, slider, waveform, window


def filter_panel() -> dict:
    """A small instrument panel: two controls and a waveform view. The root
    ``window`` carries no id (the id comes from the ``/gui_def`` argument); each
    child carries its own client-allocated integer id."""
    return window(
        knob(10, label="cutoff", min=20.0, max=20000.0, value=800.0),
        slider(11, label="res", min=0.0, max=1.0, value=0.2),
        waveform(12, buffer=0),
        title="Filter", w=480, h=240, layout="col",
    )


def main():
    with GuiHost() as gui:  # 127.0.0.1:57210 by default
        # One declarative message builds the whole tree under def id 1.
        gui.define(1, filter_panel())

        # Read a widget back: /gui_query 10 -> /gui_info. The float `value`
        # comes back as a float and the int `buffer` as an int -- the wire keeps
        # them apart.
        info = gui.query(10)
        if info is None:
            sys.exit("no /gui_info reply -- is the clausters-gui host running on 57210?")
        kind, props = info
        print(f"widget 10 is a {kind!r} with {props}")

        root = gui.query(1)
        print(f"root (def 1) is a {root[0]!r}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
