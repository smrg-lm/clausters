# Routines and clocks

`Session` and `Pbind` are convenience layers over three plain objects you can also drive yourself:

- a **`Routine`** — *what* happens over time, written as a Python generator that `yield`s how long to wait;
- a **`TempoClock`** — *when* it happens: it schedules the routine and keeps musical time in beats;
- a **`Server`** — *where* the sound goes: it owns the communication interface and plays events on it.

This page works at that level: write a routine by hand, play notes from it through a `Server`, anchor the clock to the server's own sample counter so the timing is exact, and then — the point of the page — *log that timing for real*, both from the server and from the client, in a way you can check by eye while you run it.

It stays on the high-level API throughout: you build `Event`s and call `play`, never hand-assemble OSC bundles.

## Logical time vs physical time

This is the idea that makes routines worth using, inherited from SuperCollider.

A routine `yield`s numbers, and each one is a wait *in beats* before the clock resumes it. Those waits play out in **physical time** — the actual wall-clock seconds the OS sleeps — and under load they jitter. But a routine also keeps a **logical time**: the running sum of everything it has yielded so far, relative to when it started and the clock's tempo. The logical time has no jitter — a routine that yields `0.5` four times is at logical beats `0, 0.5, 1.0, 1.5` exactly, whatever the scheduler did in between.

The Server stamps every event from the routine's **logical** time, not from "now". So even though the routine is woken at slightly irregular physical instants, the timing it asks the server for is precise. That is the only way to get jitter-free rhythmic sequences in real time, and everything below builds on it.

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

- An `Event` carries musical defaults (see the API reference): `midinote` (or `degree`, or an explicit `freq`) sets pitch, `amp` the level, `instrument` the def (the server has a built-in `default` sine). Timing comes from `dur`: the note's `delta` (beats to the next event) is `dur`, and its `sustain` (how long it sounds) is `dur * legato`, with `legato` defaulting to `0.8`. A dict works just as well — `Event({"midinote": 60, "amp": 0.2, "dur": 0.5}).play(server)`.
- `clock.run(seconds)` starts the real-time driver, waits, and stops it. Use `clock.start()` / `clock.stop()` to keep one clock running across several routines.
- A routine optionally receives the clock as its argument (`def melody(clock):`) if it needs it, but for playing events you rarely do — the Server finds the logical beat itself.

### The one rule

**A routine must never block the clock thread.** It runs *on* that thread, so a `time.sleep`, a blocking `server.sync()`, or any `wait=True` def send freezes every other routine and the whole timeline. Cede time with `yield` instead. To load a def from within a routine, send it asynchronously — `server.add_synthdef(sdef, wait=False)` — and `yield` enough time before the first note that uses it.

## Sample-accurate timing

The clock above paces against the OS monotonic clock and sends each event as a wall-clock timetag. That is fine for most work, but the client's clock and the server's audio clock are two different crystals: they drift, slightly but really, so over a long take the grid slips against the audio.

For drift-free timing you anchor the clock to the server's **own sample counter**. The server's audio clock becomes the single source of truth, and the `Server` then schedules every event by *absolute sample* instead of a wall-clock timetag. Over UDP the client tracks the counter by querying the server's clock and fitting a line to the `(local time, sample)` anchors:

```python
sc = server.sample_clock()      # a tracker on its own socket
sc.warmup()                     # a few real clock round trips to seed the model
sc.track()                      # keep re-anchoring in the background

clock = TempoClock(tempo=2.0, timebase=sc.timebase())
```

Passing `sc.timebase()` is the whole switch. Nothing in `melody` changes; the events now land on exact samples, and the small, fixed query latency only shifts the whole grid by a constant — it does not accumulate, so the spacing between notes stays sample-exact.

When you do not need the tracker handle yourself (the next section does, to read the clock), the same lock is one call on the clock — `clock.lock_to(server)` — which also falls back to wall-clock time if no master answers. See [Timing references](timing.md) for the two references and when to pick each.

## Logging it, for real

There are two honest places to read the timing: the server can print the sample each event is scheduled for, or the client can read the server's live sample clock. Both are shown below; both report real numbers, not the client's own predictions.

### From the server

The server logs every command at *trace* level, and a scheduled (sample-clock) event is logged with the exact sample it will fire at. Start the server with that one log target turned up:

```sh
RUST_LOG=clausters::osc=trace cargo run --release   # or: RUST_LOG=clausters::osc=trace clausters
```

Now, while a routine plays on a sample-clock-anchored `TempoClock` (the section above), the **server's terminal** prints one line per scheduled message, each ending in the absolute sample:

```text
/s_new ["default", 1000, 1, 0, "freq", 261.6, "amp", 0.2] (at sample 11520000)
/n_free [1000] (at sample 11529600)
/s_new ["default", 1001, 1, 0, "freq", 293.7, "amp", 0.2] (at sample 11532000)
/n_free [1001] (at sample 11541600)
```

This is the real schedule, to the sample. To check it, subtract consecutive `/s_new` samples: at tempo 2.0 a `dur` 0.5 note is half a beat = 0.25 s apart, so the spacing is `0.25 * sample_rate` — `0.25 * 48000 = 12000` samples (`11532000 - 11520000`), and each `/n_free` follows its `/s_new` by the sustain — `dur * legato` = 0.4 beats = `0.2 * 48000 = 9600` samples. The numbers are evenly spaced no matter what the network does — that is what "sample-accurate" means. You can also flip the trace on without restarting the server, live from the client, with `server.request("/verbosity", "clausters::osc=trace", expect=("/done",))`.

### From the client

The same tracker that anchors the clock also exposes the server's live sample position, read from real `/clock` replies. After warming it up, read it before and after a run:

```python
print("rate:", sc.rate, "Hz | drift:", f"{sc.model.drift_ppm():.1f} ppm")
before = sc.now()                       # the server's sample counter, now
clock.run(3.0)
after = sc.now()
elapsed = (after - before) / sc.rate
print(f"counter advanced {after - before} samples = {elapsed:.3f} s of audio")
```

`sc.now()` is the server's real sample counter (the model is fit from live round trips, not guessed), `sc.rate` is the server's measured sample rate, and `sc.model.drift_ppm()` is the actual measured difference between the two clocks. To verify: `elapsed` should match the `3.0` seconds you ran the clock to within the tracker's small uncertainty, and `drift_ppm` should be a handful of ppm, not hundreds.

## Doing it by hand

The point is to watch both sides at once, interactively — so run it in two terminals rather than as one script.

1. **Terminal A — the server, with the OSC trace on.** From the repository (or anywhere the `clausters` binary is installed):

   ```sh
   RUST_LOG=clausters::osc=trace cargo run --release
   ```

   Leave it running and visible; this is where the per-event samples appear.

2. **Terminal B — an interactive Python session.** In a venv where the client is installed (see [Getting started](getting-started.md)):

   ```sh
   python -i
   ```

   Then paste, one block at a time, watching the output of each before the next:

   ```python
   from clausters.base import TempoClock, Routine
   from clausters.seq import Event
   from clausters.defs import Server

   server = Server("127.0.0.1", 57110, latency=0.1)
   sc = server.sample_clock(); sc.warmup(); sc.track()
   clock = TempoClock(tempo=2.0, timebase=sc.timebase())

   def melody():
       for note in [60, 62, 64, 67, 69]:
           e = Event(midinote=note, amp=0.2, dur=0.5)
           e.play(server)
           yield e.delta()

   print("before:", sc.now(), "| rate:", sc.rate)   # read the live server clock
   clock.play(Routine(melody))
   clock.run(3.0)                                    # you should hear five notes
   print("after: ", sc.now(), "| drift:", f"{sc.model.drift_ppm():.1f} ppm")
   ```

3. **Check the two outputs against each other.** Terminal A shows five `/s_new ... (at sample N)` lines with evenly spaced samples; Terminal B shows the counter in `before`/`after` advancing by about `3.0 * rate`. Both describe the same run, from the server's side and the client's, and both are real measurements you can re-run.

4. **Clean up** when you are done:

   ```python
   sc.close(); server.close()
   ```

## Offline, with the same code

Everything here works unchanged offline. Build the `Server` with an `OscNrtInterface`, drive the clock with `clock.render()` instead of `run()`, and the routine's `play` calls accumulate a timed score the bundled renderer turns into samples — no server, no sample-clock tracking, because an offline render has no second clock to drift against. That swap is the client's central seam, covered in [Sessions](sessions.md) and [The client, layer by layer](guide.md).

## See also

- [Sessions](sessions.md) — the ergonomic handle that bundles a clock and a server, for when you do not need this level of control.
- [The client, layer by layer](guide.md) — where routines, clocks and the server sit in the whole client, and the timebase choice.
- [API reference](api.md) — `TempoClock`, `Routine`, `Event` and the `Server` methods used here.
- The **[Clausters server book](https://clausters.readthedocs.io/)** — the wire formats behind these calls: `/s_new`, `/n_free`, `/sched`, `/clock` and the `/verbosity` log control.
```
