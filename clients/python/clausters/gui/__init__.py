"""Clausters GUI client: build GuiDefs and drive the ``clausters-gui`` host.

The GUI host is a second peer in the Clausters system — a *GUI server* for the
language clients and itself a *client of the audio server* — driven over the
same OSC encoding the audio server uses, with the ``/gui_*`` vocabulary. This
submodule is the Python side of that protocol: `clausters.gui.guidef` composes a
widget tree as JSON (the GUI analogue of a ``SynthDef``), and
`clausters.gui.host.GuiHost` sends it and reads widgets back.

At this milestone the host is a headless skeleton (no window yet): it registers
and introspects the widget tree and answers ``/gui_query``. See
``examples/gui_skeleton.py``.
"""

from .guidef import knob, label, node, panel, samples_to_blob, slider, waveform, window
from .host import DEFAULT_PORT, GuiHost

__all__ = [
    "GuiHost",
    "DEFAULT_PORT",
    "node",
    "window",
    "panel",
    "label",
    "knob",
    "slider",
    "waveform",
    "samples_to_blob",
]
