---
name: ugen-dsp
description: DSP algorithms for implementing the server's UGens — oscillators (recurrence sine, saw/pulse with PolyBLEP), biquad/one-pole filters with their coefficients, EnvGen with done actions, PlayBuf with interpolation. Consult when writing or reviewing any UGen's process().
---

# UGen DSP

Conventions: `sr` = sample rate, `w = 2π·freq/sr` (normalized angular frequency),
block processing of 64 samples, everything `f32` except phase accumulators (`f64`).

## The trait

```rust
pub struct ProcessCtx { pub sample_rate: f32, pub block_size: usize, /* … */ }

pub trait UGen: Send {
    fn process(&mut self, ctx: &ProcessCtx, inputs: &[&[f32]], outputs: &mut [&mut [f32]]);
    fn done(&self) -> DoneAction { DoneAction::None } // checked after process()
}
```

Inputs may be a signal (a 64-sample slice) or a constant (a 1-sample slice — check
`len()` or resolve it during wiring construction). UGens with modulatable frequency
must read their input per sample.

## Oscillators

### Sine — accumulated phase

Accumulating phase in `f64` and calling `sin()` is correct and sufficient for v1:

```rust
out[i] = (self.phase).sin() as f32;
self.phase += TAU * freq as f64 / sr;
if self.phase >= TAU { self.phase -= TAU; }
```

Later optimization (what scsynth does): an 8192-point wavetable with linear
interpolation. Don't start there.

### Saw / Pulse — PolyBLEP (anti-aliasing)

A naive saw (`2.0 * phase - 1.0` with phase in [0,1)) aliases brutally. PolyBLEP
corrects the jump by subtracting a polynomial in the neighborhood of the
discontinuity:

```rust
fn poly_blep(t: f64, dt: f64) -> f64 {
    if t < dt        { let t = t / dt; t + t - t * t - 1.0 }
    else if t > 1.0 - dt { let t = (t - 1.0) / dt; t * t + t + t + 1.0 }
    else { 0.0 }
}
// saw: out = (2.0*phase - 1.0) - poly_blep(phase, dt)   with dt = freq/sr
// pulse: two phase-shifted saws subtracted (phase and phase+width, mod 1)
```

### WhiteNoise

LCG or xorshift per sample scaled to [-1, 1]. Don't use `rand::thread_rng()` on
the audio thread (it may allocate/lock); RNG state lives inside the UGen, seeded
at construction.

```rust
self.state ^= self.state << 13; self.state ^= self.state >> 17; self.state ^= self.state << 5;
out[i] = (self.state as i32 as f32) * (1.0 / 2147483648.0);
```

## Filters

### OnePole / Lag (parameter smoothing)

```rust
// lag to time t (60 dB): b = (-TAU / (t * sr)).exp()  → 0 if t == 0
y = x + b * (y - x);
```

`Lag` is this applied to a control signal. Essential so `/node_set freq` doesn't
click.

### Biquad (RBJ-cookbook-style LPF/HPF)

Transposed direct form II — the most numerically stable in f32:

```rust
let y = b0 * x + s1;
s1 = b1 * x - a1 * y + s2;
s2 = b2 * x - a2 * y;
```

LPF coefficients (cookbook, Q = 1/√2 for Butterworth):

```text
w0 = 2π·fc/sr;  α = sin(w0)/(2Q);  cw = cos(w0)
b0 = (1-cw)/2 · n   b1 = (1-cw) · n   b2 = b0
a1 = -2cw · n       a2 = (1-α) · n    with n = 1/(1+α)
```

HPF: same with `b0 = (1+cw)/2 · n`, `b1 = -(1+cw) · n`, `b2 = b0`.
Recompute coefficients only when fc changes (not per sample); if fc is a signal,
recomputing per block is fine in v1. Clamp fc to [10, sr·0.45].

## EnvGen

Segment-based envelope: levels `[l0, l1, …]`, durations, curves. Minimum viable:
linear and exponential. State: current segment, samples remaining, current value.

- **Gate**: starts with `gate=1`; when `gate=0`, jump to the release segment
  (defined by `releaseNode`). Enough for ADSR.
- **Done action**: when the last segment ends, `done()` returns the action
  (typically `FreeSelf` = 2). After processing the synth, the engine checks the
  done flags and enqueues the node's release (via the RT-safe mechanism — onto
  the garbage FIFO, never `drop`!).
- Exponential curve: requires same-sign, non-zero levels; per-sample multiplier
  `m = (end/start)^(1/n)`.

## PlayBuf / BufRd

- Read position in `f64` (an `f32` loses sample precision after ~190 s of audio
  at 44.1 kHz — classic bug).
- Per-sample advance: `rate * buf.samplerate / sr` (re-pitch if the file has a
  different SR).
- Minimum linear interpolation: `s = a + frac * (b - a)`; cubic (4-point) as an
  upgrade. Handle the end boundary (loop wrap or zeros + done flag).
- The UGen stores the buffer index and resolves the contents per block from the
  pool — the buffer may be replaced by `/buffer_read` between blocks (via
  `basedrop::Shared`, see [[realtime-audio]]).

## Operators and Out/In

- **BinaryOp** (`+`, `*`, …): one generic UGen with the op as an enum; `*` between
  a signal and a constant is the most common case (amplitude).
- **Out**: `bus[i] += in[i]` (adds, doesn't assign! several synths on the same bus
  mix — scsynth semantics). **ReplaceOut** assigns. **In** copies from the bus.

## General traps

- Initialize all state to 0/initial phase in the constructor; no inherited NaN.
- A NaN propagates through the whole graph: in debug,
  `debug_assert!(out[i].is_finite())` at the end of process().
- Wires and buses are shared between nodes within the same block: never assume an
  input buffer arrives zeroed.
- Denormals in filter tails: see [[realtime-audio]] (FTZ/DAZ).
