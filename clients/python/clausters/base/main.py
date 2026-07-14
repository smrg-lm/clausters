"""Execution context: the default session (port of ``sc3/base/main.py``).

`main` is the process's **default session** -- the environment that holds the
ambient state used whenever you did *not* name a session explicitly. The rule is
one line:

> Everything that does not run in an explicit `clausters.Session` runs in the
> default session, `main`.

`main` is an `clausters.base.environment.Environment` — the same base a
`clausters.Session` extends — so it *is* a session, the default one. It owns what
used to be scattered "globals": the default `server` (set first-wins by a
free-standing `Server.boot()`), an opt-in `default_clock`, and the random
context (`rng`). It is exported as ``clausters.default_session`` too; ``main`` is
just its historical short name.

An **explicit** `Session` is the same kind of thing (an `Environment`) -- its own
server, its own clocks, its own state -- so several sessions (a live RT one next
to an offline NRT render, each with its own configuration) coexist in one process
without touching each other or the default session. A session can hold **several
clocks** at different tempos, including the default one.

The process-wide pieces are thread-local: `current_tt`, "which routine is
running on this thread", and `current_session`, "which explicit session is
active on this thread" (set by a `Session` while it plays/renders or as a
context manager). Neither is a swapper of global state (as sclang's global rng
was); together they say *which session you are in* -- at run time follow
``current_tt.clock.session``, and at creation time (outside any routine)
`current_session`. Both ``None`` means the default session, and resolution
falls back to it.
"""

import threading

from .environment import Environment, RandomContext

__all__ = ["Main", "main", "default_session", "Environment", "RandomContext"]


class Main(Environment):
    """The default session: the ambient `Environment` resolution falls back to.

    An `Environment` like any `clausters.Session` (server + random context), plus
    the two roles only the *default* one plays: it holds the process-wide
    thread-local execution registry (`current_tt` / `current_session`) and it is
    the resolution authority — `resolve_server` / `resolve_clock` implement the
    single rule shared with the free `clausters.play` and every playable's
    ambient ``.play()``. It also keeps an opt-in `default_clock`. The default
    `server` is adopted first-wins by a free-standing ``Server.boot()``.
    """

    def __init__(self):
        super().__init__()          # RandomContext + the `server` slot (None)
        #: opt-in convenience default clock; ``None`` until first needed (see
        #: `get_default_clock`). An explicit `Session` brings its own clocks.
        self.default_clock = None
        self._local = threading.local()

    @property
    def current_tt(self):
        """The routine being resumed on **this thread** (thread-local), set by
        the clock around each wake, so ``Server`` can read the running routine's
        exact logical beat -- and so resolution can reach its session via
        ``current_tt.clock.session``. ``None`` outside a routine."""
        return getattr(self._local, "current_tt", None)

    @current_tt.setter
    def current_tt(self, value):
        self._local.current_tt = value

    @property
    def current_session(self):
        """The explicit `clausters.Session` active on **this thread**
        (thread-local), set by a session while it plays/renders or as a context
        manager, so material created outside any routine still resolves to that
        session's server/clock/rng. ``None`` means the default session (`main`)
        itself — the fallback when no session was named."""
        return getattr(self._local, "current_session", None)

    @current_session.setter
    def current_session(self, value):
        self._local.current_session = value

    # ---- ambient resolution (the single rule) ----

    def _ambient_session(self):
        """The session an ambient play belongs to: the running routine's
        (``current_tt.clock.session``), else the explicit `current_session`
        active on this thread, else ``None`` (the default session, ``self``)."""
        sess = getattr(getattr(self.current_tt, "clock", None), "session", None)
        return sess if sess is not None else self.current_session

    def resolve_server(self, server=None):
        """The server a free-standing play should target: the explicit ``server``
        if given, else the ambient session's server (the running routine's, or
        the `current_session` active on this thread), else the default session's
        `server`. Raises if none has been booted."""
        if server is not None:
            return server
        sess = self._ambient_session()
        if sess is not None and getattr(sess, "server", None) is not None:
            return sess.server
        if self.server is not None:
            return self.server
        raise RuntimeError(
            "no server to play on: boot one with Server.boot() (or open a "
            "Session.live()/embed()), or pass server=..."
        )

    def resolve_clock(self, clock=None):
        """The clock a play should schedule on: the explicit ``clock`` if given,
        else the clock of the routine running on this thread, else the ambient
        session's clock, else the default session's `default_clock` (which may be
        ``None`` -- the caller then plays immediately, or calls
        `get_default_clock`)."""
        if clock is not None:
            return clock
        c = getattr(self.current_tt, "clock", None)
        if c is not None:
            return c
        sess = self.current_session
        if sess is not None and getattr(sess, "clock", None) is not None:
            return sess.clock
        return self.default_clock

    def get_default_clock(self, start: bool = True):
        """The default session's clock, created (tempo 1.0) on first use and,
        when ``start`` is set, started so a routine or pattern played on it fires
        in real time. This is what an ambient `Pattern`/`Routine` play uses when
        no clock is in context."""
        if self.default_clock is None:
            from .clock import TempoClock

            self.default_clock = TempoClock()
            self.default_clock.session = self
        if start and not self.default_clock._running:
            self.default_clock.start()
        return self.default_clock

    # ---- random context (seed / rng inherited from RandomContext) ----

    def elapsed_beats(self) -> float:
        return self.default_clock.beats() if self.default_clock else 0.0


# The process-wide default session (its `current_tt` is per-thread).
main = Main()

#: Public alias — the same object, named for what it is.
default_session = main
