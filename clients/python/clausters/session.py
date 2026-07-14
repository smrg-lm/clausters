"""Session: an explicit, isolated environment (server + clocks).

A `Session` is the unit of isolation: it bundles a `Server` and a `TempoClock`
into one handle with ``play`` / ``render`` / ``run``, and the ``nrt`` / ``live``
/ ``embed`` factories pick sensible defaults. Because each session owns its own
state, **several coexist** — e.g. one offline NRT session for plotting next to a
live RT one — in the same script without touching each other.

The counterpart is the **default session**, `clausters.default_session` (the
`clausters.base.main.Main` singleton, also reachable as ``main``): the ambient
environment used whenever no session was named. Booting a server free-standing
(``Server.boot()``) adopts it there, so ``Event().play()`` and `clausters.play`
work with no `Session` at all. An explicit `Session` is simply a *named*
environment that never touches the default one.

```python
s = Session.nrt(tempo=2.0)
s.play(Pbind(instrument="default", freq=Pseq([440, 550, 660]), dur=0.5))
samples, frames = s.render()        # drains the clock, renders the score
```
"""

from contextlib import contextmanager

from .base import OscEmbedInterface, OscNrtInterface, TempoClock
from .base.main import RandomContext, main
from .defs import Server


class Session(RandomContext):
    """One `Server` plus one `TempoClock`, bundled into a single handle.

    This is the client's ergonomic entry point. Rather than wiring a server, a
    clock and (optionally) a timebase together yourself, you take a `Session`
    that owns both and drives them as a unit -- `play` a pattern on it,
    `render` it offline, or `run` it for some seconds live.

    Prefer the factories to the constructor: `nrt` builds an offline,
    score-accumulating session and `live` a real-time one over UDP, each with
    sensible defaults. The constructor is for the uncommon case of supplying
    your own `Server` and clock.

    Which factory you call is the *only* thing that differs between an offline
    render and a live take: that difference lives in the `Server`'s
    communication interface, not in the pattern or the clock. So the same
    `play` drives either kind, and an offline and a live session can run side
    by side in one script.

    A session is also its **own random context** (`seed` / ``rng``, inherited
    from `clausters.base.main.RandomContext`): ``session.seed(n)`` reproduces
    *this* session's material without touching another's, so two sessions --
    even both offline -- stay reproducible independently. Material created while
    the session drives (``play`` / ``render``) or inside a ``with`` block draws
    from this root.

    Args:
        server: the `Server` to drive -- a live one, or one holding an
            `OscNrtInterface` for offline rendering.
        clock: the `TempoClock` that sequences it; a fresh one at tempo 1.0 is
            created when omitted.

    Closing the session closes its server, so the common shape is a context
    manager:

    ```python
    with Session.live(tempo=2.0, latency=0.1) as s:
        s.play(Pbind(instrument="default", degree=Pseq([0, 2, 4]), dur=0.5))
        s.run(3.0)
    ```
    """

    def __init__(self, server: Server, clock: TempoClock | None = None):
        super().__init__()          # its own RNG root (seed/rng), isolated
        self.server = server
        self.clock = clock if clock is not None else TempoClock()
        #: back-reference so a play running on this clock resolves *this*
        #: session's server/rng (``current_tt.clock.session``), keeping several
        #: sessions isolated from each other and from the default session.
        self.clock.session = self
        #: the GUI host opened lazily by `gui`, if any; it owns its own process
        #: and is stopped with the session. The server owns any process it
        #: booted (see `Server.boot` / `live`), stopped via ``server.close``.
        self._gui = None

    # ---- factories (the "defaults", explicit) ----

    @classmethod
    def nrt(cls, tempo: float = 1.0) -> "Session":
        """Build an offline (non-real-time) session.

        Its `Server` holds an `OscNrtInterface`, so playing a pattern
        accumulates a timetagged score instead of sending anything; `render`
        then turns that score into samples through the bundled embedded
        renderer. No server process and no audio device are involved.

        Args:
            tempo: the clock's tempo, in beats per second.

        Returns:
            A `Session` whose `render` produces the audio.
        """
        return cls(Server(interface=OscNrtInterface()), TempoClock(tempo))

    @classmethod
    def live(cls, host: "str | None" = None, port: "int | None" = None, *,
             tempo: float = 1.0, latency: "float | None" = None, timebase=None,
             boot: bool = True, options=None, shm="auto", transport: "str | None" = None,
             verbose: int = 0,
             data_dir=None, server_args=(), ready_timeout: float = 10.0) -> "Session":
        """Build a real-time session, **starting a server if none is up**.

        The probe (and the boot handshake) ride UDP — discovery stays
        zero-config — and the session's command interface then connects over
        **TCP by default** (``transport="udp"``/``"ws"`` opt across), so defs
        and bulk reads are not bounded by a datagram.

        This is the everyday live-coding entry point. By default (``boot=True``)
        it ensures a server the way `nrt` ensures a renderer: if one already
        answers at the target address it attaches to it, and if none does it
        **launches a separate ``clausters`` process** — choosing a shared-memory
        segment for you — and connects to that. Either way you get a session you
        drive the same. A server the session started is stopped when the session
        is closed or the interpreter exits, so a REPL or script leaves nothing
        running; a server it merely attached to is left alone.

        Pass ``boot=False`` for the plain attach-only behavior (never start a
        process): connect to a server you launched yourself, possibly remote.

        Args:
            host: the server's host; ``None`` takes the config file's
                ``[client].host`` (default ``127.0.0.1``). Booting is local.
            port: the server's UDP port; ``None`` takes ``[client].port`` (the
                Clausters default is 57110).
            tempo: the clock's tempo, in beats per second.
            latency: seconds added to each event's timetag so it reaches the
                server slightly ahead of its play time and sounds on time
                instead of late; a small value such as 0.1 is typical for a
                live take. ``None`` takes the config file's ``[client].latency``,
                falling back to 0.1 (the real-time default for a networked
                transport) when the config sets none.
            timebase: the clock's pacing source. The default (monotonic) paces
                in wall-clock seconds; a `SampleClockTimebase` anchors timing to
                the server's sample clock for drift-free scheduling.
            boot: start a server if none is already answering (default). ``False``
                attaches only, never launching a process.
            options: a `clausters.defs.ServerOptions` sizing a *launched* server
                and this client's allocators alike; ``None`` uses the defaults.
            shm: the shared-memory segment for a launched server — ``"auto"``
                picks one, a path forces it, ``None`` launches without one. The
                path is remembered so `gui` maps the same segment.
            transport: the command carrier — ``"tcp"`` (default), ``"udp"`` or
                ``"ws"``; ``None`` takes ``[client].transport`` from the config.
            verbose: launched-server log verbosity (``1``/``2``/``3`` -> ``-v``/
                ``-vv``/``-vvv``; negative -> ``-q``).
            data_dir: a launched server's ``--data-dir``; ``None`` uses default.
            server_args: extra CLI tokens for a launched server (e.g. ``["--tcp"]``).
            ready_timeout: seconds to wait for a launched server to answer.

        Returns:
            A `Session` you drive with `run` (or `start` / `stop`).
        """
        from .launch import server_is_up

        server = Server(host, port, latency=latency, options=options, transport=transport)
        if boot and not server_is_up(server.target.host, server.target.port):
            server.close()  # drop the plain interface; boot opens its own
            server = Server.boot(options=options, shm=shm, transport=transport,
                                 verbose=verbose,
                                 data_dir=data_dir, server_args=server_args,
                                 latency=latency, ready_timeout=ready_timeout,
                                 _adopt_default=False)  # an explicit session is not the default
        return cls(server, TempoClock(tempo, timebase=timebase))

    @classmethod
    def embed(cls, tempo: float = 1.0, latency: "float | None" = None, workers: int = 0,
              timebase=None, server=None) -> "Session":
        """Build a real-time session backed by an in-process embedded server.

        The whole server — audio device and engine — runs in this process
        through the bundled native library; there is no socket and no separate
        server process. Otherwise it is identical to `live`: the same routines,
        patterns and defs drive it, because only the `Server`'s communication
        interface differs (an `OscEmbedInterface` instead of UDP). So an
        embedded, a live and an offline session can run side by side in one
        script.

        Args:
            tempo: the clock's tempo, in beats per second.
            latency: seconds added to each event's timetag so it lands a touch
                in the future and sounds on time. The embedded server is
                wall-clock timetagged like a networked one, so ``None`` takes
                the config's ``[client].latency`` and then the same 0.1 real-time
                default; a smaller value such as 0.05 is fine in-process.
            workers: engine worker threads for parallel node processing (0 lets
                the server choose).
            timebase: the clock's pacing source (default monotonic wall clock).
            server: an existing `clausters.Clausters` handle to reuse; when
                omitted the session opens and owns a fresh embedded server and
                closes it on `close`.

        Returns:
            A `Session` you drive with `run` (or `start` / `stop`), exactly like
            `live`.
        """
        iface = OscEmbedInterface(server, workers=workers)
        return cls(Server(interface=iface, latency=latency), TempoClock(tempo, timebase=timebase))

    def gui(self, *, port: "int | None" = None, transport: str = "tcp",
            verbose: int = 0, data_dir=None,
            extra_args=(), ready_timeout: float = 10.0):
        """Launch (once) a ``clausters-gui`` visual server wired to this session's
        server, and return a `clausters.gui.GuiHost` connected to it.

        The GUI parallel of `live` booting a server: one call and the visual
        server is up, its client leg pointed at this session's server and — when
        that server was launched with a shared-memory segment — mapping the same
        segment, so meters, scopes and playheads read the engine with no
        per-frame messages. You never spell out an address or a segment path:
        they come from the session. The host is owned by the session and stopped
        on `close` (or interpreter exit).

        Idempotent: repeated calls return the same `GuiHost` (the ``port`` and
        other options of the first call stand).

        Args:
            port: the GUI host's own port (script -> host, UDP and TCP alike);
                ``None`` uses the host default (57210).
            transport: the carrier this session talks to the host over —
                ``"tcp"`` (default; a ``/gui_def`` tree is not bounded by a
                datagram) or ``"udp"``.
            verbose: host log verbosity (``1``/``2``/``3`` -> ``-v``/``-vv``/``-vvv``).
            data_dir: the host's ``--data-dir`` for its GuiDef store.
            extra_args: extra host CLI tokens.
            ready_timeout: seconds to wait for the host to answer.

        Returns:
            A started `clausters.gui.GuiHost`. Use `clausters.gui.GuiHost.open`
            to open a window, `set` to edit it and `clausters.gui.GuiHost.close`
            to close it.
        """
        if self._gui is not None:
            return self._gui
        from .gui import GuiHost

        server_addr = f"{self.server.target.host}:{self.server.target.port}"
        self._gui = GuiHost.boot(
            server=server_addr, shm=self.server.shm, port=port,
            transport=transport, verbose=verbose,
            data_dir=data_dir, extra_args=extra_args, ready_timeout=ready_timeout,
        )
        return self._gui

    # ---- driving ----

    @contextmanager
    def _active(self):
        """Mark this session active on the calling thread for the duration of a
        driving call, so material created in it (a played routine, a top-level
        draw) resolves to *this* session's server/clock/rng — not the default
        session's. Save/restore, so nesting and other threads are unaffected."""
        prev = main.current_session
        main.current_session = self
        try:
            yield
        finally:
            main.current_session = prev

    def play(self, pattern, quant=None):
        """Play an event pattern on this session's clock and server.

        Args:
            pattern: an event pattern, e.g. a `Pbind`.
            quant: optional quantization handed to the player -- the beat grid
                the routine starts on; ``None`` starts immediately.

        Returns:
            The `EventStreamPlayer` driving the pattern.
        """
        with self._active():
            return pattern.play(self.clock, self.server, quant)

    def render(self, sample_rate: float = 48_000.0, channels: int = 2):
        """Drain the clock and render the accumulated score (offline only).

        Advances the clock logically with no real-time waiting, so every
        scheduled event lands in the score, then renders that score through the
        embedded renderer.

        Args:
            sample_rate: render sample rate, in Hz.
            channels: number of interleaved output channels.

        Returns:
            ``(samples, frames)`` -- interleaved float32 in a stdlib
            ``array('f')`` and the frame count. Schedule a closing event (e.g.
            freeing the root group) so the render has a defined duration.
        """
        with self._active():
            self.clock.render()
        return self.server.render(sample_rate=sample_rate, channels=channels)

    def lock_to_server(self):
        """Lock this session's clock to its server's sample clock — the
        sample-accurate, drift-free timebase, with the server as the master
        clock. Returns ``self``, so it chains after a factory:
        ``Session.live(...).lock_to_server()``.

        Safe when the server is not a reachable master (offline, or no server
        running): the clock simply stays on wall-clock OSC time. See
        `TempoClock.lock_to`.
        """
        self.clock.lock_to(self.server)
        return self

    def join_transport(self):
        """Join this session's server's shared transport, so a ``quant``-ed
        pattern starts on the same beat as every other client on it (see
        `TempoClock.join_transport`). Returns ``self`` for chaining:
        ``Session.live(...).lock_to_server().join_transport()``. No-op if the
        server has no transport defined."""
        self.clock.join_transport(self.server)
        return self

    def run(self, seconds: float):
        """Run the clock in real time for ``seconds``, then stop (live only).

        Args:
            seconds: how long to advance the clock, in wall-clock seconds.

        Returns:
            ``self``, so calls chain.
        """
        self.clock.run(seconds)
        return self

    def start(self):
        """Start the clock so scheduled events fire in real time; returns
        ``self``. Pair with `stop`, or use `run` to start, wait and stop in one
        call."""
        self.clock.start()
        return self

    def stop(self):
        """Stop the clock; returns ``self``. Events scheduled past the stop
        point do not fire."""
        self.clock.stop()
        return self

    def close(self):
        """Close the underlying `Server` and release the clock's master-clock
        tracker (from `lock_to_server`), if any. Also stops the GUI host (`gui`)
        and, if `live` launched a server, that process too — so nothing is left
        running. Done automatically when the session is used as a context manager
        and, for launched processes, on interpreter exit."""
        if self._gui is not None:
            self._gui.stop()   # stops its clausters-gui process too
            self._gui = None
        self.clock.close()
        self.server.close()    # stops a launched server process too

    def __enter__(self):
        # Activate for the whole block, so material created inside (patterns,
        # routines, top-level draws) resolves to this session's server/clock/rng.
        self._prev_session = main.current_session
        main.current_session = self
        return self

    def __exit__(self, *exc):
        main.current_session = getattr(self, "_prev_session", None)
        self.close()
