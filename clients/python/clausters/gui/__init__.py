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
playable ``piano`` virtual keyboard (real key proportions, MIDI-shaped note
events, optionally host-managed server voices), a
live ``nodetree`` of the server's node graph, a static ``plot`` of a signal and
a drawable ``bpf`` envelope editor (the server's own ``EnvGen`` segment shapes;
edits flow back as flat ``"points"`` events — see `env_to_points` /
`points_to_env` for the `clausters.defs.Env` round trip), a multitrack
``track``/``clip`` timeline (clips placed by ``offset``/``dur`` on one shared
time axis — the DAW-style track editor, whose clip bodies are a take, a
piano-roll or an editable automation curve) and a ``patch`` **patcher** of a
directed, typed signal graph (drag an outlet onto an inlet to wire it);
live updates flow through ``/gui_set`` and interactions come back as
``/gui_event``/``/gui_closed``. `clausters.gui.editor.Editor` drives that
multitrack view from a `clausters.form` composition — drawing the arrangement
tree as a GuiDef, applying the clip edit-backs onto it, and re-rendering it.
`clausters.gui.transport.Transport` is the play/pause/stop/locate machinery
every time view shares: it drives a `clausters.seq.Playhead` and the view's
playhead line together, whatever the view is — a lane, a piano-roll, an engraved
page. A ``waveform`` can also name a server buffer, a
``meter``/``scope`` reads a control bus from the audio server's shared-memory
segment, and the audio-rate views read its audio taps (route a bus into a tap
with ``Bus.watch``). A widget can also be *bound*
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
``gui_bpf`` for the drawable envelope editor,
``gui_multitrack`` for the DAW-style track timeline,
``gui_oscsend`` for the editable ``text`` field typing an OSC message live).
"""

from .editor import Editor
from .guidef import (
    bpf,
    button,
    canvas,
    clip,
    correlation,
    curve,
    env_to_points,
    field,
    keys,
    knob,
    label,
    layout,
    lissajous,
    menu,
    meter,
    node,
    nodes,
    nodetree,
    notes,
    number,
    panel,
    patch,
    plane,
    peaks_cache_file,
    peaks_cache_empty_file,
    peaks_cache_stream_file,
    peaks_cache_update_file,
    phasescope,
    piano,
    pianoroll,
    plot,
    points_to_env,
    samples_to_blob,
    samples_to_file,
    score,
    scope,
    scroll,
    signal,
    slider,
    spectrogram,
    spectrum,
    stack,
    text,
    toggle,
    timeruler,
    track,
    waveform,
    window,
)
from .handle import WidgetHandle, WindowHandle
from .view import View
from .host import DEFAULT_PORT, GuiHost
from .transport import Transport

#: The host registered by `set_ambient_host`, if any.
_ambient = None


def set_ambient_host(host):
    """Register the host the ambient visual verbs open their windows on, and
    return the one previously registered (``None`` if there was none).

    `clausters.plot` and `clausters.scope` resolve a host without being told
    one: the current session's, else a ``clausters-gui`` process they boot and
    own. That fallback assumes the host is a local process, which is the one
    assumption an out-of-process front cannot meet — a host living somewhere
    this module knows nothing about (a canvas reached over a carrier of the
    caller's own, a test double collecting packets) has no process to boot.
    Registering one here puts it ahead of the fallback, so the verbs keep
    working unchanged and neither they nor `GuiHost` learn where it runs.

    Pass ``None`` to unregister and restore the ordinary resolution. The
    registered host is *not* owned here: whoever registered it stops it.
    """
    global _ambient
    previous, _ambient = _ambient, host
    return previous


def ambient_host():
    """The host registered by `set_ambient_host`, or ``None``."""
    return _ambient


__all__ = [
    "GuiHost",
    "DEFAULT_PORT",
    "set_ambient_host",
    "ambient_host",
    "Editor",
    "Transport",
    "WidgetHandle",
    "WindowHandle",
    "View",
    "node",
    "window",
    "layout",
    "plane",
    "field",
    "signal",
    "notes",
    "curve",
    "nodes",
    "keys",
    "panel",
    "scroll",
    "stack",
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
    "piano",
    "pianoroll",
    "meter",
    "scope",
    "phasescope",
    "spectrum",
    "nodetree",
    "bpf",
    "env_to_points",
    "points_to_env",
    "plot",
    "score",
    "timeruler",
    "track",
    "clip",
    "patch",
    "canvas",
    "samples_to_blob",
    "samples_to_file",
    "peaks_cache_file",
    "peaks_cache_empty_file",
    "peaks_cache_stream_file",
    "peaks_cache_update_file",
    "correlation",
    "lissajous",
]
