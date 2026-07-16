"""The free-standing ``scope`` — one verb for watching a live signal.

`scope` is the real-time sibling of `clausters.plot`: where `plot` renders and
draws a *finished* signal, `scope` opens a window that follows a **live audio
bus** of the ambient server, frame by frame. One call does the whole wiring —
it resolves the ambient live server (`clausters.base.main.Main.resolve_server`)
and the ambient GUI host (the same one `plot` uses, here booted as a *client of
the server*: its address and shared-memory segment), takes a free audio tap
from the server's tap registry (``server.taps``), routes the bus into it with
``/tap``, and opens the window. ``view`` picks the instrument:

- ``"signal"`` (default) — a triggered **oscilloscope**: a ``window_ms``
  display window re-read every frame, aligned on a rising crossing of
  ``trigger`` (with hysteresis; free-running when the signal never crosses),
  so a periodic signal draws a stable trace.
- ``"phase"`` — the **phasescope** (goniometer): the stereo pair ``bus`` /
  ``bus + 1`` as the 45°-rotated Lissajous figure with a correlation read-out
  (mono reads vertical, anti-phase horizontal). Takes **two adjacent** taps.
- ``"spectrum"`` — the live **spectrum** (spectroscope): one FFT per frame of
  the newest ``fft_size`` window, in dB over a ``freq_scale`` frequency axis
  (log/linear/mel/bark), exponentially ``averaging``-smoothed, optional
  ``peak_hold``.

The returned `ScopeWindow` retunes the display live (``.set(...)`` →
``/gui_set``: the trigger, the FFT size, the frequency scale…) and ``.close()``
**releases the resources** — it stops the tap(s) (``/tap … -1``), returns them
to the registry and closes the window. Closing the window from the OS frees the
widgets but not the taps: prefer ``.close()``.

Natively the host reads the taps out of the server's shared-memory segment
(zero messages), so the resolved server must carry one (``shm``; `Server.boot`
and `Session.live` create it by default). A host over ``/tap_stream`` (the
browser path) can be passed explicitly with ``host=``.

```python
from clausters import Server, scope

Server.boot()
# ... play something on bus 0 ...
win = scope()                        # oscilloscope on hardware out 0
win.set(window_ms=5.0)               # tighter window, live
scope(0, view="phase")               # stereo field of outs 0/1
scope(0, view="spectrum", freq_scale="mel")
win.close()                          # stops the tap and frees it
```
"""

__all__ = ["scope", "ScopeWindow"]

_VIEWS = ("signal", "phase", "spectrum")


class ScopeWindow:
    """One open scope window and the server resources behind it: the GUI
    ``host``, the window ``id``, the scope widget's id, and the audio ``tap``
    run it owns on ``server``. ``set`` retunes the display live; ``close``
    stops the tap(s), returns them to the registry and closes the window::

        win = scope(bus, view="spectrum")
        win.set(freq_scale="mel", fft_size=4096)   # /gui_set, live
        win.close()                                # /tap -1 + registry free
    """

    def __init__(self, host, window_id: int, widget_id: int, server,
                 tap: int, count: int):
        self.host = host
        self.id = window_id
        self.widget_id = widget_id
        self.server = server
        #: first tap index of the run this window owns (`count` adjacent).
        self.tap = tap
        self._count = count
        self._closed = False

    def set(self, **props):
        """Live-set the scope widget's props via ``/gui_set`` — per view:
        ``window_ms``/``trigger``/``hold``/``min``/``max`` (signal),
        ``window_ms``/``hold`` (phase), ``fft_size``/``freq_scale``/
        ``db_floor``/``db_ceil``/``averaging``/``peak_hold`` (spectrum)."""
        self.host.set(self.widget_id, **props)
        return self

    def close(self):
        """Stop the tap(s) (``/tap … -1``), return them to ``server.taps`` and
        close the window (``/gui_free``). Idempotent."""
        if self._closed:
            return
        self._closed = True
        for k in range(self._count):
            self.server.tap(self.tap + k, -1)
        self.server.taps.free(self.tap, self._count)
        self.host.close(self.id)

    def __repr__(self):
        return f"ScopeWindow(id={self.id}, tap={self.tap})"


def scope(bus=0, *, view: str = "signal",
          window_ms: float | None = None, trigger: float | None = None,
          hold: bool | None = None, min: float | None = None,
          max: float | None = None, fft_size: int | None = None,
          db_floor: float | None = None, db_ceil: float | None = None,
          freq_scale: str | None = None, averaging: float | None = None,
          peak_hold: bool | None = None, label: str | None = None,
          title: str | None = None, w: int = 480, h: int | None = None,
          server=None, host=None) -> ScopeWindow:
    """Watch audio ``bus`` of the ambient live server in its own window.

    Args:
        bus: the audio bus to watch — a `clausters.defs.Bus` or a plain index
            (default ``0``, the first hardware output). ``view="phase"`` reads
            the stereo pair ``bus`` and ``bus + 1``.
        view: ``"signal"`` (oscilloscope, default), ``"phase"`` (goniometer)
            or ``"spectrum"`` (live FFT curve).
        window_ms: the display window — signal (default 20 ms) and phase
            (trail persistence, default 30 ms) views.
        trigger: signal view — the rising-crossing trigger level (default
            ``0.0``, with hysteresis; free-running when never crossed).
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
        label: the widget's label strip (defaults to the bus, per view).
        title: the window title (defaults to the label).
        w: window width in px.
        h: window height (default sized per view).
        server: an explicit `clausters.defs.Server`; ``None`` resolves the
            ambient live one (the running/current session's, else the default
            session's).
        host: an explicit `clausters.gui.GuiHost`; ``None`` resolves the
            ambient one — the session's `Session.gui` host if one is up, else
            an owned host booted (or rebooted) **wired to the server** (its
            address and ``shm`` segment, the native tap read path).

    Returns:
        A `ScopeWindow` — ``.set(...)`` retunes the display live, ``.close()``
        stops and frees the tap(s) and closes the window.
    """
    from .base.main import main
    from .defs.bus import Bus
    from .gui import guidef
    from .plot import _ambient_host

    if view not in _VIEWS:
        raise ValueError(f"unknown view {view!r} (one of {_VIEWS})")
    server = main.resolve_server(server)
    if host is None:
        if server.shm is None:
            raise RuntimeError(
                "scope reads the server's audio taps from its shared-memory "
                "segment, and this server handle has none: boot with the "
                "default shm='auto' (Server.boot / Session.live), or pass "
                "host= pointed at a GUI host with its own tap path (e.g. "
                "GuiHost.boot(server=..., shm=...) for a server you attached "
                "to)")
        host = _ambient_host(server)

    index = bus.index if isinstance(bus, Bus) else int(bus)
    count = 2 if view == "phase" else 1
    tap0 = server.taps.alloc(count)
    for k in range(count):
        server.tap(tap0 + k, index + k)

    label = label if label is not None else (
        f"bus {index}/{index + 1}" if view == "phase" else f"bus {index}")
    widget_id = host.alloc_id()
    if view == "signal":
        widget = guidef.scope(widget_id, tap=tap0, window_ms=window_ms,
                              trigger=trigger, hold=hold, min=min, max=max,
                              label=label)
        h = h if h is not None else 240
    elif view == "phase":
        widget = guidef.phasescope(widget_id, tap0, tap0 + 1,
                                   window_ms=window_ms, hold=hold, label=label)
        h = h if h is not None else 420
    else:
        widget = guidef.spectrum(widget_id, tap0, fft_size=fft_size,
                                 db_floor=db_floor, db_ceil=db_ceil,
                                 freq_scale=freq_scale, averaging=averaging,
                                 peak_hold=peak_hold, label=label)
        h = h if h is not None else 260
    tree = guidef.window(widget, title=title or label, w=w, h=h)
    try:
        window_id = host.open(tree)
    except Exception:
        for k in range(count):
            server.tap(tap0 + k, -1)
        server.taps.free(tap0, count)
        raise
    return ScopeWindow(host, window_id, widget_id, server, tap0, count)
