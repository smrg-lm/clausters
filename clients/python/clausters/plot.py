"""The free-standing ``plot`` — one verb for looking at a signal.

`plot` is the visual sibling of `clausters.play`: it plots whatever you hand
it, resolving the ambient context so you never spell out a host or a renderer
for a quick look. Each call opens its **own window** on the GUI host (booted
lazily on first use; plots ride the bulk file path, so no audio server is
involved unless the object itself needs one). It dispatches by kind:

- a **def** — a `clausters.defs.SynthDef`, `clausters.defs.FaustDef` or
  `clausters.defs.GraphDef` — is **rendered offline** (an ephemeral NRT
  session: the def is sent, instanced with ``controls``, freed at ``dur``) and
  its output plotted, every channel in its own lane. The way to eyeball what a
  def actually produces without a server or an audio device.
- a bare **expression** — a `clausters.defs.Ugen` graph, a Faust
  `clausters.defs.Signal` or `clausters.defs.Box` — takes the same offline
  path through the ephemeral-def coercion `play` uses
  (`clausters.defs.asdef.as_def`), so ``plot(sine(440) * 0.5)`` shows the
  signal directly. It plots as wide as it writes: one lane, unless a `Box`
  brings its own arity or ``channels`` says otherwise.
- an `clausters.defs.Env` is rendered through the server's own ``EnvGen`` (a
  one-node NRT render, gate-released at its sustain point when it has one), so
  the drawn curve is exactly what the engine plays — not a client-side
  re-evaluation. An `clausters.seq.automation.Automation` plots the same way —
  its curve is an `Env` — labelled with the automation's control name.
- a `clausters.defs.Buffer` (or a buffer number) is fetched from the ambient
  **live** server (`clausters.base.main.Main.resolve_server`) with its shape
  and sample rate, and plotted — the way to check a buffer's contents.
- any other **iterable of numbers** — a list, a stdlib ``array``, a
  `clausters.seq.pattern.Pattern` (``Pseq``, ``Pwhite``, …) or any stream — is
  materialized (up to ``n`` items for the endless ones) and plotted as a
  sequence: index counts on the x axis and the value axis **auto-fitted** to
  the data, whatever its range. A list of per-channel lists plots multichannel.

``view="spectrum"`` plots the averaged magnitude spectrum instead (dB against
frequency on ``freq_scale`` — log/linear/mel/bark), analyzed host-side with the
same shared-core FFT the spectrogram uses. Either way the window is static
(no zoom, pan or editing) but measured: x/y rulers fit the data and hovering
reads out the exact sample or bin under the cursor.

```python
from clausters import plot
from clausters.seq import Pseq, Pwhite

plot(my_synthdef, dur=2.0)                  # a def's rendered output
plot(Env.adsr(), label="adsr")              # an envelope, engine-evaluated
plot(Pwhite(40.0, 4700.0), n=200)           # a sequence, range auto-fitted
plot(my_graphdef, view="spectrum")          # its averaged spectrum
```
"""

import atexit
import itertools
import os
import tempfile

__all__ = ["plot", "PlotWindow"]

#: Inline `data` ceiling: at most this many floats ride the GuiDef JSON; more
#: go through a temp raw-f32 file the host maps (the bulk path).
_INLINE_MAX = 2048

#: The module-level GUI host the ambient verbs (`plot`, `clausters.scope`)
#: boot lazily when no session brought one, and the server its client leg
#: points at (``None`` for the leg-less host `plot` alone needs).
_own_host = None
_own_host_server = None
#: Temp files behind open plots, removed at interpreter exit.
_tmp_files: list[str] = []


class PlotWindow:
    """One open plot window: its GUI ``host``, the window ``id`` and the plot
    widget's id, so the display stays adjustable after the fact::

        win = plot(seq)
        win.set(view="spectrum", freq_scale="mel")   # /gui_set, live
        win.close()
    """

    def __init__(self, host, window_id: int, widget_id: int):
        self.host = host
        self.id = window_id
        self.widget_id = widget_id

    def set(self, **props):
        """Live-set plot props (``view``, ``min``/``max`` — a number, or
        ``"auto"`` to refit — ``freq_scale``, ``db_floor``/``db_ceil``,
        ``ruler``/``ruler_y``, ``label``…) via ``/gui_set``."""
        self.host.set(self.widget_id, **props)
        return self

    def close(self):
        """Close the window (``/gui_free``)."""
        self.host.close(self.id)

    def __repr__(self):
        return f"PlotWindow(id={self.id})"


def plot(obj, *, dur: float = 1.0, controls=None, defs=(), n: int = 1024,
         sample_rate: float = 48_000.0, channels: int | None = None,
         view: str | None = None, overlay: bool = False,
         min: float | None = None, max: float | None = None,
         freq_scale: str | None = None, fft_size: int | None = None,
         db_floor: float | None = None, db_ceil: float | None = None,
         ruler: str | None = None, ruler_y: str | None = None,
         label: str | None = None, title: str | None = None,
         w: int = 760, h: int | None = None, host=None) -> PlotWindow:
    """Plot ``obj`` in its own window on the ambient GUI host.

    Args:
        obj: what to plot — a def (`SynthDef`/`FaustDef`/`GraphDef`, rendered
            offline) or a bare expression (`Ugen`/`Signal`/`Box`, coerced to
            an ephemeral def first), an `Env` or `Automation` (rendered
            through ``EnvGen``), a `Buffer` or buffer number (fetched from
            the ambient live server), or an iterable of numbers / of
            per-channel number lists (materialized).
        dur: seconds a def is held before it is freed — the rendered length.
        controls: ``{name: value}`` controls (ports, for a `GraphDef`) the
            instance is started with.
        defs: extra defs the render needs first — a `GraphDef`'s **member
            defs** (the ephemeral offline session starts empty, so they must
            ride along), or any def ``obj``'s graph references.
        n: materialization cap for endless sequences (`Pwhite` and friends).
        sample_rate: the offline render's rate; also places a fetched buffer's
            time axis when the server reports none.
        channels: output channel count of a def render (default 2). Ignored
            for the other kinds (a buffer brings its own; sequences infer it).
        view: ``"signal"`` (default) or ``"spectrum"``.
        overlay: draw channels as overlaid color traces instead of lanes.
        min: value-axis sides of the signal view; ``None`` auto-fits that side
            to the data.
        max: see ``min``.
        freq_scale: spectrum frequency axis — ``"log"`` (default),
            ``"linear"``, ``"mel"``, ``"bark"``.
        fft_size: spectrum analysis size (a power of two, default 2048).
        db_floor: spectrum dB window (default ``-100`` / ``0``).
        db_ceil: see ``db_floor``.
        ruler: the signal view's time (x) unit — ``"time"`` (clock seconds,
            the default when the data has a sample rate), ``"samples"``
            (plain sample counts, the default for rate-less sequences) or
            ``"off"`` to hide the strip. Live-switchable later via
            ``win.set(ruler=...)``.
        ruler_y: ``"off"`` hides the value-axis strip (shown by default).
        label: the plot's label strip (defaults to something sensible per
            kind — the def's name, ``expr``, ``buffer <n>``, ``env``, an
            automation's control name, ``sequence``).
        title: the window title (defaults to the label).
        w: window width in px.
        h: window height (default sized to the channel count).
        host: an explicit `clausters.gui.GuiHost`; ``None`` resolves the
            ambient one — the current (or default) session's `Session.gui`
            host if one is up, else a host `plot` boots and owns.

    Returns:
        A `PlotWindow` — ``.set(...)`` retunes the display live (e.g.
        ``view="spectrum"``), ``.close()`` closes it.
    """
    samples, chans, rate, kind_label = _materialize(
        obj, dur=dur, controls=controls, defs=defs, n=n,
        sample_rate=sample_rate, channels=channels)
    label = label if label is not None else kind_label
    host = host if host is not None else _ambient_host()
    from .gui import guidef

    # Widget ids live in the host's one global namespace (all windows, all
    # scripts on it), so each plot's widget takes a fresh unique id — a
    # repeated id would be skipped at define time and /gui_set would hit
    # whichever widget registered it first.
    widget_id = host.alloc_id()
    props: dict = {"channels": chans, "view": view, "overlay": overlay or None,
                   "min": min, "max": max, "freq_scale": freq_scale,
                   "fft_size": fft_size, "db_floor": db_floor,
                   "db_ceil": db_ceil, "ruler": ruler, "ruler_y": ruler_y,
                   "label": label}
    if rate > 0.0:
        props["sample_rate"] = float(rate)
    elif ruler is None:
        # No rate: clock time is meaningless, so the x axis reads in counts.
        props["ruler"] = "samples"
    props = {k: v for k, v in props.items() if v is not None}
    if len(samples) <= _INLINE_MAX:
        widget = guidef.plot(widget_id, data=[float(x) for x in samples], **props)
    else:
        fd, path = tempfile.mkstemp(prefix="clausters_plot_", suffix=".f32")
        os.close(fd)
        guidef.samples_to_file(samples, path)
        _tmp_files.append(path)
        widget = guidef.plot(widget_id, path=path, **props)
    if h is None:
        h = 260 if chans <= 1 else 160 + 140 * chans
    tree = guidef.window(widget, title=title or label or "plot", w=w, h=h)
    window_id = host.open(tree)
    return PlotWindow(host, window_id, widget_id)


# ---- dispatch: turning the object into interleaved samples ----

def _materialize(obj, *, dur, controls, defs, n, sample_rate, channels):
    """Resolves ``obj`` to ``(samples, channels, sample_rate, label)`` —
    interleaved floats; ``sample_rate`` 0 marks an index (sequence) axis."""
    from .defs.asdef import as_def
    from .defs.boxes import Box
    from .defs.buffer import Buffer
    from .defs.faustdef import FaustDef
    from .defs.graphdef import GraphDef
    from .defs.signals import Signal
    from .defs.synthdef import SynthDef
    from .defs.ugens import Env, Ugen
    from .seq.automation import Automation

    if isinstance(obj, Env):
        return _render_env(obj, sample_rate)
    if isinstance(obj, Automation):
        samples, chans, rate, _ = _render_env(obj.env, sample_rate)
        return samples, chans, rate, obj.name
    if isinstance(obj, (Ugen, Signal, Box)):
        # A bare expression: the same ephemeral-def coercion play uses. It is
        # as wide as it writes — one channel unless asked otherwise (a Box
        # brings its own arity).
        if channels is None:
            channels = (obj.num_outputs or 2) if isinstance(obj, Box) else 1
        samples = _render_def(as_def(obj), dur, controls, defs, sample_rate,
                              channels)
        return samples, channels, sample_rate, "expr"
    if isinstance(obj, (SynthDef, FaustDef, GraphDef)):
        chans = channels if channels is not None else 2
        samples = _render_def(obj, dur, controls, defs, sample_rate, chans)
        return samples, chans, sample_rate, obj.name
    if isinstance(obj, Buffer):
        return _fetch_buffer(obj.bufnum, sample_rate)
    return _sequence(obj, n)


def _render_def(obj, dur, controls, defs, sample_rate, channels):
    """A def's offline samples — `clausters.render.bounce_def`, the shared
    change of state (`render` delivers it, `plot` draws it)."""
    from .render import bounce_def

    return bounce_def(obj, dur, controls, defs, sample_rate, channels)


def _render_env(env, sample_rate):
    """Renders an `Env` through the engine's own ``EnvGen`` — what you plot is
    what an ``EnvGen`` plays. A sustained envelope (``release_node``) has its
    gate closed at the sustain point, so the release segments show too."""
    from .defs import SynthDef, control, env_gen, out
    from .session import Session

    times = [float(t) for t in env.times]
    total = sum(times) or 1.0
    session = Session.nrt(tempo=1.0)
    server = session.server
    gate = control("gate", 1.0)
    sdef = SynthDef("_plot_env", out(0.0, env_gen(env, gate=gate)))
    server.add_synthdef(sdef)
    node = server.synth("_plot_env")
    release_node = getattr(env, "release_node", None)
    if release_node is not None:
        sustain_at = sum(times[: int(release_node)])
        server.send_bundle_after(sustain_at, ("/n_set", node.id, "gate", 0.0))
    server.send_bundle_after(total, ("/n_free", node.id))
    samples, _frames = session.render(sample_rate=sample_rate, channels=1)
    return samples, 1, sample_rate, "env"


def _fetch_buffer(bufnum, fallback_rate):
    """Fetches a buffer's interleaved samples and shape from the ambient live
    server (the running/default session's) — the buffer-contents check."""
    from .base.main import main

    server = main.resolve_server(None)
    info = server.query_buffer(bufnum)
    samples = server.get_samples(bufnum)
    rate = info.sample_rate if info.sample_rate > 0.0 else fallback_rate
    return samples, max(1, info.channels), rate, f"buffer {bufnum}"


def _sequence(obj, n):
    """Materializes an iterable of numbers (or of per-channel number lists,
    interleaved) — up to ``n`` items for the endless ones. The rate is 0: the
    x axis reads in index counts and the value range auto-fits."""
    if isinstance(obj, (list, tuple)) and obj and _is_sequence(obj[0]):
        chans = [[float(x) for x in itertools.islice(iter(ch), n)] for ch in obj]
        frames = min(len(c) for c in chans)
        interleaved = [c[f] for f in range(frames) for c in chans]
        return interleaved, len(chans), 0.0, "sequence"
    values = [float(x) for x in itertools.islice(iter(obj), n)]
    return values, 1, 0.0, "sequence"


def _is_sequence(x) -> bool:
    """A per-channel row: iterable but not a plain number."""
    if isinstance(x, (int, float)):
        return False
    try:
        iter(x)
        return True
    except TypeError:
        return False


# ---- the ambient GUI host ----

def _ambient_host(server=None):
    """The GUI host the ambient visual verbs open windows on: the current
    (else default) session's `gui` host when one is already up, else a
    standalone host booted once and owned by this module.

    ``server`` is the audio server the caller needs the host to be a client
    of — `clausters.scope` passes the resolved live server so the owned host
    boots with its address and shared-memory segment (the tap/bus read path);
    `plot` passes nothing (plot data rides mapped files, no client leg). An
    owned host booted leg-less is **rebooted** wired when a leg is first
    needed — any windows still open on it close (a session's host never is:
    `Session.gui` wires the leg from the start)."""
    global _own_host, _own_host_server
    from .base.main import main, default_session

    for session in (main.current_session, default_session):
        gui = getattr(session, "_gui", None)
        if gui is not None:
            return gui
    if _own_host is not None and (server is None or server is _own_host_server):
        return _own_host
    from .gui import GuiHost

    if _own_host is not None:
        # The owned host lacks the client leg this call needs (or points at
        # another server): replace it with one wired to `server`.
        _own_host.stop()
        _own_host = None
    if server is None:
        _own_host = GuiHost.boot(server=None)
    else:
        addr = f"{server.target.host}:{server.target.port}"
        _own_host = GuiHost.boot(server=addr, shm=server.shm)
    _own_host_server = server
    return _own_host


@atexit.register
def _cleanup():
    for path in _tmp_files:
        try:
            os.remove(path)
        except OSError:
            pass
