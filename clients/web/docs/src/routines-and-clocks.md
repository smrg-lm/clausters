# Routines and clocks

`Pbind` and the timeline are convenience layers over three plain objects you can also drive yourself:

- a **`Routine`** — *what* happens over time, written as a generator function that `yield`s how long to wait;
- a **`TempoClock`** — *when* it happens: it schedules the routine and keeps musical time in beats;
- a **`Server`** — *where* the sound goes: it owns the connection and plays events on it.

This page works at that level, on the high-level API throughout: you build `Event`s and call `play`, never hand-assemble OSC bundles. The model is the [Python client's](https://clausters-python.readthedocs.io/) — the same events, the same patterns, the same arithmetic, computed by the same native core — so this page covers the part that is genuinely the browser's, and links there for the rest.

## Logical time vs physical time

This is the idea that makes routines worth using, inherited from SuperCollider, and it is what survives a browser.

A routine `yield`s numbers, and each one is a wait *in beats* before the clock resumes it. Those waits play out in **physical time** — the actual milliseconds a timer sleeps — and in a tab they jitter badly: a background tab throttles its timers to about a second, a long paint blocks everything, and there is one thread for all of it. But a routine also keeps a **logical time**: the running sum of everything it has yielded, relative to when it started and the clock's tempo. That sum has no jitter — a routine that yields `0.5` four times is at logical beats `0, 0.5, 1.0, 1.5` exactly, whatever the browser did in between.

The `Server` stamps every event from the routine's **logical** time, not from "now". The wake-up only has to arrive within the emission headroom (`server.latency`); the exact instant rides on the bundle's timetag, which is already in the server's future when it is sent. That is why a page can hold a steady pulse while it is also drawing, and it is the whole reason the sequencing layer is worth having in a tab at all.

## A routine by hand

An `Event` is a bag of note parameters that knows how to play itself: `event.play(server)` creates a synth at the routine's current logical beat and schedules its release after the note's sustain. Play it from inside a routine, yielding the gap to the next note:

```js
import { Routine, Server, TempoClock, seq } from "./dist/index.js";

// A server already running: `attach` owns nothing it did not start.
const url = "ws://127.0.0.1:57120";
const server = await new Server({ transport: "ws", url }).attach();
server.latency = 0.1;                              // the emission headroom

const clock = new TempoClock(2.0);                 // 2 beats per second
clock.start();

function* melody() {
  for (const note of [60, 62, 64, 67, 69]) {       // MIDI notes
    const e = new seq.Event({ midinote: note, amp: 0.2, dur: 0.5 });
    e.play(server);                                 // half-beat note
    yield e.delta();                                // advance to the next note
  }
}

clock.play(new Routine(melody));                   // schedule it to start now
```

A few things worth knowing:

- An `Event` carries musical defaults (see the [API reference](api/index.md)): `midinote` (or `degree`, or an explicit `freq`) sets pitch, `amp` the level, `instrument` the def — the server has a built-in `default` sine, which is why this example sends no def of its own. Timing comes from `dur`: the note's `delta` (beats to the next event) is `dur`, and its `sustain` (how long it sounds) is `dur * legato` (`legato` defaults to `0.8`). An explicit `delta` or `sustain` overrides that calculation.
- `clock.start()` / `clock.stop()` are a **transport, not a reset**: `stop` holds the beat the clock reached and `start` resumes from it, so whatever is still queued keeps its place in the music. `clock.clear()` is what drops it, and `clock.close()` also releases the ticker's worker slot.
- `clock.setTempo(bps)` changes tempo **pinning the current instant**: the beat the clock is on keeps mapping to the second it already mapped to, and the new tempo governs from there — so nothing already scheduled jumps.
- `clock.play(item, quant)` snaps a start to a beat grid — `4` starts it on the next bar in 4/4, nothing starts it now. The grid is the clock's own elapsed beats, unless it has joined the server's [shared transport](transport.md), which is what makes several clients start on the same bar.
- The clock never talks to a server, and the server is not told about the clock: `play` reads the logical beat off the routine being resumed. One clock can drive routines against two servers, and one server can be played by two clocks.

A routine has its own transport, and it is not the clock's — `clock.stop()` halts the clock and every routine on it, while these touch only this routine: `routine.pause()` takes it off the clock **keeping its place**, so a later `routine.play(clock)` resumes at the very `yield` it stopped on; `routine.stop()` takes it off **and rewinds**, so the next `play` starts the generator afresh; `routine.reset()` rewinds without unscheduling.

A routine that throws is dropped the same way, with the error on the console: it loses its place in the schedule and nothing else does. The clock keeps driving the other routines — an error escaping the driver would leave it armed for no next wake, running and firing nothing.

There is no `run(seconds)` here, because nothing in a page may block: a script that waited would freeze the same thread the clock, the engine's messages and the whole document run on. The clock runs until you stop it, and the piece ends when its routines do.

### The one rule

**Never `await` inside a routine.** A routine is resumed by the clock and must return control synchronously; an `await` suspends it mid-beat and holds the queue behind it, which is the browser's version of the Python client's "never block the clock thread". Do the waiting *before* — `await def.send(server)` resolves when the server has acknowledged it — and start the routine after, or `yield` enough beats before the first note that uses a def you sent without awaiting.

## The two timebases

The clock measures its sleeps against a **timebase**, and the timebase also decides how the `Server` stamps what it emits:

- **`MonotonicTimebase`**, the default, paces on `performance.now()` and emits NTP-timetagged bundles. Relative timing inside the routine is exact; the client's clock and the server's sample clock are still two clocks, and they drift.
- **`SampleTimebase`** paces on the server's own sample counter and emits `/sched_at <absolute sample>`. There is no drift left to speak of, because there is only one clock.

`server.sampleTimebase()` builds one, and the `Server` is what builds it because the `Server` is what knows the carrier: over the in-page engine it pairs the engine's counter with the `AudioContext`'s in one worklet round trip — they are the same clock, so the pairing is exact — while over a socket it feeds `/clock_query` anchors into the core's model. Hand it to the clock at construction:

```js
const clock = new TempoClock(2.0, { timebase: await server.sampleTimebase() });
```

The rule the clock keeps either way: **it reads the timebase, and never talks to a server**.

## The wake-up

One more piece is the browser's alone. The clock is woken through a `Ticker`, and the default in a tab is a **shared worker**, because a page's own timers are throttled to roughly one second when the tab is in the background — a routine paced by `setTimeout` would simply stop being music the moment the user changed tabs. The worker's wake-ups are not throttled that way, and since the exactness lives in the timetag, all the wake-up has to do is arrive inside the headroom.

The seam is also what makes the layer testable: a test supplies its own ticker and its own timebase and drives the real driver by hand, deterministically, with no audio device and no waiting.

## Patterns, and the seekable form

The routine is the forward-only form. Above it sit the same two layers as in the reference client — `Pbind` over the value patterns for the generative form, `Timeline` and its `Playhead` for the static, editable, seekable one:

```js
new seq.Pbind({
  degree: new seq.Pseq([0, 2, 4, 7], seq.INF),
  dur: new seq.Pseq([0.5, 0.25, 0.25]),
}).play(server, { clock });
```

What an event's keys mean, how `dur` and `sustain` differ, what `Pbind` does with a pattern of patterns, how `Timeline.fromPattern` bounces one into the other — that is all the shared model, documented once in the Python book's [routines and clocks](https://clausters-python.readthedocs.io/) and [timelines](https://clausters-python.readthedocs.io/) chapters.

## Reproducible randomness

Everything random — `Pwhite`, `Prand`, and the module functions `uniform(lo, hi)`, `nextBelow(n)`, `choice(items)` — draws from **one seedable context**, the sclang model, computed by the same core the other clients use: the same seed replays the same values in every Clausters client language.

Each routine gets its **own** generator when it is created, derived from the context that created it. Same root seed plus same creation order gives the same music, and because each routine draws from its own stream, concurrent routines stay reproducible however their wakes interleave. `seed(n)`, called before you build and play, makes a whole page reproducible; there are deliberately no per-pattern seeds. To isolate some samples, play it in its own routine, which is its own derived stream by construction.

## Automation: a curve driving a control

A note is not the only thing a piece places in time. An **`Automation`** is a break-point curve that drives one or more `[node, control]` targets — a filter sweep, a fade, a glissando — and it is played the way an event is: it has a duration, it goes on a timeline, and `play` starts it.

How it is rendered is worth knowing, because it is machinery you already have. The curve is discretized on the **server** into a control buffer (`/buffer_gen "env"`, evaluated through the same envelope-shape math the `EnvGen` UGen plays), and at play time a small internal synth reads that buffer onto a **control bus** over the curve's duration; the targets follow that bus with `/node_map`. So the curve is computed where the sound is, not stepped from the page.

```js
import { seq } from "./dist/index.js";

// Break-points are [time, value, shape, curve]: times in beats, values in the
// control's real units, shapes the server's own envelope numbers (1 linear,
// 2 exponential, 5 a numeric curvature in `curve`).
const sweep = seq.Automation.fromPoints(
  [0.0, 200.0, 1, 0.0, 2.0, 4000.0, 2, 0.0],
  [synth, "cutoff"],
);

await sweep.prepare(server);   // allocate and fill the buffer, allocate the bus
sweep.play(server);            // schedule the lane; nothing here waits
```

**The two phases are the point.** `prepare` allocates and fills — it waits on the server, so it belongs at setup. `play` only *schedules* (the lane synth, the `/node_map`s and the free at the end of the curve) and waits for nothing, which is what makes it callable from inside a routine, where an `await` would hold up the whole timeline. Play it from a routine and the lane starts at the routine's exact logical beat, the curve's beats being the clock's; play it with no clock in context and it starts now, its beats reading as seconds.

`stop()` frees the lane synth mid-sweep, so the mapped controls **hold** their last value rather than jumping; `free()` returns the buffer and the bus to their allocators.

### The curve is the drawn one

The stored curve is an `Env`, the same object the `bpf` editor round-trips (`envToPoints` / `pointsToEnv`), so a drawn envelope and a played automation are one object rather than two representations that have to agree:

```js
const win = await gui.view({ title: "lane", w: 520, h: 260 },
  gui.bpf({ name: "curve", points: sweep.toPoints(), min: 100, max: 5000,
            duration: 4.0, exp: true })).open();

win.widget("curve").onEvent((...args) => {
  // The edit comes back as the same flat break-point list `fromPoints` takes.
  points = args.flat().filter((v) => typeof v === "number");
});
```

`examples/transport/automation-lane.html` is exactly that loop: draw the glissando, play it, hear what you drew.

**One difference from the reference client**, and it is the page's single thread again: `play(auto)` there prepares an unprepared curve on the spot, blocking. A synchronous verb in a page cannot, so an unprepared curve is refused by name — `await auto.prepare(server)` first.

## Sending to another application

The same logical beat is available to anything else that speaks OSC. A **destination** is where OSC goes: `Server` is the one we control, `OscDestination` is any other application.

```js
const lights = await clausters.OscDestination.open("ws://localhost:7000");

clock.play(new clausters.Routine(function* () {
    while (true) {
        new seq.Event({ instrument: "default", freq: 330 }).play(server);
        lights.sendBundle([["/lamp", "amber", 1.0]]);
        yield 1.0;
    }
}));
```

The note and the lamp carry the *same* timetag, because both read the same `Moment` — the beat the routine has accumulated, not what time it happens to be when each line runs.

What a destination sends is standard OSC and nothing more: a message, or a bundle with an NTP timetag. It does not add the server's `latency` — that is a property of our audio pipeline, and what another application needs is its own business, asked for as an explicit `delayBeats`. Nor does it send our own commands (`/sched_at`, `/server_sync`).

The page cannot open a UDP socket, so a destination here rides a `Connection` exactly as the server does: for an external application, its WebSocket bridge. The Python client, which can, defaults to UDP — the difference is the carrier, never the timing.

## The application's clock: `appClock()`

There are **two** clocks, and they are not two implementations of one thing.
`TempoClock` keeps musical time: beats, a tempo, and a piece plays on it.
`AppClock` keeps the **application's** time — seconds on the page's own loop —
and it is where anything that touches a *window* belongs: an animation, a
periodic read-out, a redraw, a follow-up to a gesture.

```js
const clock = await gui.appClock();          // the ambient host's
clock.sched(0.5, () => win.widget("knob").set({ value: 1.0 }));
clock.play(new clausters.Routine(function* () {
    for (;;) {                                // an animation is a routine that waits
        win.widget("lamp").set({ color: "red" });
        yield 0.25;
        win.widget("lamp").set({ color: "grey" });
        yield 0.25;
    }
}));
```

That is the shape worth taking from sclang's three clocks: the loop's timer
source *is* the clock, so an animation is a routine that waits rather than an
animation API beside the routines you already have. A function scheduled here
follows the same contract as one on the `TempoClock` — return a number and it is
rescheduled by that many **seconds**, return nothing and it ran once.

`defer` is the other half. It hands work to the loop and returns at once,
landing *after* the current task rather than inside it:

```js
clock.defer(() => win.close());
```

In the reference client that door matters more than it does here, because there
a routine on the musical clock runs on a thread it must never block. A page has
one thread for everything, so the rule is stronger and simpler: **nothing may
block**, and `defer` is how a piece of work says "not in the middle of this one".

The reference client builds this clock over an `EventLoop` of its own — a
thread, a selector and a wake channel — because a Python script's windows are
drained by something the script started. A page *is* that loop, so there is no
`EventLoop` here and none is missing: the class, the calls and what they mean
are the same, and what is absent is only the part that made a thread behave like
a page.

## See also

- [The client, layer by layer](guide.md) — where routines, clocks and the server sit in the whole client.
- [Getting started](getting-started.md) — the page that plays a note, if you have not run one yet.
- [Examples](examples.md) — `sequencing.html` is this page as something you can hear: the generative half and the seekable half side by side.
- [API reference](api/index.md) — `TempoClock`, `Routine`, `Event`, and the `Server` methods used here.
