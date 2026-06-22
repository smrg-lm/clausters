"""Session: ergonomic defaults without global state.

The client deliberately avoids global state, but sc3's ease of use came largely
from its globals (`Server.default`, the default clock). A `Session` gives that
ergonomics back **explicitly**: it bundles a `Server`
and a `TempoClock` into one handle with ``play`` /
``render`` / ``run``, and the ``nrt`` / ``live`` factories pick sensible
defaults. Because it is an ordinary object, **several sessions coexist** — e.g.
one offline NRT session for plotting next to a live RT one — in the same script,
which globals make impossible.

```python
s = Session.nrt(tempo=2.0)
s.play(Pbind(instrument="default", freq=Pseq([440, 550, 660]), dur=0.5))
samples, frames = s.render()        # drains the clock, renders the score
```
"""

from .base import OscNrtInterface, TempoClock
from .defs import Server


class Session:
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
        self.server = server
        self.clock = clock if clock is not None else TempoClock()

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
    def live(cls, host: str = "127.0.0.1", port: int = 57110, tempo: float = 1.0,
             latency: float = 0.0, timebase=None) -> "Session":
        """Build a real-time session talking to a running server over UDP.

        Args:
            host: the server's host.
            port: the server's UDP port (the Clausters default is 57110).
            tempo: the clock's tempo, in beats per second.
            latency: seconds added to each event's timetag so it reaches the
                server slightly ahead of its play time and sounds on time
                instead of late; a small value such as 0.1 is typical for a
                live take.
            timebase: the clock's pacing source. The default (monotonic) paces
                in wall-clock seconds; a `SampleClockTimebase` anchors timing to
                the server's sample clock for drift-free, sample-accurate
                scheduling.

        Returns:
            A `Session` you drive with `run` (or `start` / `stop`).
        """
        return cls(Server(host, port, latency=latency), TempoClock(tempo, timebase=timebase))

    # ---- driving ----

    def play(self, pattern, quant=None):
        """Play an event pattern on this session's clock and server.

        Args:
            pattern: an event pattern, e.g. a `Pbind`.
            quant: optional quantization handed to the player -- the beat grid
                the routine starts on; ``None`` starts immediately.

        Returns:
            The `EventStreamPlayer` driving the pattern.
        """
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
        tracker (from `lock_to_server`), if any. Done automatically when the
        session is used as a context manager."""
        self.clock.close()
        self.server.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
