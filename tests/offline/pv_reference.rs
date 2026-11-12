//! Every curated `PV_*` operation, and a `PV_Kernel` phase program, held
//! sample for sample to a reference: the chain's weighted overlap-add done
//! here, frame by frame on the same grid with the same window, with the
//! operation written again from its definition. A level error, a bin off by
//! one, a frame misplaced or a DC/Nyquist slot mishandled all show as a
//! difference far above the f32 rounding the tolerance allows.
//!
//! The inputs are known samples (noise plus a tone, read by `PlayBuf`), not
//! oscillators, so the reference sees exactly what the chain saw.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::clausters_core::fft::{irfft_into, rfft_into};
use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::clausters_core::window::Window;
use clausters::dsp::buffer::Buffer;
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use serde_json::{Value, json};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
/// The chains' window and hop: a 512-point Hann at a 50% hop.
const N: usize = 512;
const HOP: usize = 256;
const HALF: usize = N / 2;
/// Samples rendered.
const LEN: usize = 150 * BLOCK_SIZE;
/// A node id a multiple of the hop's block count, so the hop-phase stagger is
/// 0 and the first frame ends at `N`.
const NODE: i32 = 8;

/// Deterministic input: noise at 0.3 plus a 0.3 tone at `freq`, so some bins
/// stand far above the rest.
fn input(seed: u32, freq: f32) -> Vec<f32> {
    let mut state = seed;
    (0..LEN)
        .map(|n| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 8) as f32 / (1 << 23) as f32 - 1.0;
            0.3 * noise + 0.3 * (std::f32::consts::TAU * freq * n as f32 / SR).sin()
        })
        .collect()
}

fn inputs() -> (Vec<f32>, Vec<f32>) {
    (input(12_345, 1500.0), input(999, 4200.0))
}

/// Renders a def whose UGens 0 and 1 play buffers 0 and 1 (`a`, `b`), 2 and
/// 3 are their `FFT`s, then `middle`, then an `IFFT` of the last UGen and an
/// `Out`. `events` are timed commands for the synth.
fn render(a: &[f32], b: &[f32], controls: Value, middle: Vec<Value>, events: Vec<Cmd>) -> Vec<f32> {
    let play = |buf: f32| {
        json!({"kind": "PlayBuf", "inputs": [
            {"const": buf}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
            {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]})
    };
    let fft = |src: usize| {
        json!({"kind": "FFT", "inputs": [{"ugen": src}, {"const": 1.0}],
               "fft_size": N, "hop": 0.5})
    };
    let mut ugens = vec![play(0.0), play(1.0), fft(0), fft(1)];
    ugens.extend(middle);
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "IFFT", "inputs": [{"ugen": last}]}));
    ugens.push(json!({"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": last + 1}]}));
    let spec: SynthDefSpec =
        serde_json::from_value(json!({"name": "pv", "controls": controls, "ugens": ugens}))
            .unwrap();
    let synth = UGenSynth::new(
        Arc::new(clausters::synthdef::compile(spec).unwrap()),
        SR,
        SEED_STRIDE,
    );

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    for (index, samples) in [a, b].into_iter().enumerate() {
        handle
            .send(Cmd::SetBuffer {
                index,
                buffer: Some(Arc::new(Buffer::new(
                    samples.to_vec(),
                    1,
                    samples.len(),
                    SR as f64,
                ))),
            })
            .ok()
            .unwrap();
    }
    engine.process_block(&mut out);
    handle.collect_garbage();
    let start = BLOCK_SIZE as u64;
    handle
        .send(Cmd::Schedule {
            time: start,
            cmds: vec![Cmd::AddSynth {
                id: NODE,
                target: ROOT_NODE_ID,
                action: AddAction::Tail,
                synth: Box::new(synth),
                usage: Default::default(),
            }],
        })
        .ok()
        .unwrap();
    for cmd in events {
        let Cmd::Schedule { time, cmds } = cmd else {
            panic!("events are timed")
        };
        handle
            .send(Cmd::Schedule {
                time: time + start,
                cmds,
            })
            .ok()
            .unwrap();
    }
    let mut sig = Vec::with_capacity(LEN);
    for _ in 0..LEN / BLOCK_SIZE {
        engine.process_block(&mut out);
        sig.extend(out.iter().step_by(CHANNELS).copied());
    }
    sig
}

/// One bin of a packed frame, `0..=HALF` (DC and Nyquist real-only).
fn get(f: &[f32], k: usize) -> (f32, f32) {
    match k {
        0 => (f[0], 0.0),
        HALF => (f[1], 0.0),
        _ => (f[2 * k], f[2 * k + 1]),
    }
}

fn set(f: &mut [f32], k: usize, (re, im): (f32, f32)) {
    match k {
        0 => f[0] = re,
        HALF => f[1] = re,
        _ => {
            f[2 * k] = re;
            f[2 * k + 1] = im;
        }
    }
}

fn mag((re, im): (f32, f32)) -> f32 {
    (re * re + im * im).sqrt()
}

/// The chain's weighted overlap-add, done here: frames ending at `N`, `N +
/// HOP`, ... over `a` and `b`, each windowed and transformed, `op(end, frame
/// a, frame b)` applied to frame a, inverse-transformed, windowed again and
/// added at its place, the sum divided by the window's steady-state overlap.
/// Indexed by input position; the chain outputs it one window late.
fn reference(a: &[f32], b: &[f32], mut op: impl FnMut(usize, &mut [f32], &[f32])) -> Vec<f32> {
    let mut w = vec![0.0f32; N];
    Window::Hann.fill(&mut w);
    let norm: Vec<f32> = (0..HOP)
        .map(|r| (r..N).step_by(HOP).map(|k| w[k] * w[k]).sum())
        .collect();
    let mut acc = vec![0.0f32; LEN];
    let (mut fa, mut fb, mut t) = (vec![0.0f32; N], vec![0.0f32; N], vec![0.0f32; N]);
    let mut end = N;
    while end <= LEN {
        let s = end - N;
        let wa: Vec<f32> = (0..N).map(|k| a[s + k] * w[k]).collect();
        let wb: Vec<f32> = (0..N).map(|k| b[s + k] * w[k]).collect();
        rfft_into(&wa, &mut fa);
        rfft_into(&wb, &mut fb);
        op(end, &mut fa, &fb);
        irfft_into(&fa, &mut t);
        for k in 0..N {
            acc[s + k] += t[k] * w[k];
        }
        end += HOP;
    }
    (0..LEN).map(|j| acc[j] / norm[j % HOP]).collect()
}

/// Asserts the chain's output is the reference one window late, over the
/// whole render but the last window (whose frames the reference has and the
/// chain has not output yet).
fn assert_matches(name: &str, got: &[f32], want: &[f32]) {
    let peak = want.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak > 0.05, "{name}: the reference is silent ({peak})");
    let err = (N..LEN)
        .map(|t| (got[t] - want[t - N]).abs())
        .fold(0.0f32, f32::max);
    assert!(err < 1e-5, "{name}: max error {err} (peak {peak})");
}

/// One filter UGen of `kind` on chain A with constant inputs `params`.
fn filter(kind: &str, params: &[f32]) -> Value {
    let mut inputs = vec![json!({"ugen": 2})];
    inputs.extend(params.iter().map(|p| json!({"const": p})));
    json!({"kind": kind, "inputs": inputs})
}

#[test]
fn the_plain_round_trip_matches() {
    let (a, b) = inputs();
    // With nothing in between, the IFFT reads the last UGen, chain B's FFT.
    let got = render(&a, &b, json!([]), vec![], vec![]);
    assert_matches("round trip", &got, &reference(&b, &a, |_, _, _| {}));
}

#[test]
fn the_magnitude_gates_match() {
    let (a, b) = inputs();
    // Thresholds that bite, and ones that clear every bin (transparent).
    for (kind, th) in [
        ("PV_MagAbove", 5.0f32),
        ("PV_MagBelow", 5.0),
        ("PV_MagClip", 5.0),
        ("PV_MagAbove", 0.0),
        ("PV_MagClip", 1.0e9),
    ] {
        let got = render(&a, &b, json!([]), vec![filter(kind, &[th])], vec![]);
        let want = reference(&a, &b, |_, f, _| {
            for k in 0..=HALF {
                let (re, im) = get(f, k);
                let m = mag((re, im));
                match kind {
                    "PV_MagAbove" if m < th => set(f, k, (0.0, 0.0)),
                    "PV_MagBelow" if m > th => set(f, k, (0.0, 0.0)),
                    "PV_MagClip" if m > th => set(f, k, (re * th / m, im * th / m)),
                    _ => {}
                }
            }
        });
        assert_matches(kind, &got, &want);
    }
}

#[test]
fn the_brick_wall_matches() {
    let (a, b) = inputs();
    for wipe in [0.3f32, -0.3, 0.97] {
        let got = render(
            &a,
            &b,
            json!([]),
            vec![filter("PV_BrickWall", &[wipe])],
            vec![],
        );
        let nbins = (HALF + 1) as f32;
        let want = reference(&a, &b, |_, f, _| {
            let zeroed = |k: usize| {
                if wipe > 0.0 {
                    k >= (nbins * (1.0 - wipe)).round() as usize
                } else {
                    k < (nbins * -wipe).round() as usize
                }
            };
            for k in (0..=HALF).filter(|&k| zeroed(k)) {
                set(f, k, (0.0, 0.0));
            }
        });
        assert_matches(&format!("PV_BrickWall {wipe}"), &got, &want);
    }
}

#[test]
fn the_combiners_match() {
    let (a, b) = inputs();
    for kind in [
        "PV_Add",
        "PV_Mul",
        "PV_Min",
        "PV_Max",
        "PV_MagMul",
        "PV_CopyPhase",
    ] {
        let combine = json!({"kind": kind, "inputs": [{"ugen": 2}, {"ugen": 3}]});
        let got = render(&a, &b, json!([]), vec![combine], vec![]);
        let want = reference(&a, &b, |_, fa, fb| {
            for k in 0..=HALF {
                let (x, y) = (get(fa, k), get(fb, k));
                let r = match kind {
                    "PV_Add" => (x.0 + y.0, x.1 + y.1),
                    "PV_Mul" => (x.0 * y.0 - x.1 * y.1, x.0 * y.1 + x.1 * y.0),
                    "PV_Min" => {
                        if mag(y) < mag(x) {
                            y
                        } else {
                            x
                        }
                    }
                    "PV_Max" => {
                        if mag(y) > mag(x) {
                            y
                        } else {
                            x
                        }
                    }
                    "PV_MagMul" => (x.0 * mag(y), x.1 * mag(y)),
                    _ => {
                        // A's magnitude on B's phase.
                        let (ma, mb) = (mag(x), mag(y));
                        if mb > 0.0 {
                            (y.0 * ma / mb, y.1 * ma / mb)
                        } else {
                            (ma, 0.0)
                        }
                    }
                };
                set(fa, k, r);
            }
        });
        // A product of two spectra is much louder than either: hold it to
        // the same relative rounding.
        let scale = want.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1.0);
        let got: Vec<f32> = got.iter().map(|x| x / scale).collect();
        let want: Vec<f32> = want.iter().map(|x| x / scale).collect();
        assert_matches(kind, &got, &want);
    }
}

#[test]
fn the_smear_matches() {
    let (a, b) = inputs();
    for bins in [0usize, 1, 4] {
        let got = render(
            &a,
            &b,
            json!([]),
            vec![filter("PV_MagSmear", &[bins as f32])],
            vec![],
        );
        let want = reference(&a, &b, |_, f, _| {
            let mags: Vec<f32> = (0..=HALF).map(|k| mag(get(f, k))).collect();
            for k in 0..=HALF {
                let (lo, hi) = (k.saturating_sub(bins), (k + bins).min(HALF));
                let avg = mags[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32;
                let (re, im) = get(f, k);
                let m = mags[k];
                set(
                    f,
                    k,
                    if m > 0.0 {
                        (re * avg / m, im * avg / m)
                    } else {
                        (avg, 0.0)
                    },
                );
            }
        });
        // The engine sums the magnitudes as a running prefix, the reference
        // window by window: the same values, added in another order.
        assert_matches(&format!("PV_MagSmear {bins}"), &got, &want);
    }
}

#[test]
fn the_bin_shifts_match() {
    let (a, b) = inputs();
    for (kind, stretch, shift) in [
        ("PV_BinShift", 1.0f32, 0.0f32),
        ("PV_BinShift", 1.0, 5.0),
        ("PV_BinShift", 1.5, -3.0),
        ("PV_MagShift", 0.75, 2.0),
    ] {
        let got = render(
            &a,
            &b,
            json!([]),
            vec![filter(kind, &[stretch, shift])],
            vec![],
        );
        let want = reference(&a, &b, |_, f, _| {
            let src: Vec<(f32, f32)> = (0..=HALF).map(|k| get(f, k)).collect();
            let mut moved = vec![(0.0f32, 0.0f32); HALF + 1];
            for (k, &bin) in src.iter().enumerate() {
                let t = (k as f32 * stretch + shift).round();
                if t < 0.0 || t > HALF as f32 {
                    continue;
                }
                let t = t as usize;
                if kind == "PV_MagShift" {
                    moved[t].0 += mag(bin);
                } else {
                    // A bin landing on DC or Nyquist keeps only its real part.
                    let im = if t == 0 || t == HALF { 0.0 } else { bin.1 };
                    moved[t] = (moved[t].0 + bin.0, moved[t].1 + im);
                }
            }
            for k in 0..=HALF {
                let r = if kind == "PV_MagShift" {
                    let (re, im) = src[k];
                    let m = mag(src[k]);
                    let target = moved[k].0;
                    if m > 0.0 {
                        (re * target / m, im * target / m)
                    } else {
                        (target, 0.0)
                    }
                } else {
                    moved[k]
                };
                set(f, k, r);
            }
        });
        assert_matches(&format!("{kind} {stretch} {shift}"), &got, &want);
    }
}

/// `PV_MagFreeze` switched on at a frame boundary: the frames up to it pass,
/// every later one carries the last open frame's magnitudes on its own
/// phases.
#[test]
fn the_freeze_matches() {
    // A multiple of the hop and of the block: frames ending at or before it
    // see the freeze off, every later one sees it on.
    const AT: usize = 20 * HOP;
    let (a, b) = inputs();
    let freeze = json!({"kind": "PV_MagFreeze", "inputs": [{"ugen": 2}, {"control": 0}]});
    let got = render(
        &a,
        &b,
        json!([{"name": "freeze", "default": 0.0}]),
        vec![freeze],
        vec![Cmd::Schedule {
            time: AT as u64,
            cmds: vec![Cmd::SetControl {
                id: NODE,
                index: 0,
                value: 1.0,
            }],
        }],
    );
    let mut held = vec![0.0f32; HALF + 1];
    let want = reference(&a, &b, |end, f, _| {
        for (k, h) in held.iter_mut().enumerate() {
            let (re, im) = get(f, k);
            let m = mag((re, im));
            if end <= AT {
                *h = m;
            } else if m > 0.0 {
                set(f, k, (re * *h / m, im * *h / m));
            }
        }
    });
    assert_matches("PV_MagFreeze", &got, &want);
}

/// The polar path of `PV_Kernel`: `phase + pi` negates every bin, so the
/// output is the negated round trip -- to the rounding of an
/// `atan2`/`cos`/`sin` round trip, not merely "close".
#[test]
fn a_kernel_phase_program_matches() {
    let (a, b) = inputs();
    let kernel = json!({"kind": "PV_Kernel", "inputs": [{"ugen": 2}],
                        "phase_expr": ["phase", std::f32::consts::PI, "add"]});
    let got = render(&a, &b, json!([]), vec![kernel], vec![]);
    let want = reference(&a, &b, |_, f, _| {
        for k in 0..=HALF {
            let (re, im) = get(f, k);
            let (m, ph) = (mag((re, im)), im.atan2(re) + std::f32::consts::PI);
            set(f, k, (m * ph.cos(), m * ph.sin()));
        }
    });
    assert_matches("PV_Kernel phase + pi", &got, &want);
}
