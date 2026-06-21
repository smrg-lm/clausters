"""Session: ergonomic defaults without global state.

The project rule is to avoid global state (see the memory
``evitar-estados-globales-clausters``), but sc3's ease of use came largely from
its globals (`Server.default`, the default clock). A `Session` gives that
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
    def __init__(self, server: Server, clock: TempoClock | None = None):
        self.server = server
        self.clock = clock if clock is not None else TempoClock()

    # ---- factories (the "defaults", explicit) ----

    @classmethod
    def nrt(cls, tempo: float = 1.0) -> "Session":
        """An offline session: an NRT server that accumulates a score to
        `render`."""
        return cls(Server(interface=OscNrtInterface()), TempoClock(tempo))

    @classmethod
    def live(cls, host: str = "127.0.0.1", port: int = 57110, tempo: float = 1.0,
             latency: float = 0.0, timebase=None) -> "Session":
        """A real-time session talking to a running server over UDP."""
        return cls(Server(host, port, latency=latency), TempoClock(tempo, timebase=timebase))

    # ---- driving ----

    def play(self, pattern, quant=None):
        """Play an event pattern on this session's clock and server."""
        return pattern.play(self.clock, self.server)

    def render(self, sample_rate: float = 48_000.0, channels: int = 2):
        """Offline only: drain the clock logically, then render the score."""
        self.clock.render()
        return self.server.render(sample_rate=sample_rate, channels=channels)

    def run(self, seconds: float):
        """Real-time: run the clock for ``seconds`` then stop."""
        self.clock.run(seconds)
        return self

    def start(self):
        self.clock.start()
        return self

    def stop(self):
        self.clock.stop()
        return self

    def close(self):
        self.server.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
