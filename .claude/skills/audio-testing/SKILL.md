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
