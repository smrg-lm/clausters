"""``scope`` — watch live audio buses in a window. A brief manual.

**What it is.** The real-time sibling of `clausters.plot`: one call opens a
window that follows ``channels`` consecutive audio buses of the running
server, frame by frame, with no per-frame messages (the GUI host reads the
server's shared memory). Everything is wired for you: the ambient server and
GUI host are resolved, and the GUI host asks the server to record the buses
it draws — you name a bus, nothing else.

**Open one:**

```python
from clausters import Server, scope

server = Server.boot()
# ... play something ...
win = scope()                        # hardware out 0, oscilloscope
win = scope(0, channels=2)           # outs 0/1, one lane per channel
win = scope(bus)                     # a Bus monitors all its channels
win = scope(0, view="phase")         # stereo field of outs 0/1
win = scope(0, view="spectrum", channels=2, freq_scale="mel")
```

**The three views** (``view=``):

- ``"signal"`` — a triggered **oscilloscope**. Each channel is a lane (or a
  color-coded trace with ``overlay=True``); the x ruler reads milliseconds of
  the ``window_ms`` display window, the y ruler signal value over
  ``[min, max]``. The trace is *phase-locked*: every frame is aligned on a
  rising crossing of the ``trigger`` level (marked by a faint line) found in
  the **first** channel, so a periodic signal stands still and the channels
  keep their true relative phase. The corner read-out says ``lock`` (the
  trigger fired) or ``free`` (no crossing — silence or DC — so the window
  free-runs).
- ``"phase"`` — a **phasescope** (goniometer) of the stereo pair ``bus`` /
  ``bus + 1``: mono draws a vertical line, anti-phase horizontal, a wide
  field fills the lozenge; the bar underneath is the correlation.
- ``"spectrum"`` — a live **spectrum**: one FFT per channel per frame, one
  color-coded curve each; the x ruler reads hertz on ``freq_scale``
  (log/linear/mel/bark), the y ruler dB over ``[db_floor, db_ceil]``.

**Adjust it live** with ``win.set(...)`` (any prop of the open view — the
window, the trigger, the scale, the FFT size):

```python
win.set(window_ms=5.0)               # signal: zoom the time window
win.set(trigger=0.2, min=-0.5, max=0.5)
win.set(freq_scale="linear", fft_size=4096)   # spectrum
win.set(ruler="off", ruler_y="off")  # bare field, no axis strips
```

**Close it** with ``win.close()`` — it closes the window, and the host stops
recording whatever no open view is drawing any more (closing from the window
manager does the same).

**Requirements.** A live server with a shared-memory segment (`Server.boot`
and `Session.live` create one by default). Recording an audio bus uses one of
the server's sample rings (``--taps``, 8 by default): a stereo scope holds two
while open, so close scopes you are done with. To scope a server you only *attached* to, pass ``host=``
pointed at a `clausters.gui.GuiHost` booted with that server's segment path.
"""

__all__ = ["scope", "ScopeWindow"]

_VIEWS = ("signal", "phase", "spectrum")


class ScopeWindow:
    """One open scope window: the GUI ``host``, the window ``id`` and the
    scope widget's id. ``set`` retunes the display live; ``close`` closes the
    window, and the host stops the recording nothing is drawing any more::

        win = scope(bus, view="spectrum")
        win.set(freq_scale="mel", fft_size=4096)   # /gui_set, live
        win.close()                                # /gui_free
    """

    def __init__(self, host, window_id: int, widget_id: int, server, bus: int, count: int):
        self.host = host
        self.id = window_id
        self.widget_id = widget_id
        self.server = server
        #: first bus of the adjacent run this window watches (`count` of them).
        self.bus = bus
        self._count = count
        self._closed = False

    def set(self, **props):
        """Live-set the scope widget's props via ``/gui_set`` — per view:
        ``window_ms``/``trigger``/``hold``/``min``/``max``/``overlay``
        (signal), ``window_ms``/``hold`` (phase), ``fft_size``/``freq_scale``/
        ``db_floor``/``db_ceil``/``averaging``/``peak_hold`` (spectrum);
        ``ruler``/``ruler_y`` (``"off"`` hides an axis strip) and ``label``
        on any. ``bus`` and ``channels`` retarget it."""
        self.host.set(self.widget_id, **props)
        return self

    def close(self):
        """Close the window (``/gui_free``). Idempotent. The recording behind
        it is the host's business: it stops what no open view reads."""
        if self._closed:
            return
        self._closed = True
        self.host.close(self.id)

    def __repr__(self):
        return f"ScopeWindow(id={self.id}, bus={self.bus})"


def scope(bus=0, *, view: str = "signal", channels: int | None = None,
          overlay: bool | None = None,
          window_ms: float | None = None, trigger: float | None = None,
          hold: bool | None = None, min: float | None = None,
          max: float | None = None, fft_size: int | None = None,
          db_floor: float | None = None, db_ceil: float | None = None,
          freq_scale: str | None = None, averaging: float | None = None,
          peak_hold: bool | None = None, ruler: "bool | str | None" = None,
          ruler_y: "bool | str | None" = None, label: str | None = None,
          title: str | None = None, w: int = 480, h: int | None = None,
          server=None, host=None) -> ScopeWindow:
    """Watch ``channels`` consecutive audio buses from ``bus`` in a window.

    See the module manual above for how each view reads. Signal and spectrum
    views monitor ``channels`` buses (``bus .. bus + channels - 1``); the phase
    view is the two-channel case — it always reads the pair ``bus`` /
    ``bus + 1``.

    Args:
        bus: the first audio bus to watch — a `clausters.defs.Bus` or a plain
            index (default ``0``, the first hardware output).
        view: ``"signal"`` (oscilloscope, default), ``"phase"`` (goniometer)
            or ``"spectrum"`` (live FFT curves).
        channels: how many consecutive buses to monitor. Default: a `Bus`'s
            own channel count, else ``1``; the phase view is fixed at ``2``.
        overlay: signal view — color-coded traces in one field instead of
            stacked lanes.
        window_ms: the display window — signal (default 20 ms) and phase
            (trail persistence, default 30 ms) views.
        trigger: signal view — the rising-crossing trigger level (default
            ``0.0``; searched in the first channel, marked by a faint line).
        hold: freeze the trace (signal/phase; also live via ``set``).
        min: vertical range of the signal view (default ``-1``).
        max: see ``min`` (default ``1``).
        fft_size: spectrum analysis size (a power of two, 256..4096, default
            2048); live via ``set``.
        db_floor: spectrum dB window (default ``-100`` / ``0``).
        db_ceil: see ``db_floor``.
        freq_scale: spectrum frequency axis — ``"log"`` (default),
            ``"linear"``, ``"mel"``, ``"bark"``; live via ``set``.
        averaging: spectrum per-bin exponential smoothing, 0..1 (default 0.5).
        peak_hold: spectrum — overlay a slowly decaying peak trace.
        ruler: the x axis strip (ms / Hz per view), shown by default;
            ``False`` or ``"off"`` hides it. The phase view has no rulers.
        ruler_y: the y axis strip (value / dB), likewise.
        label: the widget's label strip (defaults to the buses, per view).
        title: the window title (defaults to the label).
        w: window width in px.
        h: window height (default sized per view and channel count).
        server: an explicit `clausters.defs.Server`; ``None`` resolves the
            ambient live one (the running/current session's, else the default
            session's).
        host: an explicit `clausters.gui.GuiHost`; ``None`` resolves the
            ambient one — the session's `Session.gui` host if one is up, else
            an owned host booted (or rebooted) **wired to the server** (its
            address and ``shm`` segment, the native tap read path).

    Returns:
        A `ScopeWindow` — ``.set(...)`` retunes the display live, ``.close()``
        closes the window (and with it the recording behind it).
    """
    from .base.main import main
    from .defs.bus import Bus
    from .gui import guidef
    from .plot import _ambient_host

    if view not in _VIEWS:
        raise ValueError(f"unknown view {view!r} (one of {_VIEWS})")
    if view == "phase":
        if channels is not None and channels != 2:
            raise ValueError("view='phase' reads exactly 2 channels "
                             f"(bus and bus + 1), got channels={channels}")
        channels = 2
    elif channels is None:
        channels = bus.channels if isinstance(bus, Bus) else 1
    if channels < 1:
        raise ValueError(f"channels must be >= 1, got {channels}")
    server = main.resolve_server(server)
    if host is None:
        from .gui import ambient_host

        # A *registered* host answers this for itself. The demand below is
        # about the host this module would otherwise boot -- a native one,
        # which reads the taps out of the server's segment. A host someone
        # else registered may have another way in (the browser host streams
        # them over its own server leg, having no segment to map), and it is
        # not this module's to reason about: it cannot boot it, reconnect it
        # or point it elsewhere, which is why it was registered.
        if ambient_host() is None and server.shm is None:
            raise RuntimeError(
                "scope reads the server's audio buses from its shared-memory "
                "segment, and this server handle has none: boot with the "
                "default shm='auto' (Server.boot / Session.live), or pass "
                "host= pointed at a GUI host with its own segment (e.g. "
                "GuiHost.boot(server=..., shm=...) for a server you attached "
                "to)")
        host = _ambient_host(server)

    index = bus.index if isinstance(bus, Bus) else int(bus)

    if label is None:
        if view == "phase":
            label = f"bus {index}/{index + 1}"
        elif channels > 1:
            label = f"bus {index}-{index + channels - 1}"
        else:
            label = f"bus {index}"
    widget_id = host.alloc_id()
    if view == "signal":
        widget = guidef.scope(index, id=widget_id, channels=channels,
                              overlay=overlay, window_ms=window_ms,
                              trigger=trigger, hold=hold, min=min, max=max,
                              ruler=ruler, ruler_y=ruler_y, label=label)
        lanes = 1 if overlay else channels
        h = h if h is not None else (200 + 90 * lanes)
    elif view == "phase":
        widget = guidef.phasescope(index, id=widget_id,
                                   window_ms=window_ms, hold=hold, label=label)
        h = h if h is not None else 420
    else:
        widget = guidef.spectrum(index, id=widget_id, channels=channels,
                                 fft_size=fft_size, db_floor=db_floor,
                                 db_ceil=db_ceil, freq_scale=freq_scale,
                                 averaging=averaging, peak_hold=peak_hold,
                                 ruler=ruler, ruler_y=ruler_y, label=label)
        h = h if h is not None else 280
    tree = guidef.window(widget, title=title or label, w=w, h=h)
    window_id = host.open(tree)
    return ScopeWindow(host, window_id, widget_id, server, index, channels)
