"""Building GuiDefs the way defs are built.

A GuiDef is the GUI analogue of a ``SynthDef``/``GraphDef``: a tree of
``{id, type, ...props, children}`` nodes serialized to JSON and carried inside
one OSC argument. These helpers compose that tree as plain ``dict``s — they are
**host-agnostic**, just like building a ``SynthDef`` is server-agnostic; only
`clausters.gui.host.GuiHost` knows how to send one. The root node carries no
``id`` (it comes from the ``/gui_def <id>`` argument); every child carries its
own client-allocated integer id.

The int/float distinction is the user's to make and is preserved end to end:
write ``480`` for an integer property and ``480.0`` for a float — ``json.dumps``
keeps them apart in the JSON text and the host's serde parse keeps them apart on
the wire (ids stay integers, control values stay floats).
"""

import array
import json
import sys
from ..defs.ugens import env_to_points, points_to_env  # re-exported; shared with seq.automation

__all__ = [
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
    "canvas",
    "to_json",
    "samples_to_blob",
    "samples_to_file",
    "peaks_cache_file",
    "correlation",
    "lissajous",
]


def node(type: str, *, id: int | None = None, children=None, **props) -> dict:
    """A generic widget node ``{id?, type, ...props, children?}``.

    The building block every other helper wraps. Pass ``id`` for any non-root
    widget, ``children`` as an iterable of nodes for a container, and any other
    keyword as a property (kept verbatim, so its int/float type is preserved).
    """
    out: dict = {"type": type}
    if id is not None:
        out["id"] = id
    out.update(props)
    if children:
        out["children"] = list(children)
    return out


def window(*children, title: str | None = None, w: int | None = None, h: int | None = None,
           layout: str | None = None, **props) -> dict:
    """A top-level ``window`` container (a GuiDef root). It takes no id."""
    extra = _drop_none(title=title, w=w, h=h, layout=layout)
    return node("window", children=children, **extra, **props)


def panel(id: int, *children, layout: str | None = None, **props) -> dict:
    """A nestable ``panel`` container; ``layout`` is ``row``/``col``/``grid``/``free``."""
    extra = _drop_none(layout=layout)
    return node("panel", id=id, children=children, **extra, **props)


def label(id: int, text: str, **props) -> dict:
    """Static ``label`` text."""
    return node("label", id=id, text=text, **props)


def knob(id: int, *, label: str | None = None, min: float | None = None,
         max: float | None = None, value: float | None = None, **props) -> dict:
    """A rotary ``knob`` over a continuous range."""
    extra = _drop_none(label=label, min=min, max=max, value=value)
    return node("knob", id=id, **extra, **props)


def slider(id: int, *, label: str | None = None, min: float | None = None,
           max: float | None = None, value: float | None = None,
           vertical: bool = False, **props) -> dict:
    """A continuous ``slider`` over a range. ``vertical=True`` lays it out along
    the y axis (min at the bottom, max at the top) instead of horizontally."""
    extra = _drop_none(label=label, min=min, max=max, value=value)
    if vertical:
        extra["vertical"] = True
    return node("slider", id=id, **extra, **props)


def number(id: int, *, label: str | None = None, min: float | None = None,
           max: float | None = None, value: float | None = None, **props) -> dict:
    """A draggable numeric read-out over a range."""
    extra = _drop_none(label=label, min=min, max=max, value=value)
    return node("number", id=id, **extra, **props)


def button(id: int, *, label: str | None = None, **props) -> dict:
    """A momentary push ``button`` (emits ``1`` on press, ``0`` on release)."""
    extra = _drop_none(label=label)
    return node("button", id=id, **extra, **props)


def toggle(id: int, *, label: str | None = None, value: bool | None = None, **props) -> dict:
    """A boolean ``toggle``. ``value`` is sent as ``1``/``0`` (OSC has no bool)."""
    extra = _drop_none(label=label)
    if value is not None:
        extra["value"] = 1 if value else 0
    return node("toggle", id=id, **extra, **props)


def text(id: int, *, value: str | None = None, label: str | None = None, **props) -> dict:
    """A ``text`` field showing ``value`` (script-driven via ``/gui_set``)."""
    extra = _drop_none(value=value, label=label)
    return node("text", id=id, **extra, **props)


def menu(id: int, options, *, index: int | None = None, label: str | None = None, **props) -> dict:
    """A ``menu`` selector over ``options`` (a list of strings); a click cycles
    to the next and emits the chosen ``index``."""
    extra = _drop_none(index=index, label=label)
    return node("menu", id=id, options=list(options), **extra, **props)


def waveform(id: int, *, data=None, blob: int | None = None, buffer: int | None = None,
             path: str | None = None, cache: str | None = None, channels: int | None = None,
             base_bucket: int | None = None, overlay: bool | None = None,
             ruler: str | None = None, ruler_y: str | None = None,
             bit_depth: int | None = None, sample_rate: float | None = None,
             tempo: float | None = None, beat_at: float | None = None,
             quant: float | None = None, sel_start: float | None = None,
             sel_len: float | None = None, playhead_at: float | None = None,
             y_start: float | None = None, y_len: float | None = None,
             link: int | None = None, **props) -> dict:
    """The heavy ``waveform`` view, fed its samples one of several ways (in the
    host's precedence order):

    - ``cache`` — a path to a prebuilt peak-pyramid file (see `peaks_cache_file`)
      the host memory-maps and renders directly; the raw samples are never
      loaded. The most compact **bulk path**: nothing rides OSC. A cache built
      with ``channels > 1`` holds every channel in the one file.
    - ``path`` — a path to a file of raw little-endian ``f32`` samples (see
      `samples_to_file`, or the server's ``/b_export``) the host memory-maps; a
      **multi-megabyte buffer renders with no OSC and no re-send**.
    - ``buffer`` — a server buffer number; the host fetches its samples from the
      audio server over OSC (it must be started with ``--server``). The async
      fallback when a shared file is not available.
    - ``data`` — a small list of floats embedded inline in the JSON;
    - ``blob`` — the index of a binary blob carried beside the JSON in the same
      ``/gui_def`` message (see `samples_to_blob` and `GuiHost.define`).

    ``channels`` is the interleaved channel count of ``path``/``data``/``blob``
    (default 1): **every** channel is kept and drawn — stacked lanes sharing the
    time axis by default, or per-color overlaid traces with ``overlay=True``.
    ``base_bucket`` sets the peak-pyramid bucket size (default 256); for ``path``
    it also keys the sibling cache the host writes beside the file.

    The rulers (each in its own strip beside the view, each independently
    switchable off, all live via ``GuiHost.set`` — so a menu or button in the
    same GUI can retune them): ``ruler`` labels the time axis — ``"time"``
    (the default; clock time, using ``sample_rate`` or the rate the source
    brings), ``"samples"``, ``"beats"`` (musical time: ``tempo`` in beats per
    second — pass ``clock.tempo`` — ``beat_at`` the beat position of sample 0,
    ``quant`` the beats per bar, labels ``bar:beat``), or ``"off"``.
    ``ruler_y`` labels the amplitude axis — ``"norm"`` (the default;
    normalized [-1, 1]), ``"db"`` (dBFS), ``"bits"`` (integer sample values at
    the ``bit_depth`` resolution, default 16), ``"percent"`` (0-100% of full
    scale), or ``"off"``.

    The rest of the editor chrome: ``sel_start``/``sel_len`` set the selection
    in samples (dragging on the view updates it and emits
    ``/gui_event id "selection" start len``; Shift+drag pans, the wheel zooms).
    ``playhead_at`` draws a playhead tracking the engine sample clock: pass the
    ``/clock`` sample value that corresponds to buffer position 0 (negative or
    omitted = no playhead). ``y_start``/``y_len`` set the **vertical view
    window** — the visible slice of the amplitude axis, in normalized display
    units where ``0, 1`` (the default) is the full axis: the wheel over the
    y-ruler strip zooms it, dragging the strip pans it, and every change is
    reported as ``/gui_event id "view_y" y_start y_len`` (a non-positive
    ``y_len`` resets to the full axis).

    ``link`` puts the view in a shared **navigation group**: every timeline
    view (waveform or spectrogram, in any window) declaring the same ``link``
    id shares one horizontal view, selection and playhead — a zoom, pan or
    drag-selection on any member moves all of them, and setting
    ``view_start``/``view_len`` (samples; a non-positive ``view_len`` resets
    to the whole timeline), ``sel_start``/``sel_len`` or ``playhead_at`` via
    ``GuiHost.set`` on any member applies group-wide. Events still emit once,
    with the interacted member's id. Membership is live: set ``link`` to
    another group id to move the view, or to a negative value to unlink it
    (it keeps the view it had). Only the vertical window ``y_start``/``y_len``
    stays per-view."""
    extra = _drop_none(data=list(data) if data is not None else None,
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       channels=channels, base_bucket=base_bucket,
                       ruler=ruler, ruler_y=ruler_y, bit_depth=bit_depth,
                       sample_rate=sample_rate, tempo=tempo, beat_at=beat_at,
                       quant=quant, sel_start=sel_start, sel_len=sel_len,
                       playhead_at=playhead_at, y_start=y_start, y_len=y_len,
                       link=link)
    if overlay is not None:
        extra["overlay"] = 1 if overlay else 0
    return node("waveform", id=id, **extra, **props)


def spectrogram(id: int, *, data=None, blob: int | None = None, buffer: int | None = None,
                path: str | None = None, cache: str | None = None,
                channels: int | None = None, window_size: int | None = None,
                hop: int | None = None, sample_rate: float | None = None,
                db_floor: float | None = None, db_ceil: float | None = None,
                freq_scale: str | None = None, log_freq: bool | None = None,
                colormap: int | None = None, ruler: str | None = None,
                ruler_y: str | None = None, tempo: float | None = None,
                beat_at: float | None = None, quant: float | None = None,
                sel_start: float | None = None, sel_len: float | None = None,
                playhead_at: float | None = None, y_start: float | None = None,
                y_len: float | None = None, link: int | None = None,
                **props) -> dict:
    """The heavy ``spectrogram`` (STFT time-frequency) view, fed like the
    `waveform`: a mapped ``path`` of raw little-endian ``f32``, a server
    ``buffer``, inline ``data``/``blob``, or a prebuilt single-channel STFT
    ``cache`` file. ``channels`` de-interleaves the source (default 1); each
    channel gets its own analysis, drawn as stacked lanes sharing the time axis.

    The analysis: ``window_size`` is the FFT size (a power of two, default
    1024) and ``hop`` the frame advance (default ``window_size // 2``; the host
    raises it as needed so a long file fits the GPU texture). ``sample_rate``
    places the frequency axis for ``path``/inline sources (a fetched ``buffer``
    brings its own rate). The display is live (``GuiHost.set``): the dB window
    ``[db_floor, db_ceil]`` (default ``-90``/``0``) controls contrast,
    ``freq_scale`` picks the frequency axis — ``"log"`` (the default),
    ``"linear"``, ``"mel"`` or ``"bark"`` (``log_freq`` is the legacy boolean
    alias for the first two) — and ``colormap`` picks 0 viridis / 1 magma /
    2 grayscale.

    The rulers ride their own strips beside the view: ``ruler_y`` (``"hz"``,
    the default, or ``"off"``) draws the frequency ruler, its tick positions
    following ``freq_scale``; ``ruler`` labels the time axis exactly as on the
    `waveform` (``"time"``/``"samples"``/``"beats"`` with
    ``tempo``/``beat_at``/``quant``, or ``"off"``). The rest of the editor
    chrome (``sel_start``/``sel_len``, ``playhead_at``, drag-to-select /
    Shift+drag pan / wheel zoom) also works exactly as on the `waveform` —
    including the vertical view window ``y_start``/``y_len``, which here
    slices the **frequency display axis** (normalized, ``0, 1`` = the full
    axis, whatever the ``freq_scale``): wheel over the Hz-ruler strip zooms,
    dragging it pans, changes emit ``/gui_event id "view_y" y_start y_len``.

    ``link`` joins a shared navigation group exactly as on the `waveform` —
    the classic composition is a waveform lane and a spectrogram lane of the
    same render under one ``link``, scrolling and selecting in lockstep."""
    extra = _drop_none(data=list(data) if data is not None else None,
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       channels=channels, window_size=window_size, hop=hop,
                       sample_rate=sample_rate, db_floor=db_floor,
                       db_ceil=db_ceil, freq_scale=freq_scale,
                       colormap=colormap, ruler=ruler, ruler_y=ruler_y,
                       tempo=tempo, beat_at=beat_at, quant=quant,
                       sel_start=sel_start, sel_len=sel_len,
                       playhead_at=playhead_at, y_start=y_start, y_len=y_len,
                       link=link)
    if log_freq is not None:
        extra["log_freq"] = 1 if log_freq else 0
    return node("spectrogram", id=id, **extra, **props)


def meter(id: int, bus: int, *, min: float | None = None, max: float | None = None,
          label: str | None = None, **props) -> dict:
    """A level ``meter`` reading control ``bus`` straight from the audio server's
    shared-memory segment each frame (zero OSC messages). The host must be started
    with ``--shm`` pointing at the server's segment. ``min``/``max`` scale the bar
    (default ``0``/``1``)."""
    extra = _drop_none(min=min, max=max, label=label)
    return node("meter", id=id, bus=bus, **extra, **props)


def scope(id: int, bus: int = 0, *, tap: int | None = None,
          window_ms: float | None = None, trigger: float | None = None,
          hold: bool | None = None, min: float | None = None,
          max: float | None = None, label: str | None = None, **props) -> dict:
    """A time-domain ``scope``, in one of two rates. By default (control rate)
    it plots the recent history of control ``bus``, read from shared memory
    each frame (needs ``--shm`` like `meter`). Passing ``tap`` makes it an
    audio-rate **oscilloscope** over that audio-tap ring of the server (route
    a bus into it first with ``Server.tap``): a ``window_ms`` display window
    (default 20 ms), re-read every frame and aligned on a rising crossing of
    ``trigger`` (default ``0.0``, with hysteresis; free-running when the
    signal never crosses), so a periodic signal draws a stable trace.
    ``hold`` freezes the trace. Natively the host reads the tap out of the
    ``--shm`` segment with zero messages; in the browser it subscribes
    ``/tap_stream`` over the server leg. ``min``/``max`` set the vertical
    range (default the bipolar ``-1``/``1``)."""
    extra = _drop_none(tap=tap, window_ms=window_ms, trigger=trigger,
                       min=min, max=max, label=label)
    if hold is not None:
        extra["hold"] = 1 if hold else 0
    return node("scope", id=id, bus=bus, **extra, **props)


def phasescope(id: int, tap: int, tap2: int | None = None, *,
               window_ms: float | None = None, hold: bool | None = None,
               label: str | None = None, **props) -> dict:
    """A ``phasescope`` (goniometer): the two audio taps ``tap`` (left) and
    ``tap2`` (right, default ``tap + 1``) drawn as the 45°-rotated Lissajous
    figure — vertical is the mid ``(L + R)/√2``, horizontal the side
    ``(L - R)/√2``, the audio-engineering convention where mono reads as a
    vertical line, anti-phase as horizontal and a wide field fills the lozenge.
    An age-faded persistence trail spans the last ``window_ms`` of pairs (default
    30 ms) and a **correlation** read-out (Pearson's r over the window) sits
    under the field. Route each channel's bus into its tap first with
    ``Server.tap``; ``hold`` freezes the trace. Reads the segment natively
    (zero messages) and ``/tap_stream`` in the browser, like the oscilloscope."""
    extra = _drop_none(window_ms=window_ms, label=label)
    if tap2 is not None:
        extra["tap2"] = tap2
    if hold is not None:
        extra["hold"] = 1 if hold else 0
    return node("phasescope", id=id, tap=tap, **extra, **props)


def spectrum(id: int, tap: int, *, fft_size: int | None = None,
             db_floor: float | None = None, db_ceil: float | None = None,
             log_freq: bool | None = None, averaging: float | None = None,
             peak_hold: bool | None = None, label: str | None = None,
             **props) -> dict:
    """A live ``spectrum`` (spectroscope): one forward FFT per frame over the
    newest window of audio tap ``tap``, drawn as a magnitude curve. ``fft_size``
    is a power of two (256..4096, default 2048); the vertical axis is dB over
    ``[db_floor, db_ceil]`` (default ``-100``/``0``); ``log_freq`` (default true)
    selects a log frequency axis. Raw per-frame FFTs flicker, so ``averaging``
    (0..1, default 0.5) exponentially smooths each bin and ``peak_hold`` (default
    false) overlays a slowly decaying peak trace. Route a bus into the tap first
    with ``Server.tap``; the analysis uses the shared-core FFT and Hann window,
    so it agrees with the spectrogram. Native reads the segment; the browser
    subscribes ``/tap_stream``."""
    extra = _drop_none(fft_size=fft_size, db_floor=db_floor, db_ceil=db_ceil,
                       averaging=averaging, label=label)
    if log_freq is not None:
        extra["log_freq"] = 1 if log_freq else 0
    if peak_hold is not None:
        extra["peak_hold"] = 1 if peak_hold else 0
    return node("spectrum", id=id, tap=tap, **extra, **props)


def nodetree(id: int, *, group: int = 0, controls: bool | None = None,
             label: str | None = None, **props) -> dict:
    """A live ``nodetree`` view of the audio server's node tree rooted at ``group``
    (default the root group ``0``). The host mirrors the server's tree over its
    client leg (it must be started with ``--server``), refreshing on node
    creation/removal and a low-rate poll, so group/synth changes and ``/n_set``
    edits show live. ``controls`` (default true) shows each synth's control
    name/value pairs. A read-only view."""
    extra = _drop_none(label=label)
    if controls is not None:
        extra["controls"] = 1 if controls else 0
    return node("nodetree", id=id, group=group, **extra, **props)


def bpf(id: int, *, points=None, min: float | None = None, max: float | None = None,
        duration: float | None = None, exp: bool | None = None,
        label: str | None = None, **props) -> dict:
    """A drawable ``bpf`` break-point function — the envelope editor.

    Breakpoints ``(time, value)`` plus a per-segment shape using the server's
    own envelope shape numbers, evaluated host-side through the same shared
    math the server's ``EnvGen`` plays — what you draw is what you hear.
    ``points`` accepts either the flat quad list ``[t, v, shape, curve, ...]``
    (the wire form: shapes int, everything else float) or a list of tuples
    ``(time, value)`` / ``(time, value, shape)`` where ``shape`` is an
    `Env`-style curve spec — a name (``"lin"``, ``"exp"``, ``"sin"``,
    ``"step"``, ``"hold"``, ...) or a numeric curvature. Omitting ``points``
    draws a flat, immediately editable line. See `env_to_points` /
    `points_to_env` for the round trip with `clausters.defs.Env`.

    The model is general on purpose (the automation-lane shape): values live in
    ``[min, max]`` — unipolar (the ``0``/``1`` default), bipolar, or any
    parameter span; an on/off lane is the ``"hold"`` shape over ``0``/``1``
    (each point's value held until the next point — ``"step"``, per the
    SuperCollider semantics, instead jumps to the *target* level at segment
    start, so a step segment shows the next point's value);
    ``exp=True`` gives frequency-like ranges a geometric display scale
    (requires ``0 < min < max``). Times span ``[0, duration]`` (omitting
    ``duration`` fits the last point).

    Editing (drag a point — times stay monotonic; drag a segment vertically to
    bend its curvature; Ctrl+click adds a point, Ctrl+click on one removes it)
    flows back per the **edit-back pattern**:
    ``/gui_event <id> "points" <t v shape curve ...>`` to the script — or, when
    the widget is bound (`GuiHost.bind` or an inline ``bind``), the flat list
    is forwarded straight to the audio server after the binding's prefix.
    Setting is live too: ``GuiHost.set(id, points=json.dumps(flat))`` replaces
    the whole list (a ``/gui_set`` value is a scalar, so the array rides as its
    JSON string)."""
    extra = _drop_none(points=_flat_points(points) if points is not None else None,
                       min=min, max=max, duration=duration, label=label)
    if exp is not None:
        extra["exp"] = 1 if exp else 0
    return node("bpf", id=id, **extra, **props)


def _flat_points(points) -> list:
    """Normalizes a `bpf` ``points`` argument to the flat quad list: a flat
    number list is kept (validated to whole quads, shapes coerced int), tuples
    become ``t, v, shape, curve`` with the shape resolved like an `Env` curve
    spec (default linear)."""
    points = list(points)
    if not points:
        return []
    if not isinstance(points[0], (tuple, list)):
        if len(points) % 4 != 0:
            raise ValueError("a flat points list must be [t, v, shape, curve, ...] quads")
        return [int(x) if i % 4 == 2 else float(x) for i, x in enumerate(points)]
    from ..defs.ugens import _resolve_curve  # lazy: keep guidef import-light

    out: list = []
    for p in points:
        t, v = float(p[0]), float(p[1])
        shape, curve = _resolve_curve(p[2]) if len(p) > 2 else (1, 0.0)
        out += [t, v, int(shape), float(curve)]
    return out


def _flat_notes(notes) -> list:
    """Normalizes a ``notes`` argument to the flat **quintuple** wire form
    ``start dur pitch velocity channel`` (the canonical form the host reads for
    both the ``pianoroll`` and the ``clip`` roll). Each note is a
    ``(start, dur, pitch[, velocity[, channel]])`` tuple; a missing velocity
    defaults to 100, a missing channel to 0. ``velocity``/``channel`` stay ints
    so the JSON keeps them integral (the host distinguishes them from the float
    time/pitch)."""
    out: list = []
    for n in notes:
        n = list(n)
        start, dur, pitch = float(n[0]), float(n[1]), float(n[2])
        velocity = int(n[3]) if len(n) > 3 else 100
        channel = int(n[4]) if len(n) > 4 else 0
        out += [start, dur, pitch, velocity, channel]
    return out


def _flat_osc(osc) -> list:
    """Normalizes an ``osc`` argument to the flat ``time, label`` pairs the host
    reads. Each event is a ``(time, label)`` tuple or a bare ``time`` (no
    label); the label is coerced to a string (``""`` = none)."""
    out: list = []
    for e in osc:
        if isinstance(e, (tuple, list)):
            time = float(e[0])
            label = str(e[1]) if len(e) > 1 and e[1] is not None else ""
        else:
            time, label = float(e), ""
        out += [time, label]
    return out


def plot(id: int, *, data=None, blob: int | None = None, path: str | None = None,
         channels: int | None = None, min: float | None = None, max: float | None = None,
         label: str | None = None, **props) -> dict:
    """A simple static ``plot`` of a signal over ``[min, max]`` (default the
    bipolar ``-1``/``1``) — a line when the data fits the width, a min/max envelope
    when it does not. Unlike the heavy `waveform`, it does not zoom or pan; it is
    the catalog's "plot of an NRT-generated signal/file". Its samples come from:

    - ``path`` — a file of raw little-endian ``f32`` (see `samples_to_file`, or an
      NRT render written out) the host memory-maps; the **bulk path**, no OSC.
      ``channels`` de-interleaves channel 0 (default 1).
    - ``data`` — a small list of floats inline in the JSON;
    - ``blob`` — the index of a binary blob carried beside the JSON (see
      `samples_to_blob` and `GuiHost.define`).
    """
    extra = _drop_none(data=list(data) if data is not None else None,
                       blob=blob, path=path, channels=channels, min=min, max=max,
                       label=label)
    return node("plot", id=id, **extra, **props)


def track(id: int, *clips, label: str | None = None, height: float | None = None,
          snap: float | None = None, ruler: str | None = None,
          sample_rate: float | None = None, tempo: float | None = None,
          beat_at: float | None = None, quant: float | None = None,
          playhead_at: float | None = None, **props) -> dict:
    """A multitrack ``track`` lane holding `clip` children placed on a shared
    time axis — the DAW-style track editor's lane. ``label`` names it in a left
    header; ``height`` is its lane weight when several tracks stack under one
    window (a ``col`` layout). The window's tracks share one time axis, so a
    clip at a given offset lines up across lanes. ``snap`` is the drag grid in
    timeline samples a clip's move/resize rounds to (omitted / ``0`` = snap to
    whole samples).

    The lanes of a window **navigate as one**: they share a time axis you can
    zoom (wheel) and pan (Shift+drag), spanning the composition (the longest clip
    end over every lane, so dragging a clip past the end lengthens it). That is
    the same navigation group the heavy views use, so ``link`` joins or splits it
    — pass a shared id to align lanes across *windows*, or a distinct one to give
    a lane an axis of its own. Scripted navigation is ``GuiHost.set(track_id,
    view_start=…, view_len=…)``, and it applies group-wide.

    A lane carries the same time chrome as the heavy editor views:

    - ``ruler`` — a time ruler under the lane (``"time"``, ``"samples"``,
      ``"beats"``, or the default ``"off"``: a lane reserves no ruler strip
      unless asked). ``sample_rate`` labels real time, and ``tempo``/``beat_at``/
      ``quant`` label beats. One ruler under the bottom lane is the usual layout.
    - ``playhead_at`` — the engine sample-clock value at timeline position 0, so
      the playhead sweeps the clips as the composition plays (the same anchor the
      `waveform` uses; read the clock with ``Server.request("/clock")``). Set it
      live with ``GuiHost.set(track_id, playhead_at=clock)``; a negative value
      (the default) draws no playhead.

    Pass the clips positionally::

        track(1, clip(10, offset=0, dur=4, data=take_a),
                 clip(11, offset=4, dur=2, data=take_b), label="drums")
    """
    extra = _drop_none(label=label, height=height, snap=snap, ruler=ruler,
                       sample_rate=sample_rate, tempo=tempo, beat_at=beat_at,
                       quant=quant, playhead_at=playhead_at)
    return node("track", id=id, children=clips, **extra, **props)


def clip(id: int, *, offset: float = 0.0, dur: float, data=None, blob: int | None = None,
         buffer: int | None = None, path: str | None = None, cache: str | None = None,
         channels: int | None = None, base_bucket: int | None = None,
         notes=None, points=None, exp: bool | None = None,
         min: float | None = None, max: float | None = None,
         label: str | None = None, **props) -> dict:
    """One ``clip`` on a `track`: a placed rectangle spanning ``[offset, offset +
    dur]`` in timeline sample units (the graphic unit — length = duration). Its
    body is one of three:

    - a **waveform** — the take, drawn decimated to the clip's pixel width;
    - a **piano-roll** — ``notes``, an iterable of ``(start, dur, pitch)`` (or
      ``(start, dur, pitch, velocity, channel)``) events (times relative to the
      clip, in samples; pitch mapped over ``[min, max]``), drawn as note bars —
      the events-track view. The dedicated editor-grade `pianoroll` widget draws
      the same notes with a keyboard and editing; or
    - an **automation curve** — ``points``, break-points over the clip's span
      (the `bpf` editor's model and shape math, placed on a lane): times relative
      to the clip in samples, values over ``[min, max]`` (``exp=True`` gives a
      frequency-like range a geometric display scale). It is **editable in
      place** — drag a point, Ctrl+click to add one or remove the one under the
      cursor — and an edit flows back as the same flat ``"points"`` event the
      `bpf` view sends, so an `clausters.seq.Automation` consumes it either way.

    A real take is minutes long, so it never rides the wire as JSON. The
    waveform body reaches the clip exactly the ways the heavy `waveform` view's
    samples do, in the same precedence order:

    - ``cache`` — a prebuilt peak-pyramid file the host maps (see
      `peaks_cache_file`); the most compact bulk path, raw samples never loaded.
    - ``path`` — a file of raw little-endian ``f32`` the host maps (see
      `samples_to_file`); ``channels`` de-interleaves it, ``base_bucket`` sizes
      the pyramid built (and cached) on load. No OSC.
    - ``buffer`` — a server buffer, fetched over the host's client leg.
    - ``data``/``blob`` — a short body inline (a float list, or the index of a
      blob carried beside the JSON — see `samples_to_blob`); it must fit the
      datagram, so keep it to a sketch.

    Whichever the source, the body is summarized to fit the clip rectangle
    through the take's peak pyramid — the same "never resolve finer than the
    screen" rule the editor views follow.

    Other keywords:

    - ``offset`` — the clip's start on the shared timeline (samples; ``>= 0``).
    - ``dur`` — its duration (samples); a clip with no duration draws nothing.
      For an audio take placed 1:1, that is the take's frame count.
    - ``min``/``max`` — the waveform value range, or the low/high pitch of a
      piano-roll (default the bipolar ``-1``/``1``).

    Dragging a clip (move) or its edge (resize) flows back as a ``"clip"``
    event carrying the new ``offset``/``dur`` — the edit-back path — so a driver
    can update the composition model and re-realize."""
    extra = _drop_none(offset=offset,
                       data=list(data) if data is not None else None,
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       channels=channels, base_bucket=base_bucket,
                       notes=_flat_notes(notes) if notes is not None else None,
                       points=_flat_points(points) if points is not None else None,
                       min=min, max=max, label=label)
    if exp is not None:
        extra["exp"] = 1 if exp else 0
    return node("clip", id=id, dur=dur, **extra, **props)


def pianoroll(id: int, *, notes=None, osc=None, min: float | None = None,
              max: float | None = None, snap: float | None = None,
              velocity: bool | None = None, osc_lane: bool | None = None,
              midi_in: bool | None = None,
              link: int | None = None, ruler: str | None = None,
              sample_rate: float | None = None, tempo: float | None = None,
              beat_at: float | None = None, quant: float | None = None,
              sel_start: float | None = None, sel_len: float | None = None,
              playhead_at: float | None = None, playhead: float | None = None,
              y_start: float | None = None, y_len: float | None = None,
              label: str | None = None, **props) -> dict:
    """The dedicated editor-grade ``pianoroll`` view: a piano keyboard gutter, a
    note grid, an optional velocity lane and an OSC-event lane — the timeline
    sibling of the compact `clip` piano-roll body, drawing the **same notes** with
    the same geometry (they share the host's ``pianoroll`` primitives), plus
    editing, rulers and navigation.

    Content:

    - ``notes`` — an iterable of ``(start, dur, pitch)`` or ``(start, dur, pitch,
      velocity, channel)`` MIDI notes: times in timeline samples, ``pitch`` a MIDI
      note number drawn over the ``[min, max]`` window (default the 88-key range
      21–108), ``velocity`` ``0..127`` (default 100), ``channel`` ``0..15``. The
      notes are the MIDI messages the roll represents.
    - ``osc`` — an iterable of ``(time, label)`` (or bare ``time``) OSC events,
      drawn as flags in a lane below the grid — the OSC messages the roll carries
      alongside the notes.

    Editing (native gestures; the browser keeps display + ``/gui_set`` parity):
    drag a note to move it in time/pitch, drag an edge to resize it, Ctrl+click to
    add a note or remove the one under the cursor; drag in the velocity lane to
    set a note's velocity; Ctrl+click the OSC lane to add/remove an event, drag
    one to move it. ``snap`` is the drag grid in timeline samples (``0`` = whole
    samples). An edit flows back as a flat ``"notes"`` event (``start dur pitch
    velocity channel …``) or ``"osc"`` event (``time label …``) — the edit-back
    pattern — so a driver updates the model and re-realizes.

    Navigation and chrome mirror the heavy editor views: it is a timeline widget,
    so ``link`` joins/splits its navigation group (zoom with the wheel over the
    grid, pan with Shift+drag, all group-wide); ``ruler`` places a time ruler
    (``"time"``/``"samples"``/``"beats"``, default ``"time"``) with
    ``sample_rate``/``tempo``/``beat_at``/``quant`` labelling it; ``sel_start``/
    ``sel_len`` mark a time selection; ``playhead_at`` sweeps a playhead from the
    engine clock (``playhead`` sets a static cursor); ``y_start``/``y_len`` are the
    vertical pitch window (normalized ``0..1`` over ``[min, max]``) for pitch
    zoom/pan. ``velocity=False`` hides the velocity lane; ``osc_lane=True`` opens
    the OSC lane even with no events (to author them). ``midi_in=True`` arms
    **live MIDI painting** in the native host: it opens a virtual MIDI input
    port ("clausters-gui") and paints incoming notes into this roll — at the
    running playhead, or step-entering on the ``snap`` grid when the transport
    is stopped — flowing back as the usual ``"notes"`` events (the standalone
    host's live input; a script can equally paint via a `clausters.responders.
    MidiFunc` and ``/gui_set``)."""
    extra = _drop_none(
        notes=_flat_notes(notes) if notes is not None else None,
        osc=_flat_osc(osc) if osc is not None else None,
        min=min, max=max, snap=snap, link=link, ruler=ruler,
        sample_rate=sample_rate, tempo=tempo, beat_at=beat_at, quant=quant,
        sel_start=sel_start, sel_len=sel_len, playhead_at=playhead_at,
        playhead=playhead, y_start=y_start, y_len=y_len, label=label)
    if velocity is not None:
        extra["velocity"] = 1 if velocity else 0
    if osc_lane is not None:
        extra["osc_lane"] = 1 if osc_lane else 0
    if midi_in is not None:
        extra["midi_in"] = 1 if midi_in else 0
    return node("pianoroll", id=id, **extra, **props)


def graph(id: int, *, members=None, buses=None, wires=None,
          label: str | None = None, **props) -> dict:
    """A ``graph`` **patcher**: a bus-wired node graph (a `clausters.defs.GraphDef`)
    drawn as member boxes, bus nodes, and a wire per connection — the *logical*
    side of a composition, where materials relate by processing rather than by
    time.

    The view is deliberately **bipartite**, because that is what a GraphDef knows:
    a member's control *touches* a bus. Which end writes and which reads is the
    server's own analysis (it sorts the graph), so the patch shows the connection
    and leaves the direction to the engine.

    - ``members`` — the nodes, each ``(def_name, [control, …])``: the def and the
      controls that are wired (each drawn as a port on its box).
    - ``buses`` — the internal bus names, plus ``"OUT"`` (the hardware) when used.
    - ``wires`` — the connections, each ``(member_index, control, bus)``.

    Dragging a port onto a bus **rewires** that control; dropping it on empty
    space unwires it. Either way the edit flows back as ``/gui_event <id> "wire"
    <member> <control> <bus>`` (an empty bus = unwired), so a driver updates the
    logical group and re-realizes it — the same edit-back pattern the clips use.
    """
    extra = _drop_none(
        members=[{"name": str(name), "ports": [str(c) for c in ports]}
                 for name, ports in members] if members is not None else None,
        buses=[str(b) for b in buses] if buses is not None else None,
        wires=[x for w in wires for x in (int(w[0]), str(w[1]), str(w[2]))]
        if wires is not None else None,
        label=label)
    return node("graph", id=id, **extra, **props)


def canvas(id: int, shader: str | None = None, *, params=None, buses=None,
           label: str | None = None, **props) -> dict:
    """A ``canvas`` running a script-supplied WGSL shader over the widget area --
    custom visuals (ShaderToy-style).

    ``shader`` is the body of a ``shade`` function the host wraps and runs::

        fn shade(uv: vec2<f32>, frag: vec4<f32>) -> vec4<f32> { ... }

    Inside it, the host exposes ``u.resolution`` (the viewport size in px),
    ``u.time`` (seconds), and ``u.params`` (a ``vec4<f32>`` of four values). The
    four params are driven two ways, which is the point of the widget:

    - from the **script** -- ``GuiHost.set(id, param0=...)`` sends an OSC value
      that lands in ``u.params.x`` (``param0``..``param3`` -> ``.x``..``.w``);
    - from a **control bus**, read straight from the audio server's shared memory
      each frame (zero messages) -- ``buses=[busA, busB, ...]`` maps each control
      bus onto the param of the same index; a ``-1`` (or absent) slot stays
      script-driven. Needs the host started with ``--shm`` (like `meter`).

    So a shader can animate from OSC parameters and from live server audio at
    once. Omitting ``shader`` uses a default moving color field. ``params`` is an
    optional initial list of floats."""
    extra = _drop_none(shader=shader, label=label,
                       params=[float(x) for x in params] if params is not None else None,
                       buses=[int(b) for b in buses] if buses is not None else None)
    return node("canvas", id=id, **extra, **props)


def to_json(tree: dict) -> str:
    """Serializes a GuiDef tree to the JSON string carried in ``/gui_def``."""
    return json.dumps(tree)


def samples_to_blob(samples) -> bytes:
    """Packs an iterable of floats into a little-endian ``f32`` blob, the bulk
    form a ``waveform`` reads via ``blob``. Flat bytes at the boundary — the same
    rule the rest of the client follows."""
    buf = array.array("f", samples)
    if sys.byteorder != "little":
        buf.byteswap()
    return buf.tobytes()


def samples_to_file(samples, path: str) -> str:
    """Writes `samples` to `path` as raw little-endian ``f32`` — the **local
    shared resource** a ``waveform(path=...)`` maps. Unlike `samples_to_blob`
    (which rides the ``/gui_def`` message and so must fit a datagram), a file has
    no size limit: this is how a multi-megabyte buffer reaches the host without
    OSC. Returns `path`."""
    buf = array.array("f", samples)
    if sys.byteorder != "little":
        buf.byteswap()
    with open(path, "wb") as f:
        f.write(buf.tobytes())
    return path


def peaks_cache_file(samples, path: str, base_bucket: int = 256, channels: int = 1) -> str:
    """Builds the peak-pyramid cache for `samples` (via the shared native core,
    so it is byte-identical to the host's own) and writes it to `path` — the most
    compact bulk path, mapped by a ``waveform(cache=...)``. The host renders the
    overview without ever loading the raw samples. With ``channels > 1`` the
    samples are interleaved frames and the file is the **multichannel** cache
    (one resource, a pyramid per channel — the editor-grade stacked lanes).
    Returns `path`."""
    from .._native import peaks_cache  # lazy: only needs the cdylib if used

    with open(path, "wb") as f:
        f.write(peaks_cache(samples, base_bucket, channels))
    return path


def correlation(left, right) -> float | None:
    """The stereo **correlation** (Pearson's r) of two equal-length channels,
    in ``[-1, 1]`` — ``+1`` mono/in-phase, ``0`` decorrelated, ``-1`` anti-phase
    — via the shared native core, so a headless capture reads the identical
    number the GUI phasescope draws. ``None`` when it is undefined (empty input
    or a constant channel: silence/DC). Pair it with ``Server.stream_taps`` to
    measure a live stereo signal without the GUI."""
    from .._native import correlation as _correlation  # lazy: needs the cdylib

    return _correlation(left, right)


def lissajous(left, right) -> list:
    """The **Lissajous / goniometer** coordinates of stereo pairs ``(left,
    right)``: each maps to ``(x, y)`` with ``x`` the side ``(L - R)/√2`` and
    ``y`` the mid ``(L + R)/√2`` — the rotated stereo plane a goniometer draws.
    The geometry lives once in the shared native core (the phasescope draws the
    same points); useful for plotting or driving a stereo image in
    electroacoustic work. Returns a list of ``(x, y)`` tuples."""
    from .._native import lissajous as _lissajous  # lazy: needs the cdylib

    return _lissajous(left, right)


def _drop_none(**kwargs) -> dict:
    """Keeps only the keyword arguments that were actually given."""
    return {k: v for k, v in kwargs.items() if v is not None}
