# Timing models

A clock can keep time in a few different ways — *models* — and this page is the one place to understand and **try** each: what it is, how to make a clock on it, and how to watch its timing from {{alt Python || TypeScript}}. The choice of model is independent of *where* the OSC goes. [Routines and clocks](routines-and-clocks.md) is the companion page: it builds the routines you play on these clocks.

| Model | In one line | How to select it |
| --- | --- | --- |
| Wall-clock OSC time | the client's own clock; works everywhere, including with no server | a bare {{alt `TempoClock(tempo)` || `new TempoClock(tempo)`}} |
| Sample clock | paces on a server's sample counter; drift-free, sample-exact | {{alt `TempoClock(timebase=server.sample_timebase())` || `new TempoClock(1, { timebase: await server.sampleTimebase() })`}} — **and the default for `Session.live()` and `Session.embed()`** |
| Shared transport | a server-hosted beat grid several clients align on | {{alt `clock.join_transport(server)` || `await clock.joinTransport(server)`}} + `quant` |

All three ride **logical time** — the jitter-free relative timing a routine's `yield`s define (see [Routines and clocks](routines-and-clocks.md)). They differ in the *reference* the clock paces against and how it addresses events on the wire.

**A clock's timebase is fixed when it is made, and a session sets it.** A
`TempoClock` made while a session is active is made on **the session's
timebase** and kept in the session; one made with no session active paces on
wall-clock time (below) unless it is given a timebase. A `Session.live()` or
`Session.embed()` is on **the server's sample clock by default** (the next
section) — `live` by tracking it over {{alt UDP || a WebSocket}}, `embed` by reading the
in-process counter directly (no tracker, no round trips). <!-- python -->That default is set by the
config key `[client].clock` (`"sample"` by default, `"monotonic"` to opt out),
or per session by passing `timebase=`.<!-- /python --><!-- web -->That default is changed per
session, by passing a `timebase` in its options: the web client reads no
configuration file.<!-- /web --> An offline session (`Session.nrt()`) is
on a `LogicalTimebase`, and it is the only time it can have.

A clock never changes mode: rendering one that is not on logical time {{alt raises || throws}},
and so does making one on another timebase than the active session's. The
reason is its queue — every beat already scheduled is measured against the time
the clock was made on.

## Wall-clock OSC time

A plain clock paces against wall-clock **OSC time** (OSC timetags are NTP: absolute seconds since 1900). You get it from a clock made with no session active, or from a session made on it:

```python
clock = TempoClock(tempo=2.0)                        # no session active
session = Session.live(host, port, timebase=MonotonicTimebase())  # or: [client].clock = "monotonic"
```

```ts
const clock = new TempoClock(2.0);                   // no session active
const session = await Session.live(url, { timebase: new MonotonicTimebase() });
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

```ts
const session = await Session.live(url);   // on the sample clock
const clock = new TempoClock(1, { timebase: await server.sampleTimebase() });   // no session: ask for it
```

- The server becomes the **master clock**. Over {{alt UDP || a WebSocket}} the client tracks the server's published `/clock_query` anchor<!-- python --> on its own socket<!-- /python -->; with an in-process <!-- python -->or shared-memory <!-- /python -->server it reads the counter directly.
- **Drift-free and sample-coherent.** Events land on exact samples, and several clients locked to the same master share one sample axis.<!-- web --> In a page the engine and the `AudioContext` are one clock, so the session's clock is exact there.<!-- /web -->
- **It {{alt raises || throws}} when no master answers.** A clock's timebase is fixed when it is made, so there is nothing to fall back to afterwards: `{{api sample_timebase}}` (and a `Session.live()` left on its default) {{alt raises || throws}} and says why, and a session that has to run with no server is made with {{alt `timebase=MonotonicTimebase()` || `{ timebase: new MonotonicTimebase() }`}}. An offline server has no sample clock at all.

### Watching it, from {{alt Python || TypeScript}}

The point of this model is real, drift-free timing, so it is worth *seeing* it. The sample-clock tracker reads the server's live position from real `/clock_query` replies; everything below is plain {{alt Python || TypeScript}}, with a server running (<!-- python -->the installed `clausters` command<!-- /python --><!-- web -->a native one, reached over its WebSocket<!-- /web -->). Build the tracker explicitly so you can read it, and hand its timebase to the clock:

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

```ts
const sc = await sampleClockFor(server, { warmup: true });   // seeded with a warmup
sc.track();                                                  // keep the model fresh
const clock = new TempoClock(2.0, { timebase: sc.timebase() });   // what sampleTimebase does, with a handle

console.log("rate:", sc.rate, "Hz | drift:", `${sc.driftPpm?.toFixed(1)} ppm`);
const before = sc.now();                                     // the server's sample counter, now
await clock.run(3.0);                                        // play something for 3 seconds
const after = sc.now();
console.log(`counter advanced ${after - before} samples = ${((after - before) / sc.rate).toFixed(3)} s`);
```

`sc.now()` is the server's real sample counter (the model is fit from live round trips, not guessed), `sc.rate` is its measured sample rate, and {{alt `sc.model.drift_ppm()` || `sc.driftPpm`}} is the actual measured difference between the two clocks. To verify the lock: the advance should match the `3.0` seconds you ran the clock to within the tracker's small uncertainty, and the drift should be a handful of ppm, not hundreds. (The server can also print the exact sample of each scheduled event at trace level — enable it with {{alt `server.request("/server_verbosity", "clausters::osc=trace", expect=("/done",))` || `await server.request("/server_verbosity", ["clausters::osc=trace"], { expect: ["/done"] })`}} and read the server's own terminal — but the client-side reading above needs nothing but {{alt Python || the client}}.)

`{{api Server.sample_timebase}}()` is the same in one call when you do not need the tracker handle; it {{alt raises || throws}} if no master answers.

## Shared transport — phase-aligning several clients

> This section is that timing model in brief; [A DAW-style transport](transport.md) is the full workflow guide — conducting, following, starting together on a bar, and following a tempo change live.

Locking to a master gives every client the same sample axis, but each routine still *starts* whenever you play it. To make several clients begin on the **same beat**, two pieces work together:

- **`quant`** — {{alt `clock.play(routine, quant=4)` || `clock.play(routine, 4)`}} (or {{alt `session.play(pattern, quant=4)` || `session.play(pattern, 4)`}}) snaps the routine's start to the next beat that is a multiple of `quant` (a bar in 4/4). {{alt `None` || `null`}} or `0` starts immediately. On its own it snaps to the clock's own grid — handy for one client adding a voice cleanly on the next bar.
- **A shared transport** — {{alt `clock.join_transport(server)` || `await clock.joinTransport(server)`}} (or `{{api Session.join_transport}}()`) adopts the server's `/transport_set` grid: its tempo and an origin every client shares. Now `quant` snaps to *that* grid, so every client on it hits the same bar. One client (the conductor) defines it with {{alt `server.set_transport(origin_sample, tempo)` || `await server.setTransport(originSample, tempo)`}}; the others join.

With each client's clock also made on the master's sample clock, the shared bar is an exact sample, so the clients are sample-aligned; in plain wall-clock mode they are beat-aligned (drift-bounded, via the server's OSC-time anchor). Start the clock before playing a quantized routine, so `quant` snaps against the running grid.

### Trying it

The {{alt `transport/sync.py` || `transport/sync.html`}} example (see [Examples](examples.md)) sets a transport, has two independent clients join it on the sample clock, and shows them landing on the same bar. The check is that both compute the *same* next-bar sample — using only public state, so any client on the same transport gets the same number:

```python
import math

def next_bar_sample(server, clock, quant=4):
    origin, tempo = server.transport()
    rate = clock.timebase.sample_rate
    target = math.ceil(clock.grid_beat() / quant) * quant
    return round(origin + target * rate / tempo)
```

```ts
async function nextBarSample(server: Server, clock: TempoClock, quant = 4): Promise<number> {
    const { originSample, tempo } = (await server.transport())!;
    const rate = (clock.timebase as SampleClockTimebase).sampleRate;
    const target = Math.ceil(clock.gridBeat() / quant) * quant;
    return Math.round(originSample + target * rate / tempo);
}
```

Run it for two clients sampled back-to-back and the two values match to the sample.

## Reference is independent of destination

The time model is **orthogonal to the destination** — where the OSC actually goes (any OSC endpoint, a local or remote server). The one hard rule is that the sample clock and the transport need a Clausters master; everything else runs on wall-clock time.

<!-- python -->
| You are talking to… | Model | How |
| --- | --- | --- |
| nothing / another OSC program | wall-clock OSC time | a bare `TempoClock`, or `[client].clock = "monotonic"` |
| a remote server across a network | wall-clock OSC time | `[client].clock = "monotonic"` (NTP/PTP-quality sync) |
| a local / LAN Clausters server | sample clock | `Session.live()` default; `server.sample_timebase()` for a clock with no session |
| several clients, one server | sample clock + transport | the session default, then `join_transport` |

A `Session` is on the sample clock by default and raises if no master answers, so the "local server" row is the zero-config case. The `"monotonic"` rows are the opt-out for when you *want* wall-clock — driving a remote or non-Clausters peer.
<!-- /python -->
<!-- web -->
| You are talking to… | Model | How |
| --- | --- | --- |
| nothing / another OSC program | wall-clock OSC time | a bare `TempoClock`, or `{ timebase: new MonotonicTimebase() }` |
| a remote server across a network | wall-clock OSC time | `{ timebase: new MonotonicTimebase() }` (NTP/PTP-quality sync) |
| the page's engine, or a local / LAN Clausters server | sample clock | `Session.embed()` / `Session.live()` default; `server.sampleTimebase()` for a clock with no session |
| several clients, one server | sample clock + transport | the session default, then `joinTransport` |

A `Session` is on the sample clock by default and throws if no master answers, so the "local server" row is the zero-config case. The `MonotonicTimebase` rows are the opt-out for when you *want* wall-clock — driving a remote or non-Clausters peer.
<!-- /web -->

## MIDI always rides OSC time

MIDI output never uses the sample clock. A `MidiServer` writing a score keeps its timeline in beats (logical/OSC time) and quantizes to ticks only when it writes the file; live MIDI output is emitted on the clock's logical time. The sample clock changes only how the *OSC* `Server` schedules; it does not touch MIDI timing — MIDI is not sample-exact by design, and the client may have no sample clock at all. Live OS MIDI output is therefore best-effort; for exact MIDI timing, write a score offline (its ticks come from logical time). Tighter live-MIDI timing is a possible future refinement.

### An MPE zone as the destination

{{alt `MidiServer(zone=15)` || `new MidiServer({ zone: 15 })`}} makes the destination an **MPE zone** — the lower one, master channel 0, unless `upper`. The RPN that declares it goes out first, and each note goes on a member channel of its own, preceded by its expression: the event's `bend` in semitones (through `{{api bend_range}}`, 48 by default), its `press` (the pressure) and its `slide` (the timbre), 0..1 — the controls a server's zone names, so one pattern plays either end. So three notes of one chord can be bent, pressed and coloured apart, which one channel cannot say. The channel is chosen by the shared crate's assigner (a free member first, else the one held longest), the same rule on every end. A server plays a zone the same way when one is bound with `{{api Server.midi_bind_zone}}`; `crates/clausters-server/examples/midi_mpe.py` does both.

## The API, at a glance

- `{{api Server.sample_timebase}}()` — the server's sample clock as a timebase to make a clock on ({{alt `TempoClock(timebase=...)` || `new TempoClock(tempo, { timebase })`}}); `Session.live()`/`embed()` use it by default. <!-- python -->Blocking over UDP (it does `/clock_query` round trips; instant on an embedded server); call before `start`/`run`, never from a routine.<!-- /python --><!-- web -->Asynchronous over a WebSocket (it does `/clock_query` round trips; immediate on the page's engine); await it before `start`/`run`.<!-- /web -->
- `Session.timebase` — the time every clock of a session is made on.
- `{{api TempoClock.join_transport}}(server)` / `{{api leave_transport}}()` — adopt / drop the server's shared transport grid. `{{api Session.join_transport}}()` is the wrapper.
- `{{api Server.set_transport}}({{alt origin_sample || originSample}}, tempo)` / `Server.transport()` — define / read the shared grid (the conductor sets it once).
- {{alt `play(routine, quant=...)` || `play(routine, { quant })`}} — start on the next `quant`-beat boundary of the current grid.

## See also

- [Routines and clocks](routines-and-clocks.md) — writing the routines you play on these clocks.
- [Sessions](sessions.md) — the handle that bundles a clock and a server.
- {{alt [API reference](api/clausters.md) || [API reference](api/index.md)}} — `{{api Server.sample_timebase}}`, `{{api TempoClock.join_transport}}`, `Session`, `Server`.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — `/clock_query` (the master-clock anchor), `/sched_at` and `/transport_set`.
