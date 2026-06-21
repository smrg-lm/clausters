"""Streams and routines (port of ``sc3/base/stream.py``).

The coroutine layer. A `Routine` wraps a Python **generator function**;
driving it resumes the generator, and the value it ``yield``s is a *time to
wait* (in beats) before the next resumption. The thing that resumes routines on
a schedule is the clock (`clausters.base.clock`); here we only define the
protocol. This is the part that stays in the host language — ``yield`` is
Python control flow and never moves to Rust (see ``clients/PLAN.md``).
"""

import inspect


class StopStream(StopIteration):
    """Raised (and caught) to end a stream normally."""


class YieldAndReset(Exception):
    """Yield ``value`` and reset the routine to its initial state."""

    def __init__(self, value=None):
        super().__init__()
        self.value = value


class Stream:
    """A lazy sequence: implements the iterator protocol over `next`."""

    def __iter__(self):
        return self

    def __next__(self):
        try:
            return self.next()
        except StopStream:
            raise StopIteration from None

    def next(self, inval=None):
        raise NotImplementedError(f"{type(self).__name__}.next")

    def reset(self):
        pass


class FunctionStream(Stream):
    """Wraps a plain callable: each `next` calls it with ``inval``."""

    def __init__(self, func, reset_func=None):
        self.func = func
        self.reset_func = reset_func
        self.clock = None
        self._logical_beat = 0.0

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
    asynchronous form -- ``server.add_faustdef(fdef, wait=False)`` (or
    ``add_synthdef(..., wait=False)``) -- which only sends; do *not* call the
    blocking ``server.sync()`` here. A non-blocking, notification-driven
    barrier you can ``yield`` (``OSCFunc``) is future work; until then, send
    the def async and ``yield`` enough time before the ``/s_new`` that depends
    on it.
    """

    def __init__(self, func):
        self.func = func
        self._gen = None
        self._started = False
        self.state = "init"  # init | running | done | paused
        #: the clock currently driving this routine (set by the clock on wake),
        #: so a Server can read the logical time. None when not playing.
        self.clock = None
        #: the exact logical beat at which the clock last resumed this routine
        #: (yield-accumulated, not wall-clock); the Server emits timetags from it.
        self._logical_beat = 0.0

    @classmethod
    def run(cls, func):
        """Decorator/constructor sugar: ``@Routine.run`` over a genfunc."""
        return cls(func)

    def reset(self):
        self._gen = None
        self._started = False
        self.state = "init"

    def next(self, inval=None):
        if self.state == "done":
            raise StopStream
        try:
            if not self._started:
                self._started = True
                self.state = "running"
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
        """Schedule this routine on ``clock`` (the default clock if None)."""
        from .main import main

        clock = clock or main.default_clock
        if clock is None:
            raise RuntimeError("no clock to play on (set main.default_clock)")
        clock.play(self, quant)
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
