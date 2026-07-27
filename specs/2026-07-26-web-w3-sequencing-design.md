# W3 — Sequencing in the TypeScript client: clock, routines, events, patterns

Design for the milestone `clients/web/PLAN.md` labels W3. Approved 2026-07-26.

## What this milestone is

The timing layer of `clients/web`: a `TempoClock` that resumes routines on
musical time, the events and patterns that ride on it, and the timed-send path
on `Server` that turns a routine's logical beat into a timetagged bundle. The
reference is `clients/python/clausters` — `base/clock.py`, `base/stream.py`,
`base/timebase.py`, `base/rand.py`, `base/builtins.py` and `seq/` — and the
target is behavioural parity with it, not a re-invention.

Two properties define the layer and everything below serves them:

1. **The logical beat advances only by the routines' yields.** A routine that
   yields `0.25` is resumed exactly a quarter-beat later, whatever the browser's
   timers do. Wall-clock jitter never accumulates into musical time.
2. **The exactness reaches the server through the timetag, not through the
   wake-up.** The clock wakes early (the `latency` headroom) and stamps the
   bundle with the exact time; the server's own queue plays it. So a late
   wake-up is harmless as long as it arrives within the headroom.

## Division of labour: what is reused, what is written

The milestone is mostly a **Rust** milestone. Every value and time
transformation already exists in `clausters-core` and is already exposed to the
Python client by `clausters-ffi`; W3 opens the same doors on the wasm side, in
`crates/clausters-core-web`, and writes a thin TypeScript driver over them.

Grown in `crates/clausters-core-web` (each one a mechanical shell over
`clausters-core`, holding no logic of its own):

| Door | Backed by |
|---|---|
| `Scheduler` (push/peekTime/popDue/remove/len/clear) | `tempoclock::Scheduler` |
| `beats_to_secs`, `secs_to_beats` | `tempoclock::TempoClock` |
| `secs_to_samples`, `samples_to_secs` | `tempoclock` |
| `quant_delay`, `bar`, `beat_in_bar` | `tempoclock` |
| `unix_to_ntp`, `unix_to_sample` | `osc` |
| `osc_encode_bundle(timetag, messages)` | `osc::bundle` + `osc::encode` |
| `Rng` (fromSeed/state/nextF64/uniform/nextBelow/nextU64/spawn) | `rng::Rng` |
| `unary`, `binary`, `degree_to_midinote` | `builtins` |
| `SampleClockModel` (addAnchor/sampleAt/localTimeOf/driftPpm/…) | `clocksync` |

Written in TypeScript, and nothing else: the coroutine driver (`function*`), the
`Ticker` pacing seam, the queue's id→routine bookkeeping, the composition of
`Event`/`Pbind`/`Timeline`, and the browser carriers. **No time formula and no
random value is computed in TS.** A `Math.random()` or a `beats * tempo` in the
diff is a design error, not a shortcut.

The wasm shell's existing conventions hold: snake_case free functions, camelCase
methods via `js_name`, and every export `#[cfg(target_arch = "wasm32")]` so the
crate still builds natively for its own tests.

## The threading shape: why the clock is not a Worker

The Python client runs the real-time drive on a background thread that *shares
objects* with the rest of the program — the routine, the `Server`, the session.
A browser Worker shares no objects, only structured-cloned messages, and a
routine is a user closure over a `Server` and over its own variables: it cannot
cross. So the coroutine driver stays on the page's thread, which is exactly the
rule `clients/PLAN.md` already states ("the coroutine driver stays in the
language").

What does move to a Worker is the one thing the page thread does badly: the
**wake-up**. `setTimeout` is clamped on the page (≥4 ms when nested) and Chrome
throttles it to ~1 s in a background tab, which is longer than any usable
lookahead. A Worker whose only job is `setTimeout` + `postMessage` is not
throttled that way, and restores the property the Python background thread has.

Hence a `Ticker` seam in `base/clock.ts`:

- `workerTicker()` — one module Worker (`base/tick-worker.ts`, emitted as
  `tick-worker.js` and loaded through `new URL("./tick-worker.js",
  import.meta.url)`, which the no-bundler package shape supports directly),
  shared by every clock on the page. The default wherever `Worker` exists.
- `timerTicker()` — `setTimeout`. The fallback for node and for any environment
  without `Worker`.

Note that `AudioContext.currentTime` keeps advancing in a background tab: only
the wake-up is throttled, never the sample timebase.

## Modules

```
src/base/   clock.ts  timebase.ts  tick-worker.ts  stream.ts  rand.ts  builtins.ts
src/seq/    event.ts  pattern.ts  eventstream.ts  timeline.ts  index.ts
src/defs/   server.ts   (grown with the timed-send path)
```

### `base/stream.ts`

`Stream` (the lazy-sequence protocol: `next(inval)`, `reset()`), `Routine` over a
generator function, `FunctionStream` over a plain callable, and `StopStream`.
Each stream carries its own `rng`, derived at construction from the creating
context — one root seed reproduces a whole script, and concurrent routines stay
reproducible per routine.

A routine is a `function*` that yields a **delay in beats**. `async function*`
is not the driver's contract: awaiting inside a routine both breaks the ambient
context and stalls the timeline, and the documentation says so where the Python
client says "never block in a routine".

### `base/clock.ts`

`TempoClock(tempo, { timebase, ticker })`:

- the queue is the core's `Scheduler`; TS holds only `id → [item, pending]`,
  the strong reference while an item is queued (the Python client's shape).
- `sched(delayBeats, item)`, `schedAbs(beat, item)`, `play(routine, quant)`,
  `unsched(item)`, `clear()`.
- `beats()`, `beats2secs()`, `secs2beats()`, `setTempo()` (pinning the current
  instant), `bar()`, `beatInBar()` — all arithmetic through the core.
- `start()` / `stop()`: the real-time drive. Each turn pops what is due, resumes
  it, and asks the `Ticker` to wake when the next item comes due.

Around each wake the clock sets a module-level **current routine**, which is what
lets `Event().play()` inside a routine find its exact logical beat and its rng
with no parameters, as `main.current_tt` does in Python. This is sound because a
wake is synchronous on the page's single thread.

There is no NRT/score drive: the TS client has no score interface, and test
determinism comes from the seams (below) rather than from a second drive.

### `base/timebase.ts`

`Timebase` is `{ kind, now(): number }`. Two kinds:

- `MonotonicTimebase` — `performance.now() / 1000`. Paces sleeps; events go out
  as NTP-timetagged bundles.
- `SampleTimebase` — `{ now(), currentSample(), sampleAt(secs), sampleRate }`.
  Events go out as `/sched <absolute sample>`: drift-free and sample-exact.

### `defs/server.ts` — the timed-send path

W1 deliberately left this to W3 ("logical time belongs to the bundle path, which
a later milestone brings"). It grows:

- `sendBundle(messages, { delayBeats, clock })` — stamps at the running
  routine's exact logical beat plus `latency`. Under a monotonic timebase that
  is an NTP timetag; under a sample timebase it is `/sched` with the absolute
  sample, the conversion coming from the core so client and server round
  identically.
- `sendBundleAfter(delaySecs, messages)` — the clockless counterpart, for the
  release of a note played outside any routine.
- `playEvent(event)` — the OSC side of the event's double dispatch: `/s_new`
  plus its release (`gate 0` when the event releases by gate, else `/n_free`).
- `sampleTimebase()` — see below.

### The clock never talks to the server

Here the design **departs from the Python client on purpose**. There,
`clock.lock_to(server)` has the clock open `/clock` round trips against a
server, which contradicts the client's own rule (the lesson recorded as C5: the
clock must not talk to the server). The relation is inverted:

```ts
const tb = await server.sampleTimebase();
const clock = new TempoClock(2.0, { timebase: tb });
```

`Server.sampleTimebase()` resolves by carrier, because the server is the object
that knows its connection:

- **in-page** — the engine lives in this page's `AudioContext`, so one anchor
  (the engine's `clock()` alongside the worklet's `currentFrame`) yields an
  exact integer offset; from there `AudioContext.currentTime` gives the sample
  count synchronously, with no drift by construction.
- **WebSocket** — a small tracker feeds `/clock` anchors into the core's
  `SampleClockModel`, which is the same model the Python client's `lock_to`
  uses.

The clock receives an object with a `now()` and knows nothing else. A server
that does not answer degrades to the monotonic timebase, so a client with no
reachable master keeps working.

### `seq/`

Ports of `clausters/seq`, mirroring them module for module:

- `event.ts` — `Event` over the same `DEFAULTS`, the derived quantities
  (`midinote`/`freq`/`delta`/`sustain`) computed through the core's builtins and
  `degree_to_midinote`, `play(destination)` double dispatch, `free()`,
  `release()`, and `rest()`.
- `pattern.ts` — `Pattern` plus `Pseq`, `Pser`, `Prand`, `Pwhite`, `Pseries`,
  `Pgeom`, `Pfunc`, `Pn`, `Pconst`, and `Pbind`. Generator functions under the
  hood, so nesting and embedding work as they do in Python.
- `eventstream.ts` — `EventStreamPlayer`.
- `timeline.ts` — `Timeline` (a static, editable, beat-sorted list with random
  access) and `Playhead` (play/stop/locate/loop), plus `OscEvent`. `MidiEvent`
  waits for W4's MIDI destination.

### `base/rand.ts` and `base/builtins.ts`

`rand.ts` is the random context: a draw uses the generator of the routine
running right now, else the module root, and `seed(n)` reproduces a script.
`builtins.ts` is the unary/binary catalogue over the core (`midicps`, `cpsmidi`,
`dbamp`, `ampdb`, …), accepting a number or an array as the Python client does.

## Testing

- **Parity vectors** — `tests/gen-clock-vectors.py` → `tests/clock-vectors.json`,
  the pattern the OSC/def/GuiDef vectors already established: beat arithmetic,
  the bar grid, seconds↔samples, timetag bits, `degree_to_midinote`, RNG
  sequences from a seed, and builtin values. Asserted in `node --test`, so a
  divergence from the Python client is a failing test rather than a rumour.
- **The driver, deterministically** — the `Ticker` and `Timebase` seams are
  replaced by manual implementations in `node --test`: the same code path as
  real time, advanced by hand. That is where relational exactness is asserted (a
  `yield 0.25` lands at 0.25 even when the wake-up is late), along with `quant`,
  `unsched`/`clear`, `Pbind` yielding events at the right beats, and
  `Timeline`/`Playhead` locate and loop.
- **The bytes** — a fake `Connection` captures the packets and they are decoded:
  this is where "exact timing" becomes a hard assertion (the timetag, or the
  `/sched` sample, of every bundle).
- **End to end** — a WS suite against a real `clausters --ws` (skipping itself
  when the binary is absent, as the W1/W2 suites do), and a headless-Chrome
  acceptance page `tests/seq.html`: the in-page engine, the sample timebase, a
  pattern playing, the verdict beaconed through the access log like every other
  web smoke.

## Out of scope, explicitly

- `seq/automation.ts` — the break-point control curve. It pulls in buffers,
  `Env` and a control def; it is not in the milestone's module tree.
- `MidiEvent` and MIDI destinations — W4.
- `joinTransport` / the shared `/transport` grid — multi-client phase
  alignment, not part of this acceptance.
- An NRT/score drive — the TS client has no score interface.

## Deliverables

Code and tests as above, plus, per the project's closing rule: the `W3`
checkbox and a "What shipped" note in `clients/web/PLAN.md`; a
`docs/decisions.md` entry for the two non-obvious choices (the driver stays on
the page while the wake-up moves to a Worker; the server anchors the timebase
instead of the clock locking to the server); a commented example under
`clients/web/examples/`; and the `clients/web/README.md` module map extended.

## Acceptance

The milestone's own: a routine schedules events that play with exact relational
timing under both timebases, over either carrier, matching the Python client's
behaviour on the shared vectors.
