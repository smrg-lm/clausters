# A DAW-style transport

A DAW has a *transport*: a shared timeline with a tempo and a bar grid. Everything you arm starts aligned to bars, and moving the tempo moves the whole arrangement with it. Clausters offers that same idea, but across **independent clients**: a server hosts one shared beat grid, several clients join it, and a routine on each starts on the same bar. This page is the practical guide to that workflow — being the conductor, joining as a follower, starting together on a bar, and following a tempo change live.

It builds on two other pages. [Timing models](timing-models.md) explains *why* the alignment is beat-accurate or sample-exact (the time reference a clock paces against); this page is the *how* of the transport itself. [Receiving OSC and MIDI](responders.md) is the input layer the live-change section uses.

## The shared grid

The transport is deliberately small: an **origin sample** (the sample position of beat 0) and a **tempo** (beats per second). Together they are a grid — beat `b` is sample `origin + b·rate/tempo` — that the server stores under `/transport_set` and any client can read. That is the whole of it: the server *hosts* the grid but never plays from it. There is no server-side playhead rolling forward; each client's `TempoClock` is its own playhead, and the grid is the common ruler they all measure bars against.

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

With a `Session`, both sides are one call — `session.server.set_transport(...)` to conduct, `session.join_transport()` to follow. `clock.leave_transport()` (or never joining) returns a clock to its own private grid, and `clock.joined` says which of the two it is on.

Joining reads the grid once and keeps it; from then on `clock.grid_beat()` is where the shared grid is now — the clock's own `beats()` when it has not joined one. That is the number `quant` snaps against, and the one to read when computing where a bar falls.

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

- **Plain (wall-clock) followers** align to the **beat**, drift-bounded: the grid's sample origin is mapped to OSC time through the server's `/clock_query` anchor, so everyone agrees on the bar to within the wall-vs-audio drift.
- **Followers whose clock is on the server's sample clock** align to the **sample**: the grid lives on the master's sample axis, so the shared bar is one exact sample for all of them. A live session's clocks are, by default.

```python
clock = TempoClock(timebase=server.sample_timebase())  # sample-exact, drift-free
clock.join_transport(server) # ...then phase-align on the shared bar
```

A clock's timebase is fixed when it is made, so the reference is chosen first, and the clock aligns on it afterwards.

## Following a tempo change live

This is where the transport behaves most like a DAW's: when the conductor changes the tempo (or origin), every follower should move with it. Setting `/transport_set` again **pushes** the new grid to every client registered for notifications, so followers do not have to poll. A follower reacts with an [OSC responder](responders.md) on `/transport_query.reply`:

```python
from clausters.base import OscReceiver
from clausters.responders import OscFunc

recv = OscReceiver().start()
recv.send(server.target.addr(), "/server_notify", 1)   # subscribe on this socket

def follow_transport(msg, time, src):
    # msg == ["/transport_query.reply", origin_sample, tempo, defined]
    if msg[3]:                                   # defined
        clock.join_transport(server)             # re-adopt the new grid

OscFunc(follow_transport, "/transport_query.reply", recv=recv)
```

Now the conductor doing `server.set_transport(0, 3.0)` later in the session re-tempos every follower at once — the bar grid they quantize against moves together. (Register `/server_notify` from the *receiver's* socket, as above, so the push lands where the responder is listening.) The shipped `osc_responder.py` example wires exactly this reaction; see [Examples](examples.md).

## Rolling the transport: a conductor with play / stop / locate

The transport also carries a DAW-style **rolling state** — whether it is playing, and the position — that a conductor drives and every timeline **on that transport** obeys. The server holds the state and owns the time; it plays no notes of its own.

A conductor (any client) drives it through the `Server`:

```python
server.transport_group(group)          # the transport owns the nodes it plays
server.transport_play()                # roll -- every timeline on it rolls
server.transport_locate_sample(sample) # seek the position
server.transport_stop()                # freeze it, and the plans with it
```

A follower puts its timeline on that transport, and that is the whole of following:

```python
timeline.transport = server     # needs a governed group bound
```

From then on `timeline.play`, `pause`, `stop` and `locate` are the transport's own commands, and the timeline **plans** its items onto the transport's clock (`/sched_atTransport`) from the position it is at — so a freeze holds them with the piece and a locate clears the transport queue (`server.sched_clear("transport")`) and re-cues from the new position, `latency` ahead. A locate or a play somebody *else* sent arrives as a broadcast (`/transport_query.reply`, the [responder layer](responders.md)), and the plan is written again from where it says. Every follower reads the **one** position the engine holds, so they are in lockstep by construction rather than by each computing its own. `conductor.py` ([Examples](examples.md)) shows two followers on one transport; `timeline.transport = None` gives it back its own clock.

What the mode refuses, and why: a `quant` (the start is the transport's), a `loop` (the wrap is the engine's, and a timeline's events would have to be re-cued on every one of them), and a **forward-only item** — a routine or a pattern cannot be planned from a position. Those are the client-clock mode's.

## A worked example: two clients, one bar

`sync.py` runs two completely independent client pairs — each its own `Server` and `TempoClock`, the state two separate programs would hold — and lands a note from each on the same bar. The check uses public state only, so any client on the same transport computes the same number:

```python
import math

def next_bar_sample(server, clock, quant=4):
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    target = math.ceil(clock.grid_beat() / quant) * quant
    return round(origin + target * rate / tempo)
```

Sampled back to back, the two clients return the same next-bar sample — that equality *is* the phase alignment. Each then `play(..., quant=4)`s a note, and the two sound together. The example is in [Examples](examples.md).

## What it is, and what it is not

The analogy to a DAW transport is the **bar grid and tempo plus a play/stop/position state** — enough to lock clients to the same bars, tempo, and rolling playhead. The rest of a DAW's transport is intentionally not here:

- **The server owns the time and broadcasts transport *control*; it does not sequence.** It holds the grid, the rolling state and the position, and pushes changes; which note sounds when is each client's own plan, stamped on the transport's clock (`/sched_atTransport`) or on the device's (`/sched_at`) — see [Timelines](timelines.md).
- **One grid per server, last-writer-wins.** There is a single shared transport; whoever calls `set_transport` most recently defines it. Several conductors are a coordination choice you make, not something the server arbitrates. (Multiple independently named transports on one server were considered and deferred.)
- **Tempo and origin only — no meter object.** A "bar" is whatever beat multiple you pass as `quant`; there is no separate time-signature the server stores. Pick a `quant` that matches your meter (4 for 4/4, 3 for 3/4).
- **No server-side recording or arrangement.** The timeline lives in the client; the server holds only the position, the rolling state and the grid, not the notes.

These are the honest edges of a small, composable feature: shared bars, a shared tempo, and a shared play/stop/position that several clients phase-align on, with each client owning its own playhead and arrangement.

## Cheat-sheet

| You want to… | Do this |
| --- | --- |
| Define the shared grid (conductor) | `server.set_transport(origin_sample, tempo)` |
| Read the current grid | `server.transport()` → `(origin_sample, tempo)` or `None` |
| Join the grid (follower) | `clock.join_transport(server)` / `Session.join_transport()` |
| Leave it | `clock.leave_transport()` |
| Start on the next bar | `clock.play(routine, quant=4)` / `session.play(pattern, quant=4)` |
| Align to the sample, not just the beat | a clock on `server.sample_timebase()` — a live session's default (see [Timing models](timing-models.md)) |
| Follow live tempo changes | an `OscFunc("/transport_query.reply", …)` that re-`join_transport`s (see [Receiving OSC and MIDI](responders.md)) |
| Roll a playhead from a conductor | `server.transport_play()` / `transport_stop()` / `transport_locate(beat)`; followers put a timeline on it, `timeline.transport = server` |
| Read the rolling state | `server.transport_state()` → `{tempo, playing, position, …}` |

## Freezing a piece: when the transport governs the sound

Everything above is a transport clients obey **by choice** — a shared grid and a
rolling state the server broadcasts, which each client reads to start on the
same bar. `server.transport_group(group)` changes that. With a group bound, the
transport stops being an advisory and the **engine enforces it**:
`transport_stop()` freezes that group and everything under it at the exact
sample it lands on, and `transport_play()` thaws it.

A frozen node stays in the tree with its internal state untouched — filters keep
their memory, phasors their phase, envelopes their position. So a resume
**continues** the sound rather than starting it again:

```python
piece = Group(server=server)
server.set_transport(0, 2.0)
server.transport_group(piece)
server.transport_play()
...
server.transport_stop()    # the subtree freezes, mid-gesture
...
server.transport_play()    # and carries on from exactly there
```

This matters most for samples the server *generates*. A def running a
stochastic process or a demand-rate sequence has nothing to read and no
messages arriving, so there is no position to seek to — its position **is** its
internal state. Continuing is the only thing a pause can honestly mean for it,
and it is what no DAW transport offers, because a DAW's audio exists before
you press play.

That asymmetry runs through the whole feature:

- **Pause and resume work on anything.** They are a freeze, so they do not care
  what produced the sound.
- **Locate does not.** `transport_locate(beat)` moves the position and never a
  node's state, and locating over a composition holding a resident generator is
  refused instead of moving a cursor the sound will not follow.
  Render the element first and it becomes samples like any other — the change
  of state the [composition](composition.md) chapter is built around.

Anything **scheduled** against a governed node waits out the pause with it and
fires on resume in its right relative place, so a look-ahead already in flight
is not lost. A bundle is atomic, so a bundle holding one governed message and
one live one waits entirely — the one way a message aimed at a live node ends up
waiting.

The clock follows too. `TempoClock.freeze()` holds the beat where it is and
`thaw()` picks it up there, so a client does not run away from a piece that is
not moving; `clausters.gui.PlayheadSync` does this for you, and its `resume()` is
deliberately **not** `play()` — play re-renders from a position, resume
continues the frozen sound.

| You want to… | Do this |
| --- | --- |
| Let the engine enforce the transport | `server.transport_group(group)` |
| Give it back | `server.transport_group(None)` (thaws what it governed) |
| Freeze / carry on | `server.transport_stop()` / `server.transport_play()` |
| Read the piece's own clock | `server.transport_state()["transport_sample"]` |
| Continue rather than restart, client-side | `transport.resume()` (not `play()`) |
| Ask whether a seek means anything | `editor.locatable` |

`freeze.py` in the examples freezes a generative texture and resumes
it, which is the way to hear the difference between continuing and restarting.

## See also

- [Timing models](timing-models.md) — the time reference behind beat-accurate vs sample-exact alignment.
- [Routines and clocks](routines-and-clocks.md) — the playhead (`TempoClock`) and the routines you start on the bar.
- [Receiving OSC and MIDI](responders.md) — the responder layer the live-change reaction uses.
- [Sessions](sessions.md) — the handle that bundles a clock and a server, with `join_transport`.
- [Examples](examples.md) — `sync.py` (two clients on one bar) and `osc_responder.py` (the live transport reaction).
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/transport_set` and `/clock_query` on the wire.
