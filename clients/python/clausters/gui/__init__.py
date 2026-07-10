"""Clausters GUI client: build GuiDefs and drive the ``clausters-gui`` host.

The GUI host is a second peer in the Clausters system — a *GUI server* for the
language clients and itself a *client of the audio server* — driven over the
same OSC encoding the audio server uses, with the ``/gui_*`` vocabulary. This
submodule is the Python side of that protocol: `clausters.gui.guidef` composes a
widget tree as JSON (the GUI analogue of a ``SynthDef``), and
`clausters.gui.host.GuiHost` sends it and reads widgets back.

The windowed host opens an OS window per ``window``-rooted GuiDef and renders
containers, the standard controls, the heavy editor-grade ``waveform`` and
``spectrogram`` views (multichannel lanes, time and Hz rulers, a draggable
selection, a playhead tracking the engine clock), the
shared-memory-backed ``meter``/``scope``, the audio-tap ``scope``
(oscilloscope), ``phasescope`` (goniometer) and ``spectrum`` (spectroscope), a
live ``nodetree`` of the server's node graph, a static ``plot`` of a signal and
a drawable ``bpf`` envelope editor (the server's own ``EnvGen`` segment shapes;
edits flow back as flat ``"points"`` events — see `env_to_points` /
`points_to_env` for the `clausters.defs.Env` round trip);
live updates flow through ``/gui_set`` and interactions come back as
``/gui_event``/``/gui_closed``. A ``waveform`` can also name a server buffer, a
``meter``/``scope`` reads a control bus from the audio server's shared-memory
segment, and the audio-rate views read its audio taps (route a bus into a tap
with ``Server.tap``). A widget can also be *bound*
(`clausters.gui.host.GuiHost.bind`) so its value flows straight to the audio
server, bypassing this script. A ``canvas`` runs a script-supplied WGSL shader
driven by OSC params and control buses. The stereo-field measurements the
phasescope draws — `correlation` and `lissajous` — are also exposed as plain
functions (shared with the native host through ``clausters-core``) for headless
analysis. See the ``examples/gui_*.py`` family (``gui_skeleton`` for the
headless protocol, ``gui_window``/``gui_panel`` for windows and controls,
``gui_meters`` for the audio-server client paths, ``gui_bind`` for the bound
low-latency control path, ``gui_nodetree`` for the live node tree, ``gui_plot``
for an NRT render plotted, ``gui_canvas`` for a shader, ``gui_scope`` for the
oscilloscope, ``gui_analyzer`` for the phasescope and live spectrum,
``gui_editor`` for the editor-grade waveform + spectrogram,
``gui_bpf`` for the drawable envelope editor).
"""

from .guidef import (
    bpf,
    button,
    canvas,
    correlation,
    env_to_points,
    knob,
    label,
    lissajous,
    menu,
    meter,
    node,
    nodetree,
    number,
    panel,
    peaks_cache_file,
    phasescope,
    plot,
    points_to_env,
    samples_to_blob,
    samples_to_file,
    scope,
    slider,
    spectrogram,
    spectrum,
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
    "spectrogram",
    "meter",
    "scope",
    "phasescope",
    "spectrum",
    "nodetree",
    "bpf",
    "env_to_points",
    "points_to_env",
    "plot",
    "canvas",
    "samples_to_blob",
    "samples_to_file",
    "peaks_cache_file",
    "correlation",
    "lissajous",
]
