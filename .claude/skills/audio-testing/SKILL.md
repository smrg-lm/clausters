---
name: audio-testing
description: How to test the audio server without listening to it — offline (NRT) rendering, signal asserts (frequency, RMS, impulse response), WAV golden files, no-allocation verification on the RT thread, and OSC integration tests. Consult when writing any engine or UGen test.
---

# Testing audio without ears

The principle: the engine must be able to run **without cpal** — an offline mode
that processes N blocks against in-memory buffers. All the logic (node tree,
UGens, buses, scheduling) is tested there; cpal is only tested by hand.

## Testable architecture

```rust
// The engine exposes this; the cpal backend and the tests call it the same way:
impl Engine {
    pub fn process_block(&mut self, out: &mut [f32]) // 64 samples * channels
}
```

Tests build the `Engine` directly (no network, no audio), inject commands through
the same FIFO the network thread would use, and inspect the output.

## Signal asserts

```rust
fn rms(buf: &[f32]) -> f32 { (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt() }

fn zero_crossings(buf: &[f32]) -> usize {
    buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
}
// estimated freq = positive_crossings * sr / len  → assert with 1-2% tolerance
```

- **Oscillators**: frequency via zero crossings; amplitude via peak/RMS (a sine of
  amp 1 → RMS ≈ 0.707). For PolyBLEP saws: compare the energy above Nyquist/2 via
  a hand-rolled DFT over a few bins, or a golden file.
- **Filters**: impulse response (`[1, 0, 0, …]`) compared against expected
  coefficients; or sines at several frequencies verifying attenuation (LPF at fc:
  -3 dB ≈ gain 0.707).
- **Envelopes**: sample key points (value at t=attack should be ≈ peak; after
  release, ≈ 0; done flag set).
- **Silence**: `assert!(rms < 1e-6)` — after `/n_free` the bus must be clean.
- **Always**: `assert!(buf.iter().all(|x| x.is_finite()))` in every test.

Tolerances: compare audio floats with relative tolerance (1e-4 is usually enough
for f32); never exact equality except for structural zeros.

## Measuring instead of pinning (the shared harness)

`tests/common/signal.rs` holds the measurements — include it with
`#[path = "common/signal.rs"] mod signal;`. Its own tests live in
`tests/signal.rs`, each driving a helper with a signal whose answer is known in
closed form.

| helper | estimates |
|---|---|
| `rms`, `peak`, `dc`, `assert_finite` | the basics |
| `dft_at`, `amplitude_at`, `phase_at` | one bin at an **arbitrary** frequency — not the nearest FFT bin |
| `response_at(input, output, hz, sr)` | a filter's gain and phase shift from a real I/O pair |
| `coherent_freq(target, sr, n)` | a frequency that fits a whole (odd) number of periods in `n` samples |
| `alias_snr_db(x, f0, sr)` | harmonic energy vs everything else — an oscillator's antialiasing figure |
| `power_spectrum(x, n, win)` | Welch-averaged power spectrum |
| `spectral_slope_db_per_octave` | dB/octave over octave bands (pink noise = −3.01) |
| `group_delay_samples(x, y, max)` | delay to sub-sample resolution |

**Coherent sampling.** `amplitude_at` and `alias_snr_db` are *exact* only when
the analysis window spans a whole number of periods, which is why the frequency
comes from `coherent_freq` and no window is applied: a rectangular window over a
coherent signal has zero leakage, so a component's energy sits in one bin. The
alternative — windowing a non-coherent signal — buries anything under the
window's sidelobe floor (Blackman, the best `clausters_core` offers, is about
−58 dB, above a decent oscillator's alias floor), so the measurement would
report the window rather than the UGen. `coherent_freq` also snaps to an **odd**
bin, which keeps aliased partials off the harmonic bins; on an even bin they
fold on top of the harmonics and become invisible.

**The rules a UGen test follows:**

1. A filter is asserted against the **analytic transfer function of the
   structure actually implemented**, evaluated in `f64` — never against a golden
   file, never against scsynth's output. State the tolerance in dB.
2. An oscillator **reports its measured alias SNR** at several fundamentals and
   asserts a floor derived from that measurement and written down. A naive
   (non-band-limited) saw measures 30.9 / 16.0 / 9.9 dB at 105 / 996 / 3996 Hz —
   that is the baseline a band-limited one has to beat.
3. Stochastic sources are tested for **distribution** (mean, variance, spectral
   slope) with a fixed seed, plus bit-exact reproducibility from a
   `clausters_core::rng` seed.
4. Every stateful UGen gets a **long-run numerical test** (a resonant filter at
   fc = 20 Hz for 10 s: finite, and its RMS matches the analytic gain). This is
   what catches a regression from `f64` state back to `f32`.
5. Every stateful UGen gets a **block-split test**: rendering in whole blocks and
   in split blocks must agree.
6. Don't assert on a number you cannot derive. If the expected value is not in
   closed form, print it from a run (see `report_the_measured_figures` in
   `tests/signal.rs`) and write the measurement into the doc.

Rules 4 and 5 are the same for every UGen, so they are driven from one table
rather than restated per family — see the bench below. Rules 1–3 are claims
only the family can make, and stay in its own suite.

## The bench: one declaration, asserts and ears

`tests/common/subjects.json` declares the UGens under test — one entry per row,
keyed by milestone: the kind, its wire inputs, what to feed it, and `traits`
that select which generic rules apply (`stateful`, `oscillator`, `stochastic`,
`finite`). **Adding a row to a milestone is one entry there.** Two consumers
read it:

- `tests/common/bench.rs` (`#[path = "common/bench.rs"] mod bench;`) assembles a
  subject into a def and renders it. `render`/`render_split`/`render_with_input`
  replace the private copies each suite used to carry; `assert_split_agrees`
  (rule 5), `assert_long_run_is_bounded` (rule 4) and `assert_renders_finite`
  drive the generic rules. `tests/subjects.rs` runs them over the whole table
  and contrasts every entry against `registry::all()`, so a renamed row or a
  family that gains an input fails loudly instead of quietly shrinking coverage.
- `examples/audition.py` sends the same fragment to a server so you can **hear**
  it: `python3 examples/audition.py saw`, `... U2 --wav /tmp/u2.wav` (offline, no
  hardware), `... rlpf --sweep freq 100 8000`. Its sources are the UGen
  equivalents of the test's sample arrays — same character, different samples.

One thing the def path cannot do: a **stochastic** subject's split test. Each
noise instance seeds from a shared counter and the wire has no seed input, so
two renders differ at sample 0 by design. `assert_split_agrees` refuses those;
`tests/noise.rs` discharges rule 5 for them one level down, against `with_seed`
constructors.

## Golden files

For complete scenes (synth + filter + envelope via OSC commands):

1. The test renders N seconds offline into a `Vec<f32>`.
2. It compares against a reference WAV in `tests/golden/` (loaded with `hound`)
   with per-sample tolerance.
3. Regenerate goldens with an explicit binary/flag (`cargo run --bin
   render_golden`), never automatically — a self-regenerating golden detects
   nothing.
4. Review new goldens by *listening* to them once before committing.

## RT-safety in tests

```toml
[dev-dependencies]
assert_no_alloc = "1"
```

```rust
#[test]
fn processing_does_not_allocate() {
    let mut engine = build_heavy_scene(); // allocate EVERYTHING up front
    let mut out = vec![0.0f32; 128];
    assert_no_alloc::assert_no_alloc(|| {
        for _ in 0..1000 { engine.process_block(&mut out); }
    });
}
```

This test is the design's guardian: if someone sneaks a `Vec::push` or a
`format!` into a UGen, it fails here. Run it over the most complete scene
available (including synths being created and freed — commands go pre-built into
the FIFO from outside the closure).

Note: `assert_no_alloc` needs its global allocator (`#[global_allocator]` with
`AllocDisabler`) in the test binary — it goes in `tests/rt_safety.rs`, not in
`lib.rs`.

## OSC integration

```rust
#[test]
fn s_new_and_status() {
    let server = TestServer::spawn(); // port 0 → ephemeral, offline mode with manual clock
    server.send(msg("/s_new", &["default", 1000, 0, 0]));
    server.tick_blocks(10);
    let reply = server.request(msg("/status", &[]));
    assert_eq!(reply.synth_count(), 1);
}
```

- Ephemeral port (bind to `127.0.0.1:0`) so tests run in parallel.
- The test server advances the "audio" with a manual clock (`tick_blocks`), not in
  real time — bundle scheduling tests set exact timetags and verify at which
  sample the event happened (scene: Out of a DC=1 scheduled → find the first
  non-zero sample).

## By hand (when listening is needed)

```bash
cargo run --release &           # real server with cpal
oscsend localhost 57110 /s_new siii default -1 0 0
# or sclang: s = Server("rust", NetAddr("127.0.0.1", 57110)); — see [[scsynth-osc]]
```

There is no audio device in CI: the cpal backend feature flag must be off in
tests (`--no-default-features` or an opt-in `realtime` feature).
