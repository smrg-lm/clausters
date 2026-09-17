# Timing models

A clock can keep time in a few different ways — *models* — and this page is the one place to understand and **try** each: what it is, how to make a clock on it, and how to watch its timing from Python. The choice of model is independent of *where* the OSC goes. [Routines and clocks](routines-and-clocks.md) is the companion page: it builds the routines you play on these clocks.

| Model | In one line | How to select it |
| --- | --- | --- |
| Wall-clock OSC time | the client's own clock; works everywhere, including with no server | a bare `TempoClock(tempo)` |
| Sample clock | paces on a server's sample counter; drift-free, sample-exact | `TempoClock(timebase=server.sample_timebase())` — **and the default for `Session.live()` and `Session.embed()`** |
| Shared transport | a server-hosted beat grid several clients align on | `clock.join_transport(server)` + `quant` |

All three ride **logical time** — the jitter-free relative timing a routine's `yield`s define (see [Routines and clocks](routines-and-clocks.md)). They differ in the *reference* the clock paces against and how it addresses events on the wire.

**A clock's timebase is fixed when it is made, and a session sets it.** A
`TempoClock` made while a session is active is made on **the session's
timebase** and kept in the session; one made with no session active paces on
wall-clock time (below) unless it is given a timebase. A `Session.live()` or
`Session.embed()` is on **the server's sample clock by default** (the next
section) — `live` by tracking it over UDP, `embed` by reading the in-process
counter directly (no tracker, no round trips). That default is set by the
config key `[client].clock` (`"sample"` by default, `"monotonic"` to opt out),
or per session by passing `timebase=`. An offline session (`Session.nrt()`) is
on a `LogicalTimebase`, and it is the only time it can have.

A clock never changes mode: rendering one that is not on logical time raises,
and so does making one on another timebase than the active session's. The
reason is its queue — every beat already scheduled is measured against the time
the clock was made on.

## Wall-clock OSC time

A plain clock paces against wall-clock **OSC time** (OSC timetags are NTP: absolute seconds since 1900). You get it from a clock made with no session active, or from a session made on it:

```python
clock = TempoClock(tempo=2.0)                        # no session active
session = Session.live(host, port, timebase=MonotonicTimebase())  # or: [client].clock = "monotonic"
```

- **Self-contained.** It is the client's own clock; across machines you can discipline it with NTP/PTP, but nothing here depends on a Clausters server.
- **Works anywhere** — standalone, against another OSC program, or across a network.
- **Jitter-free *relative* timing.** Logical time is exact, so events keep their spacing even though the routine wakes at slightly irregular physical instants. The routine's *start* is arbitrary (wall-clock), exactly as in SuperCollider; the guarantee is no jitter *between* events, like MIDI.
- Absolute alignment across machines is **NTP/PTP-quality**, not sample-exact.

This is the model for a bare clock, or when driving something other than a Clausters server (another OSC program, a remote peer). Nothing to test beyond playing a routine — it just sounds.

## Sample clock — drift-free, on a master (the session default)

A clock made on a Clausters server's sample clock schedules on the **server's own sample counter** (via `/sched_at`, by absolute sample), which removes the drift between the client's clock and the audio device. A `Session.live()` is made on it **by default**, and so is every clock made while that session is active, so a local live session is drift-free out of the box; a clock made with no session asks the server for the timebase:

```python
session = Session.live(host, port)   # on the sample clock (config [client].clock = "sample")
clock = TempoClock(timebase=server.sample_timebase())   # no session: ask for it
```

- The server becomes the **master clock**. Over UDP the client tracks the server's published `/clock_query` anchor on its own socket; with an in-process or shared-memory server it reads the counter directly.
- **Drift-free and sample-coherent.** Events land on exact samples, and several clients locked to the same master share one sample axis.
- **It raises when no master answers.** A clock's timebase is fixed when it is made, so there is nothing to fall back to afterwards: `sample_timebase` (and a `Session.live()` left on its default) raises and says why, and a session that has to run with no server is made with `timebase=MonotonicTimebase()`. An offline server has no sample clock at all.

### Watching it, from Python

The point of this model is real, drift-free timing, so it is worth *seeing* it. The sample-clock tracker reads the server's live position from real `/clock_query` replies; everything below is plain Python, with a server running (the installed `clausters` command). Build the tracker explicitly so you can read it, and hand its timebase to the clock:

```python
sc = server.sample_clock()                  # a tracker on its own socket
sc.warmup(); sc.track()                      # seed and keep the model fresh
clock = TempoClock(tempo=2.0, timebase=sc.timebase())   # what sample_timebase does, with a handle

print("rate:", sc.rate, "Hz | drift:", f"{sc.model.drift_ppm():.1f} ppm")
before = sc.now()                            # the server's sample counter, now
clock.run(3.0)                               # play something for 3 seconds
after = sc.now()
print(f"counter advanced {after - before} samples = {(after - before) / sc.rate:.3f} s")
```

`sc.now()` is the server's real sample counter (the model is fit from live round trips, not guessed), `sc.rate` is its measured sample rate, and `sc.model.drift_ppm()` is the actual measured difference between the two clocks. To verify the lock: the advance should match the `3.0` seconds you ran the clock to within the tracker's small uncertainty, and `drift_ppm` should be a handful of ppm, not hundreds. (The server can also print the exact sample of each scheduled event at trace level — enable it from Python with `server.request("/server_verbosity", "clausters::osc=trace", expect=("/done",))` and read the server's own terminal — but the client-side reading above needs nothing but Python.)

`server.sample_timebase()` is the same in one call when you do not need the tracker handle; it raises if no master answers.

## Shared transport — phase-aligning several clients

> This section is that timing model in brief; [A DAW-style transport](transport.md) is the full workflow guide — conducting, following, starting together on a bar, and following a tempo change live.

Locking to a master gives every client the same sample axis, but each routine still *starts* whenever you play it. To make several clients begin on the **same beat**, two pieces work together:

- **`quant`** — `clock.play(routine, quant=4)` (or `session.play(pattern, quant=4)`) snaps the routine's start to the next beat that is a multiple of `quant` (a bar in 4/4). `None` or `0` starts immediately. On its own it snaps to the clock's own grid — handy for one client adding a voice cleanly on the next bar.
- **A shared transport** — `clock.join_transport(server)` (or `Session.join_transport()`) adopts the server's `/transport_set` grid: its tempo and an origin every client shares. Now `quant` snaps to *that* grid, so every client on it hits the same bar. One client (the conductor) defines it with `server.set_transport(origin_sample, tempo)`; the others join.

With each client's clock also made on the master's sample clock, the shared bar is an exact sample, so the clients are sample-aligned; in plain wall-clock mode they are beat-aligned (drift-bounded, via the server's OSC-time anchor). Start the clock before playing a quantized routine, so `quant` snaps against the running grid.

### Trying it

The `sync.py` example (see [Examples](examples.md)) sets a transport, has two independent clients join it on the sample clock, and shows them landing on the same bar. The check is that both compute the *same* next-bar sample — using only public state, so any client on the same transport gets the same number:

```python
import math

def next_bar_sample(server, clock, quant=4):
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    target = math.ceil(clock.grid_beat() / quant) * quant
    return round(origin + target * rate / tempo)
```

Run it for two clients sampled back-to-back and the two values match to the sample.

## Reference is independent of destination

The time model is **orthogonal to the destination** — where the OSC actually goes (any OSC endpoint, a local or remote server). The one hard rule is that the sample clock and the transport need a Clausters master; everything else runs on wall-clock time.

| You are talking to… | Model | How |
| --- | --- | --- |
| nothing / another OSC program | wall-clock OSC time | a bare `TempoClock`, or `[client].clock = "monotonic"` |
| a remote server across a network | wall-clock OSC time | `[client].clock = "monotonic"` (NTP/PTP-quality sync) |
| a local / LAN Clausters server | sample clock | `Session.live()` default; `server.sample_timebase()` for a clock with no session |
| several clients, one server | sample clock + transport | the session default, then `join_transport` |

A `Session` is on the sample clock by default and raises if no master answers, so the "local server" row is the zero-config case. The `"monotonic"` rows are the opt-out for when you *want* wall-clock — driving a remote or non-Clausters peer.

## MIDI always rides OSC time

MIDI output never uses the sample clock. A `MidiServer` writing a score keeps its timeline in beats (logical/OSC time) and quantizes to ticks only when it writes the file; live MIDI output is emitted on the clock's logical time. The sample clock changes only how the *OSC* `Server` schedules; it does not touch MIDI timing — MIDI is not sample-exact by design, and the client may have no sample clock at all. Live OS MIDI output is therefore best-effort; for exact MIDI timing, write a score offline (its ticks come from logical time). Tighter live-MIDI timing is a possible future refinement.

## The API, at a glance

- `Server.sample_timebase()` — the server's sample clock as a timebase to make a clock on (`TempoClock(timebase=...)`); `Session.live()`/`embed()` use it by default. Blocking over UDP (it does `/clock_query` round trips; instant on an embedded server); call before `start`/`run`, never from a routine.
- `Session.timebase` — the time every clock of a session is made on.
- `TempoClock.join_transport(server)` / `leave_transport()` — adopt / drop the server's shared transport grid. `Session.join_transport()` is the wrapper.
- `Server.set_transport(origin_sample, tempo)` / `Server.transport()` — define / read the shared grid (the conductor sets it once).
- `play(routine, quant=...)` — start on the next `quant`-beat boundary of the current grid.

## See also

- [Routines and clocks](routines-and-clocks.md) — writing the routines you play on these clocks.
- [Sessions](sessions.md) — the handle that bundles a clock and a server.
- [API reference](api.md) — `Server.sample_timebase`, `TempoClock.join_transport`, `Session`, `Server`.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/clock_query` (the master-clock anchor), `/sched_at` and `/transport_set`.
