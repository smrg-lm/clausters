# W3 sequencing — implementation plan

Executes `specs/2026-07-26-web-w3-sequencing-design.md`. Tasks are ordered by
dependency; each ends with a green `./test.sh` slice and a commit.

**Goal:** the TypeScript client gains its timing layer — a core-backed
`TempoClock` resuming generator routines, events/patterns/timelines on it, and
the timed-send path on `Server`.

**Tech stack:** Rust (`clausters-core-web`, wasm-bindgen), TypeScript 7 emitted
by `tsc`, `node --test`, headless Chrome for the page acceptance.

## Global constraints

- Every time/value transformation comes from `clausters-core` through the wasm
  door. No arithmetic on beats, samples, timetags or random values in TS.
- The wasm exports are `#[cfg(target_arch = "wasm32")]`; the crate must keep
  building and testing natively.
- `cargo fmt` + `cargo clippy --all-targets` clean before any Rust commit.
- TS: `npm run check` (type-check of src + tests) clean before any commit.
- Naming: CamelCase types, an acronym takes only its first letter uppercase.
- Prose uses the API's verbs: a node is **freed**, a def **sent**, an element
  **rendered**.

---

### Task 1 — The wasm core doors

**Files:** modify `crates/clausters-core-web/src/lib.rs`.

**Produces:** `Scheduler` (`push`, `peekTime`, `popDue`, `remove`, `len`,
`clear`), `beats_to_secs(tempo, baseBeats, baseSecs, beats)`,
`secs_to_beats(...)`, `secs_to_samples(secs, rate)`, `samples_to_secs`,
`quant_delay(pos, quant)`, `bar`, `beat_in_bar`, `unix_to_ntp`,
`unix_to_sample(unix, anchorUnix, anchorSample, rate)`,
`osc_encode_bundle(unixSecs, messages)`, `osc_encode_immediate_bundle`,
`Rng` (`fromSeed`, `state`, `nextF64`, `uniform`, `nextBelow`, `nextU64`,
`spawn`), `unary(op, x)`, `binary(op, a, b)` by name,
`degree_to_midinote(degree, octave, root, scale)`, `SampleClockModel`
(`addAnchor`, `sampleAt`, `localTimeOf`, `driftPpm`, `rate`, `len`).

- [ ] Add the exports, mirroring `clausters-ffi`'s shapes.
- [ ] `cargo fmt`, `cargo clippy --all-targets -p clausters-core-web`.
- [ ] `./build.sh` stages `dist/core/` + `src/core/` (the `.d.ts` is the proof
      the door opened).
- [ ] Commit.

### Task 2 — Parity vectors, builtins, the random context

**Files:** create `clients/web/tests/gen-clock-vectors.py`,
`clients/web/tests/clock-vectors.json`, `clients/web/tests/clock-parity.test.ts`,
`clients/web/src/base/builtins.ts`, `clients/web/src/base/rand.ts`.

**Consumes:** Task 1. **Produces:** `midicps`/`cpsmidi`/`dbamp`/`ampdb`/… over
number|number[]; `currentRng()`, `spawnRng()`, `seed(n)`, `uniform`, `choice`,
`nextF64`.

- [ ] Generate the vectors from the Python client (beat arithmetic, bar grid,
      seconds↔samples, timetag bits, `degree_to_midinote`, RNG runs, builtins).
- [ ] Write `clock-parity.test.ts` asserting each vector; run it — it fails.
- [ ] Write `builtins.ts` and `rand.ts` over the core; run — it passes.
- [ ] Commit.

### Task 3 — Streams and routines

**Files:** create `clients/web/src/base/stream.ts`,
`clients/web/tests/stream.test.ts`.

**Produces:** `Stream`, `Routine` (over `function*`, `next(inval)`, `reset()`,
`state`, `clock`, `logicalBeat`, `rng`), `FunctionStream`, `StopStream`.

- [ ] Failing test: a routine yields its delays in order, ends by `StopStream`,
      `reset()` restarts it, and each routine gets its own rng.
- [ ] Implement, run, commit.

### Task 4 — The clock, the timebases, the ticker

**Files:** create `clients/web/src/base/timebase.ts`,
`clients/web/src/base/clock.ts`, `clients/web/src/base/tick-worker.ts`,
`clients/web/tests/clock.test.ts`.

**Produces:** `Timebase`, `MonotonicTimebase`, `SampleTimebase`,
`manualTimebase()`; `Ticker`, `timerTicker()`, `workerTicker()`,
`manualTicker()`; `TempoClock` (`sched`, `schedAbs`, `play`, `unsched`,
`clear`, `beats`, `beats2secs`, `secs2beats`, `setTempo`, `bar`, `beatInBar`,
`start`, `stop`, `startTime`, `pacingOrigin`, `timebase`), `currentRoutine()`.

- [ ] Failing tests over the manual seams: yield-exact relational timing under a
      late wake-up, `quant` snapping, `unsched` leaving the rest, tempo change
      pinning the instant.
- [ ] Implement, run, commit.

### Task 5 — The timed-send path on `Server`

**Files:** modify `clients/web/src/defs/server.ts`; create
`clients/web/tests/timed-send.test.ts`.

**Produces:** `Server.sendBundle(messages, {delayBeats, clock})`,
`sendBundleAfter(delaySecs, messages)`, `playEvent(event)`,
`sampleTimebase({timeout})`.

- [ ] Failing test: a fake `Connection` captures the packets; decode them and
      assert the timetag under a monotonic timebase and the `/sched` absolute
      sample under a sample timebase.
- [ ] Implement, run, commit.

### Task 6 — Events, patterns, the player

**Files:** create `clients/web/src/seq/event.ts`, `pattern.ts`,
`eventstream.ts`, `index.ts`; `clients/web/tests/seq.test.ts`.

**Produces:** `Event`, `rest()`, `Pattern`, `Pseq`, `Pser`, `Prand`, `Pwhite`,
`Pseries`, `Pgeom`, `Pfunc`, `Pn`, `Pconst`, `Pbind`, `EventStreamPlayer`.

- [ ] Failing tests: the derived quantities (`midinote`/`freq`/`delta`/
      `sustain`), pattern values under a seeded root, `Pbind` events, and the
      packets a played pattern emits at the right beats.
- [ ] Implement, run, commit.

### Task 7 — Timeline and playhead

**Files:** create `clients/web/src/seq/timeline.ts`; extend
`clients/web/tests/seq.test.ts`.

**Produces:** `Timeline` (`add`, `remove`, `move`, `indexAt`, `range`,
`duration`), `Playhead` (`play`, `stop`, `locate`, `loop`), `OscEvent`.

- [ ] Failing tests: ordered insertion, random access by beat, and a playhead
      that locates and loops, emitting each item once per pass.
- [ ] Implement, run, commit.

### Task 8 — End to end

**Files:** create `clients/web/tests/seq.html`,
`clients/web/examples/sequencing.html`; modify `clients/web/test.sh`,
`clients/web/tests/server.test.ts` (the WS leg).

- [ ] A WS suite: a routine plays a pattern against a real `clausters --ws`,
      skipping itself when the binary is absent.
- [ ] `tests/seq.html`: the in-page engine, `server.sampleTimebase()`, a pattern
      playing; the verdict beaconed through the access log.
- [ ] A commented example. Run `./test.sh` whole; commit.

### Task 9 — Close the milestone

**Files:** modify `clients/web/PLAN.md`, `docs/decisions.md`,
`clients/web/README.md`.

- [ ] The W3 checkbox and a "What shipped" note.
- [ ] A `docs/decisions.md` entry: the driver stays on the page while the
      wake-up moves to a Worker; the Server anchors the timebase rather than the
      clock locking to the server.
- [ ] The README module map. Commit.
