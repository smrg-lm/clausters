# Timelines and the playhead

The sequencing you have seen so far is **generative**: a `Routine` is a Python generator, a `Pbind` an event pattern, and a `TempoClock` resumes them forward in time. That model is open-ended and expressive, but it has one thing it fundamentally cannot do — **seek**. A generator's musical state lives in its local variables, so you cannot jump it to beat 100 without running through 0–100, and you cannot ask "what plays at bar 33?" without getting there.

A `Timeline` is the complement: a **static, editable list of timed items, kept sorted by beat, with random access by time**. Because it is a data structure rather than a coroutine, a `Playhead` can give it real **transport controls** — play, stop, locate (seek), loop — and a song position. This is the DAW model: the arrangement is random-access for editing and seeking, and playback is a forward scan of it from the playhead.

This page is the static counterpart to [Routines and clocks](routines-and-clocks.md); the two models coexist, and you can move between them (capture a pattern into a timeline, below).

## The timeline

A `Timeline` holds `(beat, item)` pairs in beat order. You edit it freely and query it by time.

```python
from clausters.seq import Timeline, Event

tl = Timeline()
a = tl.add(0.0, Event(degree=0))     # add returns a handle...
b = tl.add(1.0, Event(degree=2))
tl.add(2.0, Event(degree=4))

tl.move(a, 4.0)                      # ...you pass back to move or remove
tl.remove(b)
```

`add` keeps the list sorted (a stable insert, so items added at the same beat keep their order — a note-off before a re-trigger). It returns a handle you give back to `remove` and `move`, so edits stay correct as other inserts shift positions.

The **random access by time** is the point:

```python
tl.index_at(1.5)      # the cursor of the first item at or after beat 1.5
tl.range(1.0, 3.0)    # the (beat, item) pairs in the half-open window [1.0, 3.0)
tl.at(2.0)            # the items exactly at beat 2.0
tl.duration()         # the beat of the last item
```

`index_at` is the seek primitive — it is what `play(at=…)` and `locate` use to start the scan at an arbitrary point, which a forward-only routine could never do.

### What an item is

An *item* is anything that can realize itself on a destination — it has a `play(destination)` method. `Event` already is one (it plays a note on a `Server` for OSC, or a `MidiServer` for MIDI — the same double dispatch the patterns use), so a timeline of `Event`s renders to OSC *or* MIDI depending only on the destination the playhead holds. For a plain editable OSC or MIDI score, `OscEvent` and `MidiEvent` wrap a raw message:

```python
from clausters.seq import OscEvent, MidiEvent

tl.add(0.0, OscEvent("/s_new", "default", -1, 0, 0, "freq", 440.0))
tl.add(1.0, MidiEvent(b"\x90\x3c\x64"))     # note on, key 60, vel 100
```

## The playhead

A `Playhead` scans a timeline forward as a clock advances, realizing each item on a destination. It is built from a timeline, the clock that drives it, and the destination the items go to:

```python
from clausters.seq import Playhead

head = Playhead(timeline, session.clock, session.server)
session.start()                 # the clock must be running for live playback
head.play(at=0.0, quant=4)      # start on the next bar, from the top
```

The transport controls:

| Call | What it does |
| --- | --- |
| `play(at=0.0, quant=None)` | Start (or restart) from beat `at`, snapping to a `quant` bar. Re-seeks the cursor, so it doubles as locate-and-play. |
| `stop()` | Halt the playhead; no further items are realized (notes already started keep their scheduled releases). |
| `locate(beat)` | Seek to `beat` — random access. While playing, restarts the scan there; while stopped, sets where the next `play` begins. |
| `loop(start, end)` / `unloop()` | Loop the half-open window `[start, end)`; the scan wraps at `end`. |
| `position()` | The current song position in beats (interpolated from the clock while playing). |

Under the hood the playhead is a thin cursor over the static structure: the random access happens at the boundaries (`play`, `locate`, loop wrap), and between them it is a forward scan — exactly how a DAW's playback engine reads its arrangement. Because it rides the clock's logical time like everything else in the client, it **inherits the timing models for free**: `quant` starts it on a bar, `clock.lock_to(server)` makes its events sample-exact, and `clock.join_transport(server)` aligns its bars with other clients (see [Timing models](timing-models.md) and [A DAW-style transport](transport.md)).

## Capturing a pattern into a timeline

The two models meet here: run a pattern offline and record what it plays into a timeline — "bounce a pattern to a clip" — then edit and seek the result.

```python
from clausters.seq import Timeline, Pbind, Pseq

tl = Timeline.from_pattern(
    Pbind(instrument="default", degree=Pseq([0, 2, 4, 7]), dur=0.5),
    dur=2.0,      # bound an open-ended pattern; None drains a finite one fully
)
tl.add(0.0, Event(instrument="default", degree=7, dur=0.5, amp=0.3))   # then edit
```

## Offline rendering

A playhead is destination-agnostic, so rendering a timeline offline is the same code with an offline session: play it on the NRT clock and render the score.

```python
from clausters import Session

session = Session.nrt(tempo=2.0)
Playhead(timeline, session.clock, session.server).play()
session.clock.render()                       # drain the playhead in logical time
samples, frames = session.server.render()    # the offline render
```

## Following a conductor

A playhead is a local transport, but it can also obey a **shared** one. `head.follow_transport(server)` binds it to the server's transport so that a conductor's `transport_play` / `transport_stop` / `transport_locate` rolls, halts and seeks *this* playhead too — several clients in lockstep. It is built on the responder layer (an `OscFunc` on the transport broadcast) and the shared grid:

```python
head = Playhead(timeline, clock, server)
clock.start()
head.follow_transport(server, quant=4)   # roll when the conductor presses play
```

Beat-aligned in plain wall-clock mode, sample-exact when the clock is also `lock_to` the server. See [A DAW-style transport](transport.md) for the conductor side (`Server.transport_play` and friends) and `transport_conductor.py` in [Examples](examples.md).

## See also

- [Routines and clocks](routines-and-clocks.md) — the generative counterpart (the open-ended model you can capture *from*).
- [A DAW-style transport](transport.md) — the shared beat grid clients phase-align on.
- [Timing models](timing-models.md) — the timing references a playhead inherits (`quant`, `lock_to`, `join_transport`).
- [Examples](examples.md) — `timeline_transport.py`, the playhead live.
- [API reference](api.md) — `Timeline`, `Playhead`, `OscEvent`, `MidiEvent`.
