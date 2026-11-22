//! The table readers, `TWindex` and the small noises held against the
//! theory where scsynth's code is no reference: `IndexL` below the table,
//! `DegreeToKey`'s negative octaves and fractional octave, `TWindex`'s
//! leftover probability and its distribution, the dynamic noises' rate
//! change and negative rate. `sc_reference` holds the rest against scsynth.

#![cfg(feature = "synth")]

use std::sync::Arc;

use crate::bench::{Run, render_def};
use clausters::dsp::buffer::Buffer;
use serde_json::{Value, json};

const SR: f64 = 48_000.0;

fn c(v: f32) -> Value {
    json!({ "const": v })
}

fn pool(data: &[f32]) -> Vec<Option<Arc<Buffer>>> {
    vec![Some(Arc::new(Buffer::new(
        data.to_vec(),
        1,
        data.len(),
        SR,
    )))]
}

/// `n` samples of a graph whose last UGen goes to bus 0, over buffer 0.
fn render(ugens: Value, n: usize, buffers: &[Option<Arc<Buffer>>]) -> Vec<f32> {
    let mut ugens = ugens.as_array().unwrap().clone();
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), {"ugen": last}]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    let run = Run {
        buffers,
        ..Run::new(n)
    };
    render_def(&def, &run).remove(0)
}

fn read(kind: &str, index: f32, extra: &[f32], data: &[f32]) -> f32 {
    let mut inputs = vec![c(0.0), c(index)];
    inputs.extend(extra.iter().map(|&v| c(v)));
    render(json!([{"kind": kind, "inputs": inputs}]), 1, &pool(data))[0]
}

/// `IndexL` below the table reads its first value, as above it reads its
/// last.
#[test]
fn index_l_clips_both_ends() {
    let data = [0.5, -1.0, 2.0];
    assert_eq!(read("IndexL", -0.5, &[], &data), 0.5);
    assert_eq!(read("IndexL", 7.0, &[], &data), 2.0);
    assert_eq!(read("IndexL", 0.5, &[], &data), -0.25);
}

/// A degree below the scale is the scale an octave down, a whole number of
/// octaves included, and the octave is any size.
#[test]
fn degree_to_key_divides_as_a_scale_does() {
    let major = [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0];
    assert_eq!(read("DegreeToKey", -1.0, &[12.0], &major), -1.0);
    assert_eq!(read("DegreeToKey", -7.0, &[12.0], &major), -12.0);
    assert_eq!(read("DegreeToKey", -14.0, &[12.0], &major), -24.0);
    assert_eq!(read("DegreeToKey", 9.0, &[12.0], &major), 16.0);
    assert_eq!(read("DegreeToKey", 7.0, &[12.5], &major), 12.5);
}

/// `TWindex` draws by its weights, normalized or not, and gives what
/// unnormalized weights leave to the last index.
#[test]
fn t_windex_draws_by_its_weights() {
    let count = |normalize: f32, weights: &[f32]| {
        let mut inputs = vec![{ json!({"ugen": 0}) }, c(normalize)];
        inputs.extend(weights.iter().map(|&w| c(w)));
        let out = render(
            json!([
                {"kind": "Impulse", "inputs": [c(2400.0), c(0.5)]},
                {"kind": "TWindex", "inputs": inputs}
            ]),
            48_000 * 4,
            &[],
        );
        let mut hist = vec![0usize; weights.len()];
        // One value per impulse period of 20 samples.
        for x in out.iter().step_by(20) {
            hist[*x as usize] += 1;
        }
        let total: usize = hist.iter().sum();
        hist.iter()
            .map(|&h| h as f64 / total as f64)
            .collect::<Vec<_>>()
    };
    let p = count(1.0, &[1.0, 3.0]);
    assert!(
        (p[0] - 0.25).abs() < 0.02 && (p[1] - 0.75).abs() < 0.02,
        "{p:?}"
    );
    // 0.2 + 0.2 unnormalized: the last index takes the remaining 0.8 too.
    let p = count(0.0, &[0.2, 0.2]);
    assert!(
        (p[0] - 0.2).abs() < 0.02 && (p[1] - 0.8).abs() < 0.02,
        "{p:?}"
    );
}

/// A dynamic noise follows a change of rate at once, and a negative rate is
/// its magnitude.
#[test]
fn the_dynamic_noises_follow_their_rate_at_once() {
    for kind in ["LFDNoise0", "LFDNoise1", "LFDNoise3", "LFDClipNoise"] {
        // 10 Hz for about fifteen blocks, then 4.8 kHz: a value every ten
        // samples from then on.
        let out = render(
            json!([
                {"kind": "LFPulse", "rate": "kr", "inputs": [c(0.5), c(0.99), c(0.5)]},
                {"kind": "MulAdd", "rate": "kr", "inputs": [{"ugen": 0}, c(4790.0), c(10.0)]},
                {"kind": kind, "inputs": [{"ugen": 1}]}
            ]),
            2048,
            &[],
        );
        // How far the output moves per sample: a draw's worth every
        // hundredth of a second, then every ten samples.
        let motion =
            |x: &[f32]| x.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / x.len() as f32;
        assert!(motion(&out[..900]) < 0.005, "{kind}: still slow before");
        assert!(
            motion(&out[1024..1536]) > 0.02,
            "{kind}: fast at once after"
        );
        let back = render(json!([{"kind": kind, "inputs": [c(-4800.0)]}]), 512, &[]);
        assert!(motion(&back) > 0.02, "{kind}: a negative rate still moves");
    }
}

/// `MantissaMask` keeps the top bits of the mantissa, and `Logistic` settles
/// on its fixed point below chaos.
#[test]
fn mantissa_mask_and_logistic_follow_their_definitions() {
    let mask = |x: f32, bits: f32| {
        render(
            json!([{"kind": "MantissaMask", "inputs": [c(x), c(bits)]}]),
            1,
            &[],
        )[0]
    };
    assert_eq!(mask(1.2345, 0.0), 1.0);
    assert_eq!(mask(1.2345, 3.0), 1.125);
    assert_eq!(mask(-3.3, 23.0), -3.3);
    let y = render(
        json!([{"kind": "Logistic", "inputs": [c(2.5), c(48_000.0), c(0.1)]}]),
        4096,
        &[],
    );
    assert!((y[4095] - 0.6).abs() < 1e-6, "{}", y[4095]);
}
