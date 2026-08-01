"""Streams and routines (port of ``sc3/base/stream.py``).

The coroutine layer. A `Routine` wraps a Python **generator function**;
driving it resumes the generator, and the value it ``yield``s is a *time to
wait* (in beats) before the next resumption. The thing that resumes routines on
a schedule is the clock (`clausters.base.clock`); here we only define the
protocol. This is the part that stays in the host language — ``yield`` is
Python control flow and never moves to Rust.
"""

import inspect


class StopStream(StopIteration):
    """Raised (and caught) to end a stream normally."""


class YieldAndReset(Exception):
    """Raise from inside a routine to yield ``value`` and then reset the routine
    to its initial state, so its next resumption restarts the generator."""

    def __init__(self, value=None):
        super().__init__()
        self.value = value


class Stream:
    """A lazy sequence: implements the iterator protocol over `next`.

    Concrete streams carry their own random generator (`rng`), derived from the
    creating context at construction (see `clausters.base.rand`): random values
    drawn while a stream runs come from *its* stream, so one root seed
    (``main.seed``) reproduces a whole script and concurrent routines stay
    reproducible per routine."""

    #: the stream's random generator; ``None`` falls back to the root context.
    rng = None

    def __iter__(self):
        return self

    def __next__(self):
        try:
            return self.next()
        except StopStream:
            raise StopIteration from None

    def next(self, inval=None):
        """Produce the next value, optionally fed ``inval``; raise `StopStream`
        to end. Subclasses implement this -- the base raises
        ``NotImplementedError``."""
        raise NotImplementedError(f"{type(self).__name__}.next")

    def reset(self):
        """Return the stream to its initial state so iteration restarts. A no-op
        on the base; stateful subclasses override it."""
        pass


class FunctionStream(Stream):
    """Wraps a plain callable: each `next` calls it with ``inval``."""

    def __init__(self, func, reset_func=None):
        from . import rand

        self.func = func
        self.reset_func = reset_func
        self.clock = None
        self._logical_beat = 0.0
        self.rng = rand.spawn_rng()   # own stream, seeded by the creating context

    def next(self, inval=None):
        try:
            return _call(self.func, inval)
        except StopStream:
            raise

    def reset(self):
        if self.reset_func is not None:
            _call(self.reset_func, None)


class Routine(Stream):
    """Wraps a generator function into a resumable timeline.

    The generator may take zero or one positional argument (the initial
    ``inval``). Each `next` resumes it; a ``yield``ed number is the delay
    before the routine should be resumed again.

    **The generator must never block the thread — that is the user's
    responsibility.** A routine runs *on the clock thread* (RT) or inside the
    render loop (NRT); blocking it (``time.sleep``, a blocking
    ``Server.sync``/``wait=True`` def send, any synchronous wait-for-reply)
    stalls every other routine and the whole timeline. Cede time with ``yield``
    instead. In particular, to **create a def from inside a routine** use the
    asynchronous form -- ``fdef.send(server, wait=False)`` (or
    ``sdef.send(server, wait=False)``) -- which only sends; do *not* call the
    blocking ``server.sync()`` here. A non-blocking, notification-driven
    barrier you can ``yield`` (``OSCFunc``) is future work; until then, send
    the def async and ``yield`` enough time before the ``/synth_new`` that depends
    on it.
    """

    def __init__(self, func):
        from . import rand

        self.func = func
        self._gen = None
        self._started = False
        self.state = "init"  # init | running | done | paused
        #: the clock driving this routine -- set by `play` when it schedules it
        #: and again by the clock on each wake, so a Server can read the logical
        #: time and `pause`/`stop` know where to unschedule from. None until it
        #: has been played.
        self.clock = None
        #: the exact logical beat at which the clock last resumed this routine
        #: (yield-accumulated, not wall-clock); the Server emits timetags from it.
        self._logical_beat = 0.0
        #: the routine's own random generator, seeded from the creating context
        #: (sclang-style inheritance): everything random drawn while this
        #: routine runs comes from here, so a root ``main.seed`` reproduces it.
        self.rng = rand.spawn_rng()

    @classmethod
    def run(cls, func, clock=None, quant=None):
        """Wrap ``func`` in a `Routine` and `play` it at once; returns the
        routine, already scheduled.

        Sugar for ``Routine(func).play(clock, quant)``, and sclang's
        ``Routine.run``. Both arguments resolve exactly as in `play`, so with
        neither the routine lands on the ambient clock -- no `clausters.Session`
        and no booted server needed. Reads as a decorator over the definition,
        which is its point: the name is left bound to the routine itself, not to
        the function, so it can still be paused and stopped (``melody.stop()``).

            @Routine.run
            def melody():
                ...
                yield 0.5
        """
        return cls(func).play(clock, quant)

    def reset(self):
        """Discard the running generator and return to the ``init`` state, so
        the next `next` or `play` starts the generator function afresh."""
        self._gen = None
        self._started = False
        self.state = "init"

    def next(self, inval=None):
        """Resume the generator once (sending it ``inval``) and return the value
        it yields -- a delay in beats -- or raise `StopStream` when it finishes.
        The clock calls this on each wake; you rarely call it directly."""
        if self.state == "done":
            raise StopStream
        self.state = "running"      # also what resumes a paused routine
        try:
            if not self._started:
                self._started = True
                self._gen = _make_gen(self.func, inval)
                return self._gen.send(None)
            return self._gen.send(inval)
        except StopIteration:
            self.state = "done"
            raise StopStream from None
        except YieldAndReset as e:
            self.reset()
            return e.value

    def play(self, clock=None, quant=None):
        """Schedule this routine to start on ``clock``; returns ``self``.

        Args:
            clock: the `TempoClock` to run on. ``None`` resolves against the
                ambient context like every other play: the clock of the routine
                running on this thread, else the active session's, else the
                default session's — created and started on first use. So
                ``Routine(f).play()`` runs with no `clausters.Session` and no
                booted server; a routine needs a clock, not an engine.
            quant: start quantization, forwarded to the clock (see
                `TempoClock.play`; not yet implemented).
        """
        from .main import main

        clock = clock or main.resolve_clock() or main.get_default_clock()
        self.clock = clock          # known from scheduling, not only from waking
        clock.play(self, quant)
        return self

    def pause(self):
        """Take this routine off its clock, keeping its position; returns
        ``self``. The generator is untouched, so a later `play` resumes it at the
        very ``yield`` it was paused on -- the counterpart of `reset`, which
        throws that position away. Pausing a routine that is not scheduled does
        nothing."""
        if self.clock is not None:
            self.clock.unsched(self)
        if self.state == "running":
            self.state = "paused"
        return self

    def stop(self):
        """Take this routine off its clock **and** rewind it; returns ``self``.
        `pause` followed by `reset`: a later `play` starts the generator function
        afresh, from the top.

        Note this is a routine's own transport, not `TempoClock.stop` -- that one
        halts the *clock*, holding the beat it reached for every routine on it.
        """
        self.pause()
        self.reset()
        return self

    # Called by the clock when it is this routine's turn.
    def __awake__(self, clock):
        return self.next(clock)


def _make_gen(func, inval):
    """Creates the generator, passing ``inval`` if the function accepts it."""
    try:
        params = inspect.signature(func).parameters
        takes_arg = len(params) >= 1
    except (TypeError, ValueError):
        takes_arg = False
    gen = func(inval) if takes_arg else func()
    if not inspect.isgenerator(gen):
        raise TypeError("Routine expects a generator function")
    return gen


def _call(func, inval):
    try:
        params = inspect.signature(func).parameters
        return func(inval) if len(params) >= 1 else func()
    except (TypeError, ValueError):
        return func()
