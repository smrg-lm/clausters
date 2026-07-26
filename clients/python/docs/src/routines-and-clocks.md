# Routines and clocks

`Session` and `Pbind` are convenience layers over three plain objects you can also drive yourself:

- a **`Routine`** — *what* happens over time, written as a Python generator that `yield`s how long to wait;
- a **`TempoClock`** — *when* it happens: it schedules the routine and keeps musical time in beats;
- a **`Server`** — *where* the sound goes: it owns the communication interface and plays events on it.

This page works at that level: write a routine by hand and play notes from it through a `Server`, on the high-level API throughout — you build `Event`s and call `play`, never hand-assemble OSC bundles. For the different ways a clock can keep time — the default wall clock, locked to a server's sample clock, or a shared transport — and how to observe each, see [Timing models](timing-models.md).

## Logical time vs physical time

This is the idea that makes routines worth using, inherited from SuperCollider.

A routine `yield`s numbers, and each one is a wait *in beats* before the clock resumes it. Those waits play out in **physical time** — the actual wall-clock seconds the OS sleeps — and under load they jitter. But a routine also keeps a **logical time**: the running sum of everything it has yielded so far, relative to when it started and the clock's tempo. The logical time has no jitter — a routine that yields `0.5` four times is at logical beats `0, 0.5, 1.0, 1.5` exactly, whatever the scheduler did in between.

The Server stamps every event from the routine's **logical** time, not from "now". So even though the routine is woken at slightly irregular physical instants, the timing it asks the server for is precise. That is the only way to get jitter-free rhythmic sequences in real time, and every timing model builds on it.

## A routine by hand

An `Event` is a dict of note parameters that knows how to play itself: `Event(...).play(server)` creates a synth at the routine's current logical beat and schedules its release after the note's sustain. Build it with keywords or from a plain dict — an `Event` *is* a dict — and play it from inside a routine, yielding the gap to the next note:

```python
from clausters.base import TempoClock, Routine
from clausters.seq import Event
from clausters.defs import Server

server = Server("127.0.0.1", 57110, latency=0.1)   # a running server, on UDP
clock = TempoClock(tempo=2.0)                       # 2 beats per second

def melody():
    for note in [60, 62, 64, 67, 69]:               # MIDI notes
        e = Event(midinote=note, amp=0.2, dur=0.5)   # half-beat note
        e.play(server)
        yield e.delta()                              # advance to the next note

clock.play(Routine(melody))                          # schedule it to start now
clock.run(3.0)                                       # advance the clock 3 s, then stop
server.close()
```

A few things worth knowing:

- An `Event` carries musical defaults (see the API reference): `midinote` (or `degree`, or an explicit `freq`) sets pitch, `amp` the level, `instrument` the def (the server has a built-in `default` sine). Timing comes from `dur`: the note's `delta` (beats to the next event) is `dur`, and its `sustain` (how long it sounds) is `dur * legato` (`legato` defaults to `0.8`). As in SuperCollider, an explicit `delta` or `sustain` key overrides that calculation — `Event(..., dur=0.5, sustain=0.4)` sounds for exactly 0.4 beats. A dict works just as well — `Event({"midinote": 60, "amp": 0.2, "dur": 0.5}).play(server)`.
- `clock.run(seconds)` starts the real-time driver, waits, and stops it. Use `clock.start()` / `clock.stop()` to keep one clock running across several routines.
- A routine optionally receives the clock as its argument (`def melody(clock):`) if it needs it, but for playing events you rarely do — the Server finds the logical beat itself.
- This clock paces against wall-clock OSC time, the default. To make the same routine drift-free and sample-accurate, or to phase-align several clients, lock it to the server — see [Timing models](timing-models.md).

### The one rule

**A routine must never block the clock thread.** It runs *on* that thread, so a `time.sleep`, a blocking `server.sync()`, or any `wait=True` def send freezes every other routine and the whole timeline. Cede time with `yield` instead. To load a def from within a routine, send it asynchronously — `server.add_synthdef(sdef, wait=False)` — and `yield` enough time before the first note that uses it.

## The random context: one seed per session

Everything random — `Pwhite`, `Prand`, and the module functions
`clausters.next_f64()` / `uniform(lo, hi)` / `next_below(n)` / `choice(items)`
— draws from **one seedable context**, the sclang model. The context is the
**session**: each has its own root, so each reproduces independently.

- `session.seed(n)` seeds *that* session's root. Called before you build and
  play, it makes every random value in the session reproducible, end to end;
  `main.seed(n)` does the same for the **default session** (free-standing
  draws and anything played without a `Session`). Seeding one session never
  perturbs another.
- Every routine gets its **own** generator when it is created, seeded from the
  context creating it (its session's root). Same root seed + same creation
  order = the same music; and because each routine draws from its own stream,
  concurrent routines (several clocks, live RT next to an NRT render) stay
  reproducible per routine no matter how their wakes interleave.
- A draw always uses the generator of the routine running right now; outside a
  routine it uses the active session's root — the session driving on this
  thread (or entered with ``with``), else the default session.

There are **no per-pattern seeds** — independent seeds would break
per-session consistency. To isolate some material, play it inside its own
routine (its own derived stream by construction) or its own session. The
generator itself lives in the shared native core, so the same seed replays the
same values in every Clausters client language.

```python
from clausters import Session, uniform
from clausters.seq import Pbind, Pwhite

with Session.nrt(tempo=2.0) as session:
    session.seed(2026)               # this session is now reproducible
    base = uniform(-3.0, 3.0)        # a one-off draw from the session's root
    session.play(Pbind(freq=Pwhite(400.0, 800.0), dur=0.25))   # draws when played
    session.render()
```

## Offline, with the same code

Everything here works unchanged offline. Build the `Server` with an `OscNrtInterface`, drive the clock with `clock.render()` instead of `run()`, and the routine's `play` calls accumulate a timed score the bundled renderer turns into samples — no server, no audio device. That swap is the client's central seam, covered in [Sessions](sessions.md) and [The client, layer by layer](guide.md).

Because it is the same interface, one distinction matters as much offline as live: **a message has no time; a bundle does.** In a bundle a message would carry the *immediate* timetag, and on its own it means exactly that — so `server.synth(…)`, `server.set(…)`, `server.free(…)` are untimed wherever you call them. Reach for them for what has no place in a timeline: sending defs, allocating buffers, opening the groups a piece is built on.

Placing something *in time* is the other path: **`send_bundle`** stamps the beat the routine has accumulated by yielding (plus an optional `delay_beats=` lookahead), and an `Event` — so every **pattern** — does it for you. Creating a node with `send_msg` from inside a routine is therefore an error, not a thing that renders differently: live you cannot see it, because "immediately" and the logical beat are close enough to pass; offline it is the difference between a piece and a chord.

## See also

- [Timing models](timing-models.md) — the ways a clock keeps time (wall-clock, sample-locked, shared transport) and how to observe each.
- [Sessions](sessions.md) — the ergonomic handle that bundles a clock and a server, for when you do not need this level of control.
- [The client, layer by layer](guide.md) — where routines, clocks and the server sit in the whole client.
- [API reference](api.md) — `TempoClock`, `Routine`, `Event` and the `Server` methods used here.
