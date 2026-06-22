# Routines and clocks

`Session` and `Pbind` are convenience layers over three plain objects you can also drive yourself:

- a **`Routine`** — *what* happens over time, written as a Python generator that `yield`s how long to wait;
- a **`TempoClock`** — *when* it happens: it schedules the routine and keeps musical time in beats;
- a **`Server`** — *where* the OSC goes: it owns the communication interface and emits.

This page works at that level. It shows how to write a routine by hand, schedule it on a clock, emit OSC from inside it through a `Server`, and — the end goal — anchor the clock to the server's own sample counter so every event lands on an exact, logged sample.

## The shape of a routine

A routine is a generator function. Each value it `yield`s is a *delay in beats* before the clock resumes it; in between, it does whatever you want — typically emit OSC. You wrap the function in a `Routine` and hand it to a clock.

The key property is that the clock's **logical beat advances only by those yields**, never by wall-clock drift. A routine that yields `1.0` four times spans exactly four beats, whatever the OS scheduler does in between, and `Server.send_bundle` stamps each event's timetag from that exact logical beat rather than from "now". So inter-event timing is exact by construction.

```python
from clausters.base import TempoClock, Routine
from clausters.defs import Server

server = Server("127.0.0.1", 57110, latency=0.1)   # a running server, on UDP
clock = TempoClock(tempo=2.0)                       # 2 beats per second

def arpeggio():
    for freq in [440.0, 550.0, 660.0, 880.0]:
        nid = server.nodes.alloc()                  # a fresh node id
        # /s_new default <id> <add: 1=tail> <target: 0=root group> freq .. amp ..
        server.send_bundle(("/s_new", "default", nid, 1, 0, "freq", freq, "amp", 0.2))
        server.send_bundle(("/n_free", nid), delay_beats=0.9)   # release after 0.9 beats
        yield 1.0                                   # one beat to the next note

clock.play(Routine(arpeggio))                       # schedule it to start now
clock.run(2.5)                                      # advance the clock 2.5 s, then stop
server.close()
```

Three things to notice:

- `send_bundle` takes one or more `(address, *args)` tuples and emits them at the running routine's logical beat. `delay_beats=` offsets a message into the future (here, the note-off) without a separate `yield`.
- `clock.run(seconds)` starts the real-time driver, waits, and stops it. Use `clock.start()` / `clock.stop()` when you want to keep the clock running across several routines.
- The server has a built-in `default` def (a sine at `freq` scaled by `amp`), which is why this runs with no def-loading. To play your own instrument, send a `SynthDef` or `FaustDef` first — see [The client, layer by layer](guide.md).

### One-shots and composing routines

The clock also schedules plain callables for one-off work: `clock.sched(delay_beats, fn)` runs `fn` after a delay, and if `fn` returns a number it is rescheduled by it. A routine can schedule further routines (`clock.play(Routine(other))`) to layer voices, and raising `YieldAndReset(value)` from inside a routine yields `value` and then restarts the generator — a loop without an outer `while`.

### The one rule

**A routine must never block the clock thread.** It runs *on* that thread (in real time) or inside the render loop (offline), so a `time.sleep`, a blocking `Server.sync()`, or any `wait=True` def send freezes every other routine and the whole timeline. Cede time with `yield` instead. To load a def from within a routine, send it asynchronously — `server.add_synthdef(sdef, wait=False)` (or `add_faustdef`) — and `yield` enough time before the `/s_new` that depends on it.

## Sample-accurate timing

The clock above paces against the OS monotonic clock and sends each event as a wall-clock (NTP) timetag. That is simple and usually fine, but the client's clock and the server's audio clock are two different crystals: they drift, slightly but really, so over a long take the grid slowly slips against the audio.

For drift-free timing you anchor the clock to the server's **own sample counter**. The server's audio clock becomes the single source of truth, and the `Server` schedules every event by *absolute sample* (`/sched <sample>`) instead of a wall-clock timetag. Over UDP — where the client cannot read the counter out of shared memory — the client tracks it by querying the server's `/clock` and fitting a line to the `(local time, sample)` anchors:

```python
sc = server.sample_clock()      # a tracker on its own socket (server.sample_clock())
sc.warmup()                     # a few /clock round trips to seed the model
sc.track()                      # keep re-anchoring in the background

clock = TempoClock(tempo=2.0, timebase=sc.timebase())
```

Passing `sc.timebase()` (a `SampleClockTimebase`) is the whole switch: the clock now paces against the server's sample clock, and the `Server`, seeing a sample timebase, emits via `/sched <absolute_sample>`. Relative timing between events is exact at the sample — the query latency only shifts the entire grid by a small bounded constant, it does not accumulate.

## Logging the exact sample

Because *you* wrote the yields, you know each event's logical beat, and you can compute the very sample the server will execute the bundle at — the same arithmetic the `Server` uses for `/sched`:

```python
tb = clock.timebase             # the SampleClockTimebase

def arpeggio():
    beat = 0.0
    for freq in [440.0, 550.0, 660.0, 880.0]:
        nid = server.nodes.alloc()
        server.send_bundle(("/s_new", "default", nid, 1, 0, "freq", freq, "amp", 0.2))
        server.send_bundle(("/n_free", nid), delay_beats=0.9)

        secs = clock.beats2secs(beat)                       # beat -> seconds (native, tempo-aware)
        sample = tb.sample_at(clock.pacing_origin + secs + server.latency)
        print(f"freq {freq:>5.0f} | beat {beat:>4.2f} | sample {sample} | t {sample / tb.sample_rate:.4f}s")

        yield 1.0
        beat += 1.0

clock.play(Routine(arpeggio))
clock.run(2.5)
```

```text
freq   440 | beat 0.00 | sample 11520384 | t 240.0080s
freq   550 | beat 1.00 | sample 11544384 | t 240.5080s
freq   660 | beat 2.00 | sample 11568384 | t 241.0080s
freq   880 | beat 3.00 | sample 11592384 | t 241.5080s
```

The printed `sample` is exactly where the note sounds on the server, to the sample, and the differences are a rigid `0.5 s` at the chosen tempo regardless of network jitter — that is what "sample-accurate" buys you. (The absolute values depend on how long the server has been running; only the spacing is fixed.) You can spot-check the live position any time with `sc.now()` (the model's prediction of the server's current sample) and read the measured drift with `sc.model.drift_ppm()`.

When you are done, stop the tracker and close the connections:

```python
sc.close()        # stop re-anchoring, close the tracker's socket
server.close()
```

## Offline, with the same code

Everything here works unchanged offline. Build the `Server` with an `OscNrtInterface`, drive the clock with `clock.render()` instead of `run()`, and the routine's `send_bundle` calls accumulate a timed score the bundled renderer turns into samples — no server, no sample-clock tracking needed, because an offline render has no other clock to drift against. That swap is the client's central seam, covered in [Sessions](sessions.md) and [The client, layer by layer](guide.md).

## See also

- [Sessions](sessions.md) — the ergonomic handle that bundles a clock and a server, for when you do not need this level of control.
- [The client, layer by layer](guide.md) — where routines, clocks and the server sit in the whole client, and the timebase choice.
- [API reference](api.md) — `TempoClock`, `Routine`, `Event` and the `Server` methods used here.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — the wire formats this page emits: `/s_new`, `/n_free`, `/sched` and the `/clock` reply.
