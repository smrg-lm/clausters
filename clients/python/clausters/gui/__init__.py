"""Clausters GUI client: build GuiDefs and drive the ``clausters-gui`` host.

The GUI host is a second peer in the Clausters system — a *GUI server* for the
language clients and itself a *client of the audio server* — driven over the
same OSC encoding the audio server uses, with the ``/gui_*`` vocabulary. This
submodule is the Python side of that protocol: `clausters.gui.guidef` composes a
widget tree as JSON (the GUI analogue of a ``SynthDef``), and
`clausters.gui.host.GuiHost` sends it and reads widgets back.

The windowed host opens an OS window per ``window``-rooted GuiDef and renders
containers, the standard controls, the heavy ``waveform`` view and the
shared-memory-backed ``meter``/``scope``; live updates flow through ``/gui_set``
and interactions come back as ``/gui_event``/``/gui_closed``. A ``waveform`` can
also name a server buffer, and a ``meter``/``scope`` reads a control bus from the
audio server's shared-memory segment. See the ``examples/gui_*.py`` family
(``gui_skeleton`` for the headless protocol, ``gui_window``/``gui_panel`` for
windows and controls, ``gui_meters`` for the audio-server client paths).
"""

from .guidef import (
    button,
    knob,
    label,
    menu,
    meter,
    node,
    number,
    panel,
    samples_to_blob,
    scope,
    slider,
    text,
    toggle,
    waveform,
    window,
)
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
    "number",
    "button",
    "toggle",
    "text",
    "menu",
    "waveform",
    "meter",
    "scope",
    "samples_to_blob",
]
