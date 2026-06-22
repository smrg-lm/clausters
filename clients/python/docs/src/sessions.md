# Sessions

A `Session` is the client's ergonomic entry point: one object that owns a `Server` and a `TempoClock` together and drives them as a unit. It exists so that the everyday case — "set up a place to make sound, play a pattern, hear it or render it" — is a couple of lines, without giving up the design that makes this client flexible.

## Why it exists

SuperCollider's sclang is convenient largely because of globals: a default `Server`, a default clock, an implicit "current environment". You type `Synth("x")` and it just goes somewhere. That convenience has a cost — there is only ever *one* of each, so you cannot, say, run a live take and an offline render in the same script without them fighting over the same global state.

This client deliberately has none of those globals. The clock does timing and nothing else, a `Server` is an ordinary object you construct, and routines find their clock through thread-local context rather than a global. That keeps real-time and offline work independent, but it means the convenient one-liner is gone: you would otherwise wire a server, a clock and a timebase together by hand every time.

`Session` gives the convenience back **explicitly**. It is just an object that holds a `Server` and a `TempoClock` and offers `play` / `render` / `run`, plus two factories that pick sensible defaults. Because it is a plain object and not a global, you can have as many as you like.

## Two kinds of session

You almost always build a session with one of the two factories rather than the constructor.

`Session.nrt()` is an **offline** (non-real-time) session. Its server accumulates a timetagged *score* instead of sending anything, and `render()` turns that score into samples through the renderer bundled with the package. No server process and no audio device are involved.

```python
from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

session = Session.nrt(tempo=2.0)
session.play(Pbind(
    degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
    dur=0.25,
    amp=Pwhite(0.1, 0.2, seed=1),
))
samples, frames = session.render(sample_rate=48000.0, channels=2)
print(f"{frames} frames; peak {max(abs(s) for s in samples):.3f}")
```

`Session.live()` is a **real-time** session that talks to a running server over UDP (start one with `cargo run --release`, or the installed `clausters` binary). The same pattern, played the same way, now sounds on a device.

```python
from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

with Session.live(tempo=2.0, latency=0.1) as session:
    session.play(Pbind(
        instrument="default",
        degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=Pwhite(0.1, 0.2, seed=1),
    ))
    session.run(3.5)   # advance the clock in real time, then stop
```

Two arguments are worth knowing on `live()`:

- `latency` — seconds added to each event's timetag so it arrives a touch ahead of its play time and the server sounds it *on* time rather than late. `0.0` means "as soon as possible"; a small value such as `0.1` is typical for a live take.
- `timebase` — the clock's pacing source. The default paces in wall-clock seconds (monotonic); passing a `SampleClockTimebase` anchors timing to the server's own sample clock, for drift-free, sample-accurate scheduling. See [The client, layer by layer](guide.md) for the timebase distinction.

## Driving a session

Once you have a session, a small set of methods drives it. Some are offline-only, some live-only — the table makes the split explicit.

| Call | Kind | What it does |
| --- | --- | --- |
| `play(pattern, quant=None)` | both | Plays an event pattern (e.g. a `Pbind`) on this session's clock and server. Returns the `EventStreamPlayer`. `quant` is the beat grid to start on; `None` starts immediately. |
| `render(sample_rate, channels)` | offline | Drains the clock logically (no waiting), then renders the score. Returns `(samples, frames)` — interleaved float32 in a stdlib `array('f')`, and the frame count. |
| `run(seconds)` | live | Starts the clock, advances it in real time for `seconds`, then stops. Returns `self`. |
| `start()` / `stop()` | live | Start or stop the real-time clock yourself when `run` (which does both) is not enough. Both return `self`. |
| `close()` | both | Closes the underlying `Server` and its interface. |

`play` is the one call shared by both kinds, and that is the whole point — see the next section.

Because `close()` releases the server, the idiomatic shape for a live session is a context manager, which closes it for you even if the block raises:

```python
with Session.live(tempo=2.0) as session:
    session.play(my_pattern)
    session.run(4.0)
# server closed here
```

An offline session holds no socket and renders synchronously, so the context manager is optional there — though harmless, and tidy if you mix both.

One rule carries over from the rest of the client: a routine must never block the clock thread. `render()` and `run()` are *driver* calls you make from your own (main) thread, not from inside a routine — they advance the clock, so calling them from a routine the clock is running would deadlock it.

## The same code, live or offline

The reason a session draws no line between "play" and "render" is the client's central design property, the *seam*: a `Server` holds one communication interface, and which interface it holds — not your pattern, not your clock — decides where the bytes go. A live session's server holds a UDP interface; an offline session's server holds a score-accumulating one. Everything above the server is identical.

So the only thing that changes between a live take and an offline render is **which factory you called**. You can write the pattern once and run it both ways:

```python
from clausters import Session
from clausters.seq import Pbind, Pseq, Pwhite

def phrase():
    return Pbind(
        instrument="default",
        degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
        dur=0.25,
        amp=Pwhite(0.1, 0.2, seed=1),
    )

# Offline: capture it to samples.
offline = Session.nrt(tempo=2.0)
offline.play(phrase())
samples, frames = offline.render()

# Live: hear the very same phrase.
with Session.live(tempo=2.0, latency=0.1) as live:
    live.play(phrase())
    live.run(3.5)
```

This is exactly what the two shipped examples do — `offline_render.py` and `live_udp.py` share their pattern and differ only in the session factory. See [Examples](examples.md).

## Several sessions at once

Because a session is an ordinary object rather than a global, more than one can be live at the same time. The common case is rendering a score offline (for a plot, an analysis or a `.wav`) right next to a live session you are listening to, in a single script:

```python
live = Session.live(tempo=2.0, latency=0.1)
plot = Session.nrt(tempo=2.0)

live.play(phrase())
plot.play(phrase())

live.run(2.0)                       # heard in real time
samples, frames = plot.render()     # captured offline, no audio device
live.close()
```

The two never interfere: each has its own server, its own clock and its own interface. With globals this is impossible; here it is the default.

## When you don't need a Session

A `Session` is sugar over two objects you can always build yourself. When you want more control — several servers behind one clock, a clock shared across subsystems, or a custom interface — skip the factory and wire them directly:

```python
from clausters.base import TempoClock
from clausters.defs import Server

server = Server("127.0.0.1", 57110, latency=0.1)
clock = TempoClock(tempo=2.0)

phrase().play(clock, server)        # what Session.play does for you
clock.run(3.5)
server.close()
```

`Session` adds no behaviour of its own — it only bundles these two and forwards to them — so reaching for the longer form costs nothing and loses nothing.

## See also

- [API reference](api.md) — the generated reference for `Session` and every method.
- [The client, layer by layer](guide.md) — where the `Server`, the clock and the seam fit in the whole client.
- [Routines and clocks](routines-and-clocks.md) — the level below a session: driving a `Routine`, a `TempoClock` and a `Server` yourself, with sample-accurate timing.
- [Examples](examples.md) — `offline_render.py` and `live_udp.py`, the session in runnable form.
