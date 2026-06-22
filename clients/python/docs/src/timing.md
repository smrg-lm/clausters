# Timing references

Every event you play needs a time. The client offers **two time references**, and which one you use is a choice independent of *where* the OSC goes. The default works everywhere — including with no Clausters server, driving some other program — while the other locks to a server's sample clock for drift-free, sample-accurate timing. This page is the map; [Routines and clocks](routines-and-clocks.md) is the hands-on version.

## The two references

### Wall-clock OSC time — the default

A plain clock paces against wall-clock **OSC time** (OSC timetags are NTP: absolute seconds since 1900). You get it by doing nothing special:

```python
clock = TempoClock(tempo=2.0)        # or: Session.live(host, port)
```

- **Self-contained.** It is the client's own clock; across machines you can discipline it with NTP/PTP, but nothing here depends on a Clausters server.
- **Works anywhere** — standalone, against another OSC program, or across a network.
- **Jitter-free *relative* timing.** Logical time (the sum of a routine's `yield`s) is exact, so events keep their spacing even though the routine wakes at slightly irregular physical instants. The routine's *start* is arbitrary (wall-clock), exactly as in SuperCollider; the guarantee is no jitter *between* events, like MIDI.
- Absolute alignment across machines is **NTP/PTP-quality**, not sample-exact.

### The server's sample clock — opt-in, needs a master

Locking the clock to a Clausters server makes it schedule on the **server's own sample counter** (via `/sched`, by absolute sample), which removes the drift between the client's clock and the audio device:

```python
clock.lock_to(server)                # or: Session.live(host, port).lock_to_server()
```

- The server becomes the **master clock**. Over UDP the client tracks the server's published `/clock` anchor on its own socket; with an in-process or shared-memory server it reads the counter directly.
- **Drift-free and sample-coherent.** Events land on exact samples, and **several clients locked to the same master share one sample axis**, so their timing is mutually coherent.
- **Requires a reachable master.** If there is none — an offline render, or simply no server running — `lock_to` leaves the clock on wall-clock time instead of failing, so a client with no Clausters server keeps working.

## Reference is independent of destination

The time reference is **orthogonal to the destination** (where the OSC actually goes — any OSC endpoint, a local or remote server). The one hard rule is that the sample clock needs a Clausters master; everything else falls back to wall-clock.

| You are talking to… | Reference | How |
| --- | --- | --- |
| nothing / another OSC program | wall-clock OSC time | the default — do nothing |
| a remote server across a network | wall-clock OSC time | the default (NTP/PTP-quality sync) |
| a local / LAN Clausters server | sample clock | `lock_to` (drift-free, the master) |
| one server, several clients | sample clock | each client `lock_to` the same master |

## The lock API

- `TempoClock.lock_to(server)` — switch this clock to the server's sample clock; returns `self`. Releases with `unlock()`, or `close()` (which also stops the clock).
- `Session.lock_to_server()` — the session locks its own clock to its own server; returns `self`, so it chains: `Session.live(...).lock_to_server()`.

`lock_to` is **blocking** (it does `/clock` round trips to find and measure the master), so call it before `start`/`run` and **never from inside a routine**. It is also safe to call when no master answers: it just stays on wall-clock time.

## MIDI always rides OSC time

MIDI output never uses the sample clock. A `MidiServer` writing a score keeps its timeline in beats (logical/OSC time) and quantizes to ticks only when it writes the file; live MIDI output is emitted on the clock's logical time. `lock_to` changes only how the *OSC* `Server` schedules; it does not touch MIDI timing — MIDI is not sample-exact by design, and the client may have no sample clock at all. (Jitter-free MIDI *delivery* through hardware timestamps is a separate, future refinement.)

## See also

- [Routines and clocks](routines-and-clocks.md) — driving a routine and logging the timing, with `lock_to` in practice.
- [Sessions](sessions.md) — the handle that bundles a clock and a server.
- [API reference](api.md) — `TempoClock.lock_to`, `Session.lock_to_server`.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/clock` (the master-clock anchor) and `/sched`.
