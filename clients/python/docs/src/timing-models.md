# Timing models

A clock can keep time in a few different ways — *models* — and this page is the one place to understand and **try** each: what it is, how to switch a clock to it, and how to watch its timing from Python. The choice of model is independent of *where* the OSC goes. [Routines and clocks](routines-and-clocks.md) is the companion page: it builds the routines you play on these clocks.

| Model | In one line | How to select it |
| --- | --- | --- |
| Wall-clock OSC time | the client's own clock; works everywhere, including with no server | a bare `TempoClock(tempo)` |
| Sample clock | locks to a server's sample counter; drift-free, sample-exact | `clock.lock_to(server)` — **and the default for `Session.live()`** |
| Shared transport | a server-hosted beat grid several clients align on | `clock.join_transport(server)` + `quant` |

All three ride **logical time** — the jitter-free relative timing a routine's `yield`s define (see [Routines and clocks](routines-and-clocks.md)). They differ in the *reference* the clock paces against and how it addresses events on the wire.

**Which default you get depends on how you build the clock.** A bare
`TempoClock(tempo)` paces on wall-clock time (below); a `Session.live()`
**anchors to the server's sample clock by default** (the next section), because
it has a networked server to lock to. That default is set by the config key
`[client].clock` (`"sample"` by default, `"monotonic"` to opt out), or per call
by passing an explicit `timebase=`. (`Session.embed()` stays on wall-clock: its
in-process server exposes no endpoint for the sample-clock tracker.)

## Wall-clock OSC time

A plain clock paces against wall-clock **OSC time** (OSC timetags are NTP: absolute seconds since 1900). You get it from a bare clock, or from a session told not to lock:

```python
clock = TempoClock(tempo=2.0)                        # a hand-built clock
session = Session.live(host, port, timebase=MonotonicTimebase())  # or: [client].clock = "monotonic"
```

- **Self-contained.** It is the client's own clock; across machines you can discipline it with NTP/PTP, but nothing here depends on a Clausters server.
- **Works anywhere** — standalone, against another OSC program, or across a network.
- **Jitter-free *relative* timing.** Logical time is exact, so events keep their spacing even though the routine wakes at slightly irregular physical instants. The routine's *start* is arbitrary (wall-clock), exactly as in SuperCollider; the guarantee is no jitter *between* events, like MIDI.
- Absolute alignment across machines is **NTP/PTP-quality**, not sample-exact.

This is the model for a bare clock, or when driving something other than a Clausters server (another OSC program, a remote peer). Nothing to test beyond playing a routine — it just sounds.

## Sample clock — drift-free, locked to a master (the session default)

Locking the clock to a Clausters server makes it schedule on the **server's own sample counter** (via `/sched`, by absolute sample), which removes the drift between the client's clock and the audio device. A `Session.live()` does this **for you by default**, so a local live session is drift-free out of the box; a bare clock opts in with one call:

```python
session = Session.live(host, port)   # already sample-locked (config [client].clock = "sample")
clock.lock_to(server)                # a hand-built clock: lock it explicitly
```

- The server becomes the **master clock**. Over UDP the client tracks the server's published `/clock` anchor on its own socket; with an in-process or shared-memory server it reads the counter directly.
- **Drift-free and sample-coherent.** Events land on exact samples, and several clients locked to the same master share one sample axis.
- **Graceful.** With no reachable master — an offline render, or no server running — `lock_to` leaves the clock on wall-clock time instead of failing.

### Watching it, from Python

The point of this model is real, drift-free timing, so it is worth *seeing* it. The sample-clock tracker reads the server's live position from real `/clock` replies; everything below is plain Python, with a server running (the installed `clausters` command). Build the tracker explicitly so you can read it, and hand its timebase to the clock:

```python
sc = server.sample_clock()                  # a tracker on its own socket
sc.warmup(); sc.track()                      # seed and keep the model fresh
clock = TempoClock(tempo=2.0, timebase=sc.timebase())   # same lock as lock_to, with a handle

print("rate:", sc.rate, "Hz | drift:", f"{sc.model.drift_ppm():.1f} ppm")
before = sc.now()                            # the server's sample counter, now
clock.run(3.0)                               # play something for 3 seconds
after = sc.now()
print(f"counter advanced {after - before} samples = {(after - before) / sc.rate:.3f} s")
```

`sc.now()` is the server's real sample counter (the model is fit from live round trips, not guessed), `sc.rate` is its measured sample rate, and `sc.model.drift_ppm()` is the actual measured difference between the two clocks. To verify the lock: the advance should match the `3.0` seconds you ran the clock to within the tracker's small uncertainty, and `drift_ppm` should be a handful of ppm, not hundreds. (The server can also print the exact sample of each scheduled event at trace level — enable it from Python with `server.request("/verbosity", "clausters::osc=trace", expect=("/done",))` and read the server's own terminal — but the client-side reading above needs nothing but Python.)

`clock.lock_to(server)` is the same lock in one call when you do not need the tracker handle; it falls back to wall-clock time if no master answers.

## Shared transport — phase-aligning several clients

> This section is that timing model in brief; [A DAW-style transport](transport.md) is the full workflow guide — conducting, following, starting together on a bar, and following a tempo change live.

Locking to a master gives every client the same sample axis, but each routine still *starts* whenever you play it. To make several clients begin on the **same beat**, two pieces work together:

- **`quant`** — `clock.play(routine, quant=4)` (or `session.play(pattern, quant=4)`) snaps the routine's start to the next beat that is a multiple of `quant` (a bar in 4/4). `None` or `0` starts immediately. On its own it snaps to the clock's own grid — handy for one client adding a voice cleanly on the next bar.
- **A shared transport** — `clock.join_transport(server)` (or `Session.join_transport()`) adopts the server's `/transport` grid: its tempo and an origin every client shares. Now `quant` snaps to *that* grid, so every client on it hits the same bar. One client (the conductor) defines it with `server.set_transport(origin_sample, tempo)`; the others join.

With each client also `lock_to` the master, the shared bar is an exact sample, so the clients are sample-aligned; in plain wall-clock mode they are beat-aligned (drift-bounded, via the server's OSC-time anchor). Start the clock before playing a quantized routine, so `quant` snaps against the running grid.

### Trying it

The `transport_sync.py` example (see [Examples](examples.md)) sets a transport, has two independent clients join and lock, and shows them landing on the same bar. The check is that both compute the *same* next-bar sample — using only public state, so any client on the same transport gets the same number:

```python
import math

def next_bar_sample(server, clock, quant=4):
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    grid_beat = (clock.timebase.current_sample() - origin) * tempo / rate
    target = math.ceil(grid_beat / quant) * quant
    return round(origin + target * rate / tempo)
```

Run it for two clients sampled back-to-back and the two values match to the sample.

## Reference is independent of destination

The time model is **orthogonal to the destination** — where the OSC actually goes (any OSC endpoint, a local or remote server). The one hard rule is that the sample clock and the transport need a Clausters master; everything else falls back to wall-clock.

| You are talking to… | Model | How |
| --- | --- | --- |
| nothing / another OSC program | wall-clock OSC time | a bare `TempoClock`, or `[client].clock = "monotonic"` |
| a remote server across a network | wall-clock OSC time | `[client].clock = "monotonic"` (NTP/PTP-quality sync) |
| a local / LAN Clausters server | sample clock | `Session.live()` default; `lock_to` for a bare clock |
| several clients, one server | sample clock + transport | the session default `lock_to`, then `join_transport` |

A `Session` sample-locks by default and falls back to wall-clock if no master answers, so the "local server" row is the zero-config case. The `"monotonic"` rows are the opt-out for when you *want* wall-clock — driving a remote or non-Clausters peer.

## MIDI always rides OSC time

MIDI output never uses the sample clock. A `MidiServer` writing a score keeps its timeline in beats (logical/OSC time) and quantizes to ticks only when it writes the file; live MIDI output is emitted on the clock's logical time. `lock_to` changes only how the *OSC* `Server` schedules; it does not touch MIDI timing — MIDI is not sample-exact by design, and the client may have no sample clock at all. Live OS MIDI output is therefore best-effort; for exact MIDI timing, write a score offline (its ticks come from logical time). Tighter live-MIDI timing is a possible future refinement.

## The API, at a glance

- `TempoClock.lock_to(server)` / `unlock()` — switch to / off the server's sample clock. `Session.lock_to_server()` is the session wrapper. Blocking (it does `/clock` round trips); call before `start`/`run`, never from a routine.
- `TempoClock.join_transport(server)` / `leave_transport()` — adopt / drop the server's shared transport grid. `Session.join_transport()` is the wrapper.
- `Server.set_transport(origin_sample, tempo)` / `Server.transport()` — define / read the shared grid (the conductor sets it once).
- `play(routine, quant=...)` — start on the next `quant`-beat boundary of the current grid.

## See also

- [Routines and clocks](routines-and-clocks.md) — writing the routines you play on these clocks.
- [Sessions](sessions.md) — the handle that bundles a clock and a server.
- [API reference](api.md) — `TempoClock.lock_to` / `join_transport`, `Session`, `Server`.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/clock` (the master-clock anchor), `/sched` and `/transport`.
