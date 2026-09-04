"""The client's event loop: one wait over everything that can become ready.

Every application event loop is the same machine -- Cocoa's ``NSRunLoop``,
GLib's ``GMainLoop``, Qt's ``QEventLoop``, libuv, the browser -- and this is
that machine with nothing else in it:

- **sources.** Anything that can become ready and hand over items: a socket, a
  queue. A source is registered with `add_source` and drained by the loop.
- **one wait**, bounded by the nearest timer rather than by a fixed sleep. That
  is the property that separates a loop from a poll: it costs nothing while
  nothing happens, and it wakes on the *first* of "an item arrived" and "a timer
  is due".
- **a wake channel**, so another thread that posts work does not have to wait
  for the current wait to expire.
- **fixed phases.** One iteration is: posted work, then due timers, then the
  wait, then the items it produced. The order is a contract -- what a handler
  posts runs in the *next* iteration, never re-entrantly -- which is what makes
  a callback that edits, closes a window or schedules more work safe to write.
- **`run` or `iterate`.** The loop can own a thread (`start`/`stop`) or be
  driven one turn at a time by somebody else's (`iterate`), which is what lets a
  notebook or a test drive it without a thread at all.

**The timer queue is the shared core's.** `clausters._native.Scheduler` --
a min-heap by time with stable insertion order -- is what
`clausters.base.clock.TempoClock` already schedules beats through, and this
schedules seconds through the same one. There is no second queue, and no client
re-derives what "due" means.

**What is *not* here is deliberate**: no widgets, no OSC, no GUI. A `GuiHost`
registers itself as a source and `clausters.base.appclock.AppClock` is the
clock face over the timers; the loop knows about neither.

**Handlers run on the loop's thread, so what they write is read by another
one.** Measured before this existed: a timeline a projection rebuilt in place
was seen half-rebuilt in 87.7% of reads at 4000 notes -- and in none at all at 8,
because CPython's switch interval hides a short mutation, which is exactly what
makes this the class of bug that passes every small test and fails a real piece.

Two things answer it, and the order matters. **What a handler writes, it writes
in one step**: `clausters.seq.Timeline.replace` binds a new order rather than
clearing and re-adding, and a curve's projection is already a single rebinding,
so an ordinary read needs no ceremony at all (that measurement is 0 of 5099 at
the same size). **And the loop holds `lock` across a whole delivery** for
everything a swap cannot cover -- several structures written together, a handler
of the caller's own -- so a reader that needs more than one fact at a time takes
the same lock.
"""

import os
import selectors
import threading
import time
from collections import deque

from .. import _native

#: The longest a wait lasts when no timer is closer. A loop with nothing
#: scheduled still wakes this often, which is what bounds how long `stop` and a
#: `post` from another thread take on a source that cannot be selected on.
IDLE_WAIT = 0.05


class Source:
    """What the loop drains. Subclass it, or duck-type these three.

    Args:
        (none) -- a source is defined by its methods.

    A source that can be waited on says so with `fileno`; the loop then blocks
    in one `selectors` call over every source at once and wakes the moment any
    of them has something. One that cannot (a ring buffer, a queue with no
    descriptor) is polled with the loop's own timeout instead, which is the same
    thing with a coarser floor.
    """

    def fileno(self):
        """The descriptor to wait on, or ``None`` when there is none."""
        return None

    def read(self, timeout: float = 0.0):
        """The next item, or ``None`` within ``timeout``.

        Called with ``timeout=0`` once the loop already knows the source is
        ready, and with the loop's own timeout when it cannot be waited on.
        """
        raise NotImplementedError

    def deliver(self, item):
        """Act on one item. Runs on the loop's thread, holding the loop's lock."""
        raise NotImplementedError


class _Callback(Source):
    """A source built from two functions, for a caller that has no class."""

    def __init__(self, read, deliver, fileno=None):
        self._read, self._deliver, self._fileno = read, deliver, fileno

    def fileno(self):
        return self._fileno() if callable(self._fileno) else self._fileno

    def read(self, timeout: float = 0.0):
        return self._read(timeout)

    def deliver(self, item):
        return self._deliver(item)


class EventLoop:
    """One loop over a set of sources and a timer queue.

    Args:
        name: the thread's name, for a traceback that has to say which loop.

    Built directly by whoever owns one -- `clausters.gui.host.GuiHost` keeps one
    per host, since there is one inbound carrier per host and therefore one
    thing to drain.
    """

    def __init__(self, name: str = "EventLoop"):
        self.name = name
        #: Held across a whole delivery and across a timer's call. Take it to
        #: read anything a handler writes; it is re-entrant, so a handler may.
        self.lock = threading.RLock()
        self._sources: list = []
        self._queue = _native.Scheduler()
        self._timers: dict = {}          # id -> [func, pending]
        self._next_id = 1
        self._posted: deque = deque()
        self._sel = selectors.DefaultSelector()
        self._registered: dict = {}      # id(source) -> fd
        self._wake_r, self._wake_w = os.pipe()
        os.set_blocking(self._wake_r, False)
        self._sel.register(self._wake_r, selectors.EVENT_READ, None)
        self._running = False
        self._thread = None
        self._stop = threading.Event()

    # ---- what the loop watches ----

    def add_source(self, source=None, *, read=None, deliver=None, fileno=None):
        """Register a `Source` (or the three functions one is made of) and
        return it. Safe from any thread; the loop picks it up on its next turn."""
        if source is None:
            source = _Callback(read, deliver, fileno)
        with self.lock:
            self._sources.append(source)
            self._resync_selector()
        self.wake()
        return source

    def remove_source(self, source):
        """Stop draining ``source``."""
        with self.lock:
            if source in self._sources:
                self._sources.remove(source)
            self._resync_selector()
        self.wake()

    def _resync_selector(self):
        """Register exactly the sources that can be waited on. Called under the
        lock, so the selector and the source list cannot disagree."""
        wanted = {}
        for source in self._sources:
            fd = source.fileno()
            if fd is not None:
                wanted[fd] = source
        for fd in list(self._registered.values()):
            if fd not in wanted:
                try:
                    self._sel.unregister(fd)
                except (KeyError, ValueError):
                    pass
        for fd, source in wanted.items():
            if fd not in self._registered.values():
                try:
                    self._sel.register(fd, selectors.EVENT_READ, source)
                except (KeyError, ValueError):
                    continue
        self._registered = {id(s): fd for fd, s in wanted.items()}

    # ---- time ----

    def now(self) -> float:
        """The loop's clock: seconds on the monotonic timebase every deadline
        here is measured against."""
        return time.monotonic()

    def sched(self, delay: float, func):
        """Run ``func`` on the loop's thread in ``delay`` seconds; returns a
        handle `cancel` takes. A ``func`` returning a number is rescheduled by
        that many seconds, which is how a periodic task is written without a
        loop of its own."""
        return self.sched_abs(self.now() + float(delay), func)

    def sched_abs(self, when: float, func):
        """`sched` against an absolute reading of `now`."""
        with self.lock:
            key = self._next_id
            self._next_id += 1
            self._timers[key] = [func, 1]
            self._queue.push(float(when), key)
        self.wake()
        return key

    def cancel(self, handle) -> bool:
        """Drop a timer `sched` handed back. Returns whether it was still
        queued -- a timer that already ran, or one cancelled twice, is ``False``
        rather than an error, since a caller cannot know which."""
        with self.lock:
            removed = self._queue.remove(int(handle))
            self._timers.pop(int(handle), None)
        return removed > 0

    def post(self, func):
        """Run ``func`` on the loop's thread, at the start of its next
        iteration. The cross-thread door -- and the one a routine on the clock
        thread reaches through `clausters.base.appclock.AppClock.defer`, since
        that thread must never block."""
        self._posted.append(func)
        self.wake()

    def wake(self):
        """Break the current wait. Safe from any thread, including a signal
        handler; a byte down a pipe is all it is."""
        try:
            os.write(self._wake_w, b"\x01")
        except (BlockingIOError, OSError):
            pass

    # ---- driving ----

    @property
    def running(self) -> bool:
        """Whether a thread of this loop's own is driving it."""
        return self._running

    def start(self):
        """Drive the loop on a daemon thread of its own, and return it.

        A thread rather than the main one, which is where this differs from
        sclang's ``AppClock`` and has to: there the app owns the main thread and
        inherits its safety, here the main thread is the user's program. It is
        the arrangement `clausters.base._oscinterface.OscReceiver` already uses
        to reach the audio server, for the same reason -- a shared thread would
        let one leg's drain delay the other's."""
        if self._running:
            return self
        self._stop.clear()
        self._running = True
        self._thread = threading.Thread(target=self._run, name=self.name,
                                        daemon=True)
        self._thread.start()
        return self

    def stop(self, timeout: float = 1.0):
        """Ask the loop's thread to finish its turn and end."""
        if not self._running:
            return self
        self._stop.set()
        self._running = False
        self.wake()
        if self._thread is not None and self._thread is not threading.current_thread():
            self._thread.join(timeout=timeout)
        self._thread = None
        return self

    def _run(self):
        while not self._stop.is_set():
            self.iterate(IDLE_WAIT)

    def iterate(self, timeout: float = 0.0) -> bool:
        """Run **one** turn of the loop and say whether anything happened.

        The phases, in order: posted work, due timers, the wait, the items it
        produced. A handler that posts or schedules is therefore served by the
        *next* turn, never inside this one.

        This is the whole loop for a caller that has a thread already -- a
        notebook cell, a test, a program embedding this in a loop of its own --
        and is what `start` calls in a thread when nobody does.
        """
        did = self._run_posted()
        did |= self._run_timers()
        wait = self._until_next_timer(timeout)
        did |= self._drain(wait)
        return did

    def _run_posted(self) -> bool:
        """The batch that was already waiting when this turn began -- and only
        that one. What a handler posts joins the *next* turn, which is the whole
        of the phase contract: a callback that posts more work cannot starve the
        sources by feeding the same phase forever."""
        did = False
        for _ in range(len(self._posted)):
            try:
                func = self._posted.popleft()
            except IndexError:
                break
            with self.lock:
                _call(func)
            did = True
        return did

    def _run_timers(self) -> bool:
        did = False
        while True:
            now = self.now()
            with self.lock:
                due = self._queue.pop_due(now)
                if due is None:
                    break
                when, key = due
                entry = self._timers.get(key)
                if entry is None:
                    continue
                func = entry[0]
                # Outside the queue but inside the lock: a timer writing what
                # the script reads is the same contract as a delivery.
                again = _call(func)
                if isinstance(again, (int, float)):
                    self._timers[key] = [func, 1]
                    self._queue.push(when + float(again), key)
                else:
                    self._timers.pop(key, None)
            did = True
        return did

    def _until_next_timer(self, timeout: float) -> float:
        with self.lock:
            when = self._queue.peek_time()
        if when is None:
            return max(0.0, timeout)
        return max(0.0, min(timeout, when - self.now()))

    def _drain(self, timeout: float) -> bool:
        """The wait, and the items it produced.

        Sources with a descriptor are waited on together, so the loop sleeps in
        one call and wakes on whichever speaks first. Sources without one are
        read with the same timeout, shared -- a coarser floor and the same
        shape.
        """
        with self.lock:
            selectable = [s for s in self._sources if s.fileno() is not None]
            blind = [s for s in self._sources if s.fileno() is None]
        did = False
        if selectable or not blind:
            for key, _mask in self._sel.select(timeout if not blind else 0.0):
                if key.data is None:
                    try:
                        os.read(self._wake_r, 4096)
                    except (BlockingIOError, OSError):
                        pass
                    continue
                did |= self._deliver_all(key.data)
        if blind:
            share = (timeout / len(blind)) if timeout > 0 else 0.0
            for source in blind:
                item = source.read(share)
                if item is None:
                    continue
                with self.lock:
                    source.deliver(item)
                did = True
                did |= self._deliver_all(source)
        return did

    def _deliver_all(self, source) -> bool:
        """Drain a ready source until it has nothing more, so one wake empties
        a burst instead of leaving the rest for the next turn."""
        did = False
        while True:
            item = source.read(0.0)
            if item is None:
                return did
            with self.lock:
                source.deliver(item)
            did = True

    def close(self):
        """Stop the loop and release its pipe and selector."""
        self.stop()
        try:
            self._sel.close()
        except Exception:
            pass
        for fd in (self._wake_r, self._wake_w):
            try:
                os.close(fd)
            except OSError:
                pass

    def __repr__(self):
        return (f"EventLoop({self.name!r}, running={self._running}, "
                f"sources={len(self._sources)}, timers={len(self._queue)})")


def _call(func):
    """Call one handler and survive it.

    A handler that raises loses its turn and nothing else -- the same rule the
    clock applies to a routine, and for the same reason: this thread drives
    every other source, and a dead one leaves a loop that reports itself running
    while draining nobody.
    """
    import traceback

    try:
        return func()
    except Exception:
        traceback.print_exc()
        return None
