"""Session: an explicit, isolated environment (server + clocks).

A `Session` is the unit of isolation: it bundles a `Server` and a `TempoClock`
into one handle with ``play`` / ``render`` / ``run``, and the ``nrt`` / ``live``
/ ``embed`` factories pick sensible defaults. Because each session owns its own
state, **several coexist** — e.g. one offline NRT session for plotting next to a
live RT one — in the same script without touching each other.

The counterpart is the **default session**, `clausters.default_session` (the
`clausters.base.main.Main` singleton, also reachable as ``main``): the ambient
environment used whenever no session was named. Booting a server free-standing
(``Server().boot()``) adopts it there, so ``Event().play()`` and `clausters.play`
work with no `Session` at all. An explicit `Session` is simply a *named*
environment that never touches the default one.

```python
s = Session.nrt(tempo=2.0)
s.play(Pbind(instrument="default", freq=Pseq([440, 550, 660]), dur=0.5))
stats = s.render()                  # drains the clock, renders the score
```
"""

from contextlib import contextmanager

from .base import OscDestination, OscEmbedInterface, OscNrtInterface, TempoClock
from .base.environment import Environment
from .base.main import main
from .defs import Server


class Session(Environment):
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

    It is an `clausters.base.environment.Environment` — the same base the default
    session (`clausters.default_session`) extends — so a named session and the
    default one are the same kind of thing. That makes it its **own random
    context** (`seed` / ``rng``): ``session.seed(n)`` reproduces *this* session's
    events without touching another's, so two sessions -- even both offline --
    stay reproducible independently. Anything created while the session drives
    (``play`` / ``render``) or inside a ``with`` block draws from this root.

    Args:
        server: the `Server` to drive -- a live one, or one holding an
            `OscNrtInterface` for offline rendering.
        clock: the `TempoClock` that sequences it; a fresh one at tempo 1.0 is
            created when omitted.
        gui: a `clausters.gui.GuiHost` this session drives instead of booting
            one -- the visual half of taking a `Server` the session did not
            start, and the way a session adopts a host reached with
            `clausters.gui.GuiHost.attach`. `gui` then returns it rather than
            launching anything.

    Closing the session closes its server, so the common shape is a context
    manager:

    ```python
    with Session.live(tempo=2.0, latency=0.1) as s:
        s.play(Pbind(instrument="default", degree=Pseq([0, 2, 4]), dur=0.5))
        s.run(3.0)
    ```
    """

    def __init__(self, server: Server, clock: TempoClock | None = None, gui=None):
        super().__init__()          # Environment: its own RNG root + server slot
        self.server = server
        self.clock = clock if clock is not None else TempoClock()
        #: back-reference so a play running on this clock resolves *this*
        #: session's server/rng (``current_tt.clock.session``), keeping several
        #: sessions isolated from each other and from the default session.
        self.clock.session = self
        #: the session's GUI host: the one handed to the constructor, or the one
        #: `gui` boots lazily. Stopped with the session — and `GuiHost.stop`
        #: stops the ``clausters-gui`` process only if that host booted it, so a
        #: host reached with `clausters.gui.GuiHost.attach` is left standing.
        #: The server works the same way (see `Server.boot` / `Server.attach`),
        #: stopped via ``server.close``.
        self._gui = gui
        #: external OSC destinations opened by `destination`, closed with the
        #: session.
        self._destinations = []

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
             verbose: int = 0, workers: "int | None" = None,
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
            timebase: the clock's pacing source. Left unset, the session
                **anchors to the server's sample clock by default** (config
                ``[client].clock``, default ``"sample"``) — sample-accurate and
                drift-free, falling back to wall-clock if no master answers. Pass
                ``timebase=MonotonicTimebase()`` (or set ``[client].clock =
                "monotonic"``) to keep wall-clock OSC timetags.
            boot: start a server if none is already answering (default). ``False``
                attaches only, never launching a process.
            options: a `clausters.defs.ServerOptions` — the enumeration of
                **every** option a launched server takes (sizing *and*
                behavior — transports, MIDI, persistence, workers, ...) —
                sizing this client's allocators alike; ``None`` uses the
                defaults.
            shm: the shared-memory segment for a launched server — ``"auto"``
                picks one, a path forces it, ``None`` launches without one. The
                path is remembered so `gui` maps the same segment.
            transport: the command carrier — ``"tcp"`` (default), ``"udp"`` or
                ``"ws"``; ``None`` takes ``[client].transport`` from the config.
            verbose: launched-server log verbosity (``1``/``2``/``3`` -> ``-v``/
                ``-vv``/``-vvv``; negative -> ``-q``).
            workers: shortcut for ``options.workers`` (a launched server's DSP
                worker threads for parallel groups); it wins over a value set
                there. Like every launch option, it only affects a server this
                call boots — an attach never reconfigures a running server.
            data_dir: a launched server's ``--data-dir``; ``None`` uses default.
            server_args: raw CLI tokens appended **last** (they win over
                everything above) — an escape hatch for flags newer than this
                client; prefer `clausters.defs.ServerOptions` fields.
            ready_timeout: seconds to wait for a launched server to answer.

        Returns:
            A `Session` you drive with `run` (or `start` / `stop`).
        """
        from .launch import server_is_up

        server = Server(host, port, latency=latency, options=options, transport=transport)
        if boot and not server_is_up(server.target.host, server.target.port):
            # The handle is already the right one; booting re-points it at the
            # address the launcher picks. An explicit session is not the default.
            server.boot(shm=shm, verbose=verbose, workers=workers,
                        data_dir=data_dir, server_args=server_args,
                        ready_timeout=ready_timeout, adopt_default=False)
        return cls(server, TempoClock(tempo, timebase=timebase))._apply_default_clock(timebase)

    def _apply_default_clock(self, timebase):
        """Anchor a live session's clock to its server's sample clock by default.

        With no explicit ``timebase``, the clock follows the config's
        ``[client].clock`` (default ``"sample"``): a local real-time session is
        sample-accurate and drift-free out of the box. Graceful — if no master
        answers, `lock_to` leaves it on wall-clock time (see `TempoClock.lock_to`).
        An explicit ``timebase`` is honoured as-is (no auto-lock). Returns ``self``.

        Both real-time factories call this: `live` locks through the UDP
        sample-clock tracker, `embed` through a direct in-process read of the
        shared counter (no socket, no timeout). Offline (`nrt`) never does —
        a score server has no live clock.
        """
        if timebase is not None:
            return self
        from .config import client_config

        if client_config().get("clock", "sample") == "sample":
            self.lock_to_server()
        return self

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
            timebase: the clock's pacing source. Left unset, the session
                **anchors to the server's sample clock by default**, exactly
                like `live` (config ``[client].clock``, default ``"sample"``) —
                and in-process the lock is a direct read of the shared counter,
                with no tracker, socket or timeout at all. Pass
                ``timebase=MonotonicTimebase()`` (or set ``[client].clock =
                "monotonic"``) to keep wall-clock OSC timetags.
            server: an existing `clausters.ipc.Clausters` handle to reuse; when
                omitted the session opens and owns a fresh embedded server and
                closes it on `close`.

        Returns:
            A `Session` you drive with `run` (or `start` / `stop`), exactly like
            `live`.
        """
        iface = OscEmbedInterface(server, workers=workers)
        session = cls(Server(interface=iface, latency=latency), TempoClock(tempo, timebase=timebase))
        return session._apply_default_clock(timebase)

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
        other options of the first call stand). A session **given** a host (the
        constructor's ``gui``) is already settled: this returns that host and
        launches nothing, so the arguments below are ignored -- the same way
        `Server` is taken rather than booted when the constructor is used.

        The host booted here also becomes the **ambient** one if none is
        registered yet (`clausters.gui.GuiHost.boot`'s ``adopt_ambient``), so
        ``view.open()``, `clausters.plot` and `clausters.scope` land on this
        session's host instead of booting a second one. First-wins, as the
        default session adopts the first free-standing ``Server.boot()``; a host
        registered by hand keeps its place.

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
            A started `clausters.gui.GuiHost`. Open a view on it with
            `clausters.gui.guidef.View.open`, edit it with ``set`` and close it
            with `clausters.gui.GuiHost.close`.
        """
        if self._gui is not None:
            return self._gui
        from .gui import GuiHost

        server_addr = f"{self.server.target.host}:{self.server.target.port}"
        from .gui.host import DEFAULT_PORT

        # The session's share governs both legs: a session that is one of two
        # clients on a server is one of two on its host as well.
        self._gui = GuiHost(
            port=DEFAULT_PORT if port is None else port, transport=transport,
            share=self.server.share,
        ).boot(
            server=server_addr, shm=self.server.shm, verbose=verbose,
            data_dir=data_dir, extra_args=extra_args, ready_timeout=ready_timeout,
        )
        return self._gui

    @property
    def gui_host(self):
        """The GUI host this session drives, or ``None`` when it has none yet.

        The read-only half of `gui`: it says what the session already has
        without opening anything, which is what a caller that must not launch a
        process asks. `gui` is the verb that builds one.
        """
        return self._gui

    def activate(self):
        """Make this the ambient session on the calling thread, and leave it
        there; returns ``self``.

        The unscoped form of ``with session:``. A block is the right shape when
        the session's life is the block's, and the wrong one for an environment
        that outlives every statement that uses it — a REPL, a driver whose
        cells each run on their own — where there is no block to be inside of.
        After this, anything created with no session named (`clausters.play`, a
        bare `clausters.Synth`) resolves to *this* session's server, clock and
        random root.

        Thread-local, like the ambient session itself: another thread is
        unaffected, and so is a ``with`` block on this one, which saves and
        restores whatever was in force around it.
        """
        main.current_session = self
        return self

    def deactivate(self):
        """Give up being the ambient session on the calling thread; returns
        ``self``.

        The counterpart of `activate`, and a no-op when some *other* session is
        ambient — giving up a slot one does not hold would silently unseat the
        session that does.
        """
        if main.current_session is self:
            main.current_session = None
        return self

    # ---- driving ----

    @contextmanager
    def _active(self):
        """Mark this session active on the calling thread for the duration of a
        driving call, so anything created in it (a played routine, a top-level
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

    def render(self, sample_rate: float = 48_000.0, channels: int = 2,
               until: float | None = None, workers: int = 0, path=None,
               seed: int | None = None, sample_format: str = "float"):
        """Drain the clock and render the accumulated score (offline only).

        Advances the clock logically with no real-time waiting, so every
        scheduled event lands in the score, then renders that score through the
        embedded renderer.

        Args:
            sample_rate: render sample rate, in Hz.
            channels: number of interleaved output channels.
            until: stop draining the clock at this beat (see
                `TempoClock.render`); ``None`` drains everything scheduled —
                required for an endless source (an infinite pattern never
                drains on its own).
            workers: DSP worker threads for the score's parallel groups
                (``0`` renders sequentially). Bit-identical either way — the
                workers only change how long the render takes.
            path: where the audio goes. Without it the samples come back in
                ``stats.samples``; with it the **server** writes the file and
                ``stats.samples`` is ``None``. See `Server.render`.
            seed: starting seed for the render's stochastic UGens. ``None``
                draws a fresh one, so a score with noise in it is a new take
                every time; ``stats.seed`` reports the one used, and passing
                it back replays that take.
            sample_format: ``"float"``, ``"int24"`` or ``"int16"`` — only
                meaningful with ``path``, since only the file has a format.

        Returns:
            A `clausters.render.RenderStats`: ``frames``, ``channels``,
            ``sample_rate``, ``events``, per-channel ``peak`` and ``rms``,
            ``seed``, ``path``, and ``samples`` when the render kept them.
            Schedule a closing event (e.g. freeing the root group) so the
            render has a defined duration.
        """
        with self._active():
            self.clock.render(until)
        return self.server.render(sample_rate=sample_rate, channels=channels,
                                  workers=workers, path=path, seed=seed,
                                  sample_format=sample_format)

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
        call. A restart **resumes** at the beat `stop` left the clock on."""
        self.clock.start()
        return self

    def stop(self):
        """Stop the clock; returns ``self``. Nothing further fires while it is
        stopped, but the schedule is kept and the beat is held: this is a
        transport, and a later `start` picks the music up where it was
        (``session.clock.clear()`` is what drops what is queued)."""
        self.clock.stop()
        return self

    def destination(self, host: str = "127.0.0.1", port: int = 57120) -> OscDestination:
        """An external OSC application as a destination, living as long as this
        session (`close` closes it).

        What it sends is standard OSC — a message, or a bundle timetagged at
        the ambient `Moment`, so a sequence sent to another application keeps
        the same logical timing as one sent to the server. What it does not
        send is anything of ours: no `Server.latency`, no sample-accurate
        ``/sched_at``, no offline score."""
        dest = OscDestination(host, port)
        self._destinations.append(dest)
        return dest

    def close(self):
        """Close the underlying `Server` and release the clock's master-clock
        tracker (from `lock_to_server`), if any. Also stops the GUI host and, if
        `live` launched a server, that process too — so nothing this session
        started is left running. What it did **not** start it leaves standing:
        `clausters.gui.GuiHost.stop` ends a ``clausters-gui`` process only when
        that host booted it, so an attached host keeps its windows. Done automatically when the session is used as a context manager
        and, for launched processes, on interpreter exit."""
        self.deactivate()      # a closed session is nobody's ambient one
        if self._gui is not None:
            self._gui.stop()   # its process too, if that host booted one
                               # (and gives up the ambient registration)
            self._gui = None
        for dest in self._destinations:
            dest.close()
        self._destinations.clear()
        self.clock.close()
        self.server.close()    # stops a launched server process too

    def __enter__(self):
        # Activate for the whole block, so anything created inside (patterns,
        # routines, top-level draws) resolves to this session's server/clock/rng.
        self._prev_session = main.current_session
        main.current_session = self
        return self

    def __exit__(self, *exc):
        main.current_session = getattr(self, "_prev_session", None)
        self.close()
