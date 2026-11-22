//! The grains held against the theory: a grain is its window over its
//! source, panned at equal power, round a ring between the two channels
//! either side; the limit is the UGen's; `TGrains` centres a grain on its
//! position; `Warp1` reads at its rate and keeps making grains past its
//! limit, where scsynth's stops for good. `sc_reference` holds the rest
//! against scsynth.

#![cfg(feature = "synth")]

use std::sync::Arc;

use crate::bench::{Run, render_def};
use clausters::dsp::buffer::Buffer;
use serde_json::{Value, json};

const SR: f32 = 48_000.0;

fn c(v: f32) -> Value {
    json!({ "const": v })
}

/// `n` samples of each channel `chans` rows of `kind` write, over `buffers`.
fn rows(
    kind: &str,
    ins: &[Value],
    chans: usize,
    n: usize,
    buffers: &[Option<Arc<Buffer>>],
) -> Vec<Vec<f32>> {
    let mut ugens = vec![];
    for ch in 0..chans {
        let mut inputs = ins.to_vec();
        inputs.push(c(ch as f32));
        ugens.push(json!({"kind": kind, "inputs": inputs}));
    }
    for ch in 0..chans {
        ugens.push(json!({"kind": "Out", "inputs": [c(ch as f32), {"ugen": ch}]}));
    }
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    let run = Run {
        buffers,
        ..Run::new(n).channels(chans)
    };
    render_def(&def, &run)
}

/// One `GrainSin` grain is a Hann of its length over a sine from phase 0.
#[test]
fn a_grain_is_its_window_over_its_source() {
    let dur = 0.01f32;
    let f = 1000.0f32;
    let y = rows(
        "GrainSin",
        &[c(1.0), c(dur), c(f), c(0.0), c(-1.0), c(512.0), c(1.0)],
        1,
        1024,
        &[],
    )
    .remove(0);
    let len = dur as f64 * SR as f64;
    for (k, &v) in y.iter().enumerate().take(len as usize) {
        let w = (std::f64::consts::PI * (k + 1) as f64 / len).sin().powi(2);
        let s = (std::f64::consts::TAU * f as f64 * k as f64 / SR as f64).sin();
        assert!(
            (v as f64 - w * s).abs() < 1e-5,
            "{k}: {v} against {}",
            w * s
        );
    }
    assert!(
        y[len as usize..].iter().all(|&v| v == 0.0),
        "and nothing after"
    );
}

/// A grain's channels hold its power on a pair, and on a ring it sounds in
/// the two channels either side of its pan, at equal power between them.
#[test]
fn a_grain_pans_at_equal_power() {
    for pan in [-1.0f32, -0.3, 0.0, 0.6] {
        let ch = rows(
            "GrainIn",
            &[c(1.0), c(0.01), c(1.0), c(pan), c(-1.0), c(512.0), c(2.0)],
            2,
            256,
            &[],
        );
        let k = 200;
        let w = (std::f64::consts::PI * (k + 1) as f64 / 480.0)
            .sin()
            .powi(2);
        let p = (ch[0][k] as f64).powi(2) + (ch[1][k] as f64).powi(2);
        assert!((p - w * w).abs() < 1e-6, "pan {pan}: {p} against {}", w * w);
    }
    // A pan of 0.5 on four: a quarter turn, half-way between channels 1
    // and 2.
    let ch = rows(
        "GrainIn",
        &[c(1.0), c(0.01), c(1.0), c(0.5), c(-1.0), c(512.0), c(4.0)],
        4,
        256,
        &[],
    );
    assert!(ch[0].iter().chain(&ch[3]).all(|&v| v == 0.0));
    assert!((ch[1][200] - ch[2][200]).abs() < 1e-7 && ch[1][200] > 0.1);
}

/// The limit is the UGen's, whatever channel a grain sounds in: with one
/// grain allowed, a trigger while it sounds on the other channel starts
/// nothing on this one.
#[test]
fn the_grain_limit_is_the_whole_ugens() {
    // Grains at 0 and 100 samples, panned hard left then hard right.
    let def = |max: f32| {
        json!({"name": "t", "ugens": [
            {"kind": "Impulse", "inputs": [c(480.0), c(0.0)]},
            {"kind": "Phasor", "inputs": [c(0.0), c(1.0), c(0.0), c(1e6), c(0.0)]},
            {"kind": "BinaryOpUGen", "op": "gt", "inputs": [{"ugen": 1}, c(50.0)]},
            {"kind": "MulAdd", "inputs": [{"ugen": 2}, c(2.0), c(-1.0)]},
            {"kind": "GrainIn", "inputs": [{"ugen": 0}, c(0.01), c(1.0), {"ugen": 3}, c(-1.0), c(max), c(2.0), c(1.0)]},
            {"kind": "Out", "inputs": [c(0.0), {"ugen": 4}]}
        ]})
        .to_string()
    };
    let right = |max| render_def(&def(max), &Run::new(400)).remove(0);
    assert!(
        right(1.0)[150..400].iter().all(|&v| v == 0.0),
        "the left grain holds the only place"
    );
    assert!(
        right(2.0)[150..400].iter().any(|&v| v > 0.1),
        "with two, the right one sounds"
    );
}

/// `TGrains` centres a grain on `centerPos`: over a buffer whose frame `k`
/// holds `k`, the grain's middle sample reads the centre's frame.
#[test]
fn tgrains_centres_its_grain() {
    let frames = 4000;
    let ramp: Vec<f32> = (0..frames).map(|k| k as f32).collect();
    let buf = vec![Some(Arc::new(Buffer::new(ramp, 1, frames, SR as f64)))];
    let center = 0.05f32; // frame 2400
    let dur = 0.01f32; // 480 samples
    let y = rows(
        "TGrains",
        &[
            c(1.0),
            c(0.0),
            c(1.0),
            c(center),
            c(dur),
            c(0.0),
            c(1.0),
            c(2.0),
            c(1.0),
        ],
        1,
        512,
        &buf,
    )
    .remove(0);
    // The window peaks at its middle, where its value is exactly 1 at a
    // half length: the sample there is the frame.
    let mid = 239;
    let w = (std::f64::consts::PI * (mid + 1) as f64 / 480.0)
        .sin()
        .powi(2);
    let frame = y[mid] as f64 / w;
    assert!((frame - 2400.0).abs() < 0.6, "{frame}");
}

/// `Warp1` reads a sine at `freqScale` times its pitch, and goes on making
/// grains with more overlaps than it holds. At a frequency its grains read
/// in phase: they start 600 samples apart from one place, ten periods of
/// the octave up.
#[test]
fn warp1_reads_at_its_rate_and_keeps_going() {
    let frames = 48_000;
    let f = 400.0;
    let sine: Vec<f32> = (0..frames)
        .map(|k| (std::f64::consts::TAU * f * k as f64 / SR as f64).sin() as f32)
        .collect();
    let buf = vec![Some(Arc::new(Buffer::new(sine, 1, frames, SR as f64)))];
    let y = rows(
        "Warp1",
        &[
            c(0.0),
            c(0.3),
            c(2.0),
            c(0.05),
            c(-1.0),
            c(4.0),
            c(0.0),
            c(4.0),
        ],
        1,
        24_000,
        &buf,
    )
    .remove(0);
    let power = |x: &[f32], freq: f64| {
        let w = std::f64::consts::TAU * freq / SR as f64;
        let k = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0, 0.0);
        for &v in x {
            let s = v as f64 + k * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        (s1 * s1 + s2 * s2 - k * s1 * s2) / (x.len() as f64).powi(2)
    };
    let tail = &y[4800..];
    assert!(
        power(tail, 2.0 * f) > 100.0 * power(tail, f),
        "an octave up"
    );
    // 200 overlaps of 64 grains at most: still sounding a second on.
    let y = rows(
        "Warp1",
        &[
            c(0.0),
            c(0.3),
            c(1.0),
            c(0.05),
            c(-1.0),
            c(200.0),
            c(0.0),
            c(1.0),
        ],
        1,
        48_000,
        &buf,
    )
    .remove(0);
    assert!(
        y[40_000..].iter().any(|v| v.abs() > 0.1),
        "still making grains"
    );
}
