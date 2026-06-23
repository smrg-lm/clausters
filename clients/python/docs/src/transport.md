# A DAW-style transport

A DAW has a *transport*: a shared timeline with a tempo and a bar grid. Everything you arm starts aligned to bars, and moving the tempo moves the whole arrangement with it. Clausters offers that same idea, but across **independent clients**: a server hosts one shared beat grid, several clients join it, and a routine on each starts on the same bar. This page is the practical guide to that workflow — being the conductor, joining as a follower, starting together on a bar, and following a tempo change live.

It builds on two other pages. [Timing models](timing-models.md) explains *why* the alignment is beat-accurate or sample-exact (the time reference a clock paces against); this page is the *how* of the transport itself. [Receiving OSC and MIDI](responders.md) is the input layer the live-change section uses.

## The shared grid

The transport is deliberately small: an **origin sample** (the sample position of beat 0) and a **tempo** (beats per second). Together they are a grid — beat `b` is sample `origin + b·rate/tempo` — that the server stores under `/transport` and any client can read. That is the whole of it: the server *hosts* the grid but never plays from it. There is no server-side playhead rolling forward; each client's `TempoClock` is its own playhead, and the grid is the common ruler they all measure bars against.

One client is the **conductor**: it defines the grid once.

```python
from clausters.defs import Server

server = Server()                       # UDP to 127.0.0.1:57110
server.set_transport(origin_sample=0, tempo=2.0)   # beat 0 at sample 0, 2 beats/s
```

Every other client is a **follower**: it adopts that grid as its own tempo and origin.

```python
from clausters.base import TempoClock

clock = TempoClock()                    # its own tempo is about to be overwritten
clock.join_transport(server)            # adopt the shared tempo + origin
```

With a `Session`, both sides are one call — `session.server.set_transport(...)` to conduct, `session.join_transport()` to follow. `clock.leave_transport()` (or never joining) returns a clock to its own private grid.

## Starting together on a bar

A DAW starts a clip on the next bar, not the instant you click. The client's equivalent is `quant`: the beat boundary a routine's start snaps to.

```python
clock.start()                                   # the playhead must be running first
clock.play(routine, quant=4)                    # start on the next 4-beat bar
session.play(pattern, quant=4)                  # the Session form
```

`quant=4` snaps the start to the next beat that is a multiple of 4 — a bar in 4/4. `quant=1` is the next beat; `None` or `0` starts immediately. Because every follower's `quant` snaps to the **same shared grid**, they all land on the same bar, so independent clients begin in phase. Start the clock *before* playing the quantized routine, so `quant` measures against the running grid rather than a stopped one.

`quant` works without a transport too — then it snaps to the clock's own elapsed beats, which is the clean way for a single client to drop a new voice in on the next bar. Joining a transport is what makes that bar the *same* bar across clients.

## Beat-accurate or sample-exact

How tightly the clients align depends on the time reference each clock paces against — the subject of [Timing models](timing-models.md), in one paragraph here:

- **Plain (wall-clock) followers** align to the **beat**, drift-bounded: the grid's sample origin is mapped to OSC time through the server's `/clock` anchor, so everyone agrees on the bar to within the wall-vs-audio drift.
- **Followers that also `lock_to(server)`** align to the **sample**: the grid lives on the master's sample axis, so the shared bar is one exact sample for all of them.

```python
clock.lock_to(server)        # sample-exact timing (drift-free; the master clock)
clock.join_transport(server) # ...then phase-align on the shared bar
```

Order does not matter much, but lock first and join second reads well: choose the reference, then align on it.

## Following a tempo change live

This is where the transport behaves most like a DAW's: when the conductor changes the tempo (or origin), every follower should move with it. Setting `/transport` again **pushes** the new grid to every client registered for notifications, so followers do not have to poll. A follower reacts with an [OSC responder](responders.md) on `/transport.reply`:

```python
from clausters.base import OscReceiver
from clausters.responders import OscFunc

recv = OscReceiver().start()
recv.send(server.target.addr(), "/notify", 1)   # subscribe on this socket

def follow_transport(msg, time, src):
    # msg == ["/transport.reply", origin_sample, tempo, defined]
    if msg[3]:                                   # defined
        clock.join_transport(server)             # re-adopt the new grid

OscFunc(follow_transport, "/transport.reply", recv=recv)
```

Now the conductor doing `server.set_transport(0, 3.0)` later in the session re-tempos every follower at once — the bar grid they quantize against moves together. (Register `/notify` from the *receiver's* socket, as above, so the push lands where the responder is listening.) The shipped `osc_responder.py` example wires exactly this reaction; see [Examples](examples.md).

## A worked example: two clients, one bar

`transport_sync.py` runs two completely independent client pairs — each its own `Server` and `TempoClock`, the state two separate programs would hold — and lands a note from each on the same bar. The check uses public state only, so any client on the same transport computes the same number:

```python
import math

def next_bar_sample(server, clock, quant=4):
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    grid_beat = (clock.timebase.current_sample() - origin) * tempo / rate
    target = math.ceil(grid_beat / quant) * quant
    return round(origin + target * rate / tempo)
```

Sampled back to back, the two clients return the same next-bar sample — that equality *is* the phase alignment. Each then `play(..., quant=4)`s a note, and the two sound together. The example is in [Examples](examples.md).

## What it is, and what it is not

The analogy to a DAW transport is the **bar grid and tempo** — the part that makes clients lock to the same bars and the same tempo. The rest of a DAW's transport is intentionally not here:

- **No *server-side* global play/stop or song position.** The server does not roll a playhead. "Pressing play" is starting *your* clock with a `quant` onto the shared bar; stopping is stopping your clock. Each client owns its own transport-rolling — a client-side `Playhead` over a static `Timeline` gives you actual play/stop/locate/loop and a song position locally (see [Timelines and the playhead](timelines.md)); a single conductor driving every client's playhead in lockstep is a layer that can be added on top later.
- **One grid per server, last-writer-wins.** There is a single shared transport; whoever calls `set_transport` most recently defines it. Several conductors are a coordination choice you make, not something the server arbitrates. (Multiple independently named transports on one server were considered and deferred.)
- **Tempo and origin only — no meter object.** A "bar" is whatever beat multiple you pass as `quant`; there is no separate time-signature the server stores. Pick a `quant` that matches your meter (4 for 4/4, 3 for 3/4).
- **The grid is a ruler, not a schedule.** The server stores and serves it but never schedules from it; all timing still comes from your clock's logical time (see [Routines and clocks](routines-and-clocks.md)).

These are the honest edges of a small, composable feature: it gives you shared bars and a shared tempo that several clients phase-align on, and leaves the playhead in each client's hands.

## Cheat-sheet

| You want to… | Do this |
| --- | --- |
| Define the shared grid (conductor) | `server.set_transport(origin_sample, tempo)` |
| Read the current grid | `server.transport()` → `(origin_sample, tempo)` or `None` |
| Join the grid (follower) | `clock.join_transport(server)` / `Session.join_transport()` |
| Leave it | `clock.leave_transport()` |
| Start on the next bar | `clock.play(routine, quant=4)` / `session.play(pattern, quant=4)` |
| Align to the sample, not just the beat | `clock.lock_to(server)` as well (see [Timing models](timing-models.md)) |
| Follow live tempo changes | an `OscFunc("/transport.reply", …)` that re-`join_transport`s (see [Receiving OSC and MIDI](responders.md)) |

## See also

- [Timing models](timing-models.md) — the time reference behind beat-accurate vs sample-exact alignment.
- [Routines and clocks](routines-and-clocks.md) — the playhead (`TempoClock`) and the routines you start on the bar.
- [Receiving OSC and MIDI](responders.md) — the responder layer the live-change reaction uses.
- [Sessions](sessions.md) — the handle that bundles a clock and a server, with `join_transport`.
- [Examples](examples.md) — `transport_sync.py` (two clients on one bar) and `osc_responder.py` (the live transport reaction).
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/transport` and `/clock` on the wire.
