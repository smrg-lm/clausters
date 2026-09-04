"""`AppClock`: the clock face over an event loop -- the application's time.

Three clocks are three questions, and mixing them is how a program ends up with
two scheduling vocabularies:

- `clausters.base.clock.TempoClock` keeps **musical** time. It is in beats, it
  is what a piece plays on, and a routine on it must never block.
- **`AppClock` keeps the application's time.** It is in **seconds**, it runs on
  the loop that drains the windows, and it is where anything that touches a
  window belongs: an animation, a periodic read-out, a redraw, a follow-up to a
  gesture.
- The audio server keeps sample time, which is neither of these and is not a
  client clock at all.

This is sclang's reading of the same split (``SystemClock`` /
``TempoClock`` / ``AppClock``), and the part worth taking from it is not the
name: it is that **the loop's timer source and the clock are one object**. An
animation is then a routine that waits --

    def blink():
        while True:
            handle.set(color="red")
            yield 0.25
            handle.set(color="grey")
            yield 0.25

    app_clock().play(Routine(blink))

-- rather than an animation API beside the routines the client already has.

**`defer` is the other half**, and it is what the standing rule was missing. A
routine on the `TempoClock` must never block its thread, so until now it had
nowhere to put work that touches a window; ``defer`` hands that work to the
loop's thread and returns immediately.
"""

from .stream import resume


class AppClock:
    """Seconds on an event loop.

    Args:
        loop: the `clausters.base.loop.EventLoop` that drives it. One clock per
            loop, and a host hands out its own (`clausters.gui.host.GuiHost.clock`,
            or `clausters.gui.app_clock` for the ambient one) rather than
            building a second.

    It holds no thread and no queue of its own: the loop's timers *are* the
    schedule, so a routine on this clock and a redraw the loop does are ordered
    against each other rather than racing.
    """

    def __init__(self, loop):
        #: The loop this clock schedules on.
        self.loop = loop
        self._handles: dict = {}       # id(item) -> [handles]
        #: The reading `elapsed` counts from.
        self._origin = loop.now()

    # ---- reading the time ----

    def elapsed(self) -> float:
        """Seconds since this clock was made -- the reading `sched` measures a
        delay from, and the one an animation asks for its phase."""
        return self.loop.now() - self._origin

    #: The monotonic reading itself, for a caller pairing it with `sched_abs`.
    def now(self) -> float:
        """The loop's own clock reading, which `sched_abs` takes."""
        return self.loop.now()

    # ---- scheduling ----

    def sched(self, delay: float, item):
        """Run ``item`` ``delay`` **seconds** from now, on the loop's thread.

        ``item`` is a `clausters.base.stream.Routine` (or any `Stream`), or a
        plain callable for a one-shot. A routine is rescheduled by whatever it
        ``yield``s, a callable by whatever number it returns, and one returning
        ``None`` runs once -- the same contract `TempoClock.sched` states, in
        the other unit.

        Safe from any thread, which is the point of the loop having a wake
        channel: scheduling from the clock thread or from a callback does not
        wait for the current wait to expire.
        """
        return self._remember(item, self.loop.sched(delay, self._waker(item)))

    def sched_abs(self, when: float, item):
        """`sched` against an absolute reading of `now`."""
        return self._remember(item, self.loop.sched_abs(when, self._waker(item)))

    def play(self, routine):
        """Schedule ``routine`` to start now, and return it.

        There is no ``quant`` here and there should not be: quantization is a
        musical grid and this clock has none -- a routine that must land on a
        beat belongs on the `TempoClock`, and one that must touch a window from
        there gets here through `defer`.
        """
        self.sched(0.0, routine)
        return routine

    def defer(self, func):
        """Run ``func`` on the loop's thread as soon as it comes round, and
        return immediately.

        The door from any other thread -- and the one a routine on the
        `TempoClock` uses, since its own thread must not block: what it defers
        runs where the windows are drained, in the loop's own order.
        """
        self.loop.post(func)
        return func

    def unsched(self, item) -> bool:
        """Cancel whatever is queued for ``item``, leaving the rest in order."""
        handles = self._handles.pop(id(item), ())
        return any(self.loop.cancel(h) for h in handles)

    # ---- internals ----

    def _waker(self, item):
        """The loop-side callable for one item: resume it, and hand the loop the
        delay it asked for so the loop's own requeue does the rest.

        The resumption is `clausters.base.stream.resume`, the same one the
        `TempoClock` drives with -- what driving a routine means is written
        once, so the two clocks cannot come to mean different things by it.
        """
        def wake():
            return resume(item, self)

        return wake

    def _remember(self, item, handle):
        self._handles.setdefault(id(item), []).append(handle)
        return handle

    def __repr__(self):
        return f"AppClock(elapsed={self.elapsed():.3f}s, loop={self.loop.name!r})"
