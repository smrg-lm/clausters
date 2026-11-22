//! The square, Ambisonics, the reverbs and the physical models held against
//! the theory: `Pan4`'s equal power, B-format's round trip through
//! `DecodeB2`, Freeverb's decay at any rate, GVerb's at any room, a spring's
//! frequency and a dropped ball's fall. `sc_reference` holds the rest
//! against scsynth.

#![cfg(feature = "synth")]

use crate::bench::{IN_BUS, Run, render_def};
use clausters::dsp::BLOCK_SIZE;
use serde_json::{Value, json};

fn c(v: f32) -> Value {
    json!({ "const": v })
}

/// `n` samples at `sr` of a graph whose last UGen goes to bus 0.
fn render_at(ugens: Value, n: usize, sr: f32) -> Vec<f32> {
    let mut ugens = ugens.as_array().unwrap().clone();
    let last = ugens.len() - 1;
    ugens.push(json!({"kind": "Out", "inputs": [c(0.0), {"ugen": last}]}));
    let def = json!({"name": "t", "ugens": ugens}).to_string();
    let run = Run {
        sample_rate: sr,
        ..Run::new(n)
    };
    render_def(&def, &run).remove(0)
}

fn render(ugens: Value, n: usize) -> Vec<f32> {
    render_at(ugens, n, 48_000.0)
}

/// The gain of one row fed a constant 1.
fn gain(kind: &str, params: &[f32]) -> f32 {
    let mut inputs = vec![c(1.0)];
    inputs.extend(params.iter().map(|&v| c(v)));
    render(json!([{"kind": kind, "inputs": inputs}]), 1)[0]
}

/// The four gains hold the power anywhere in the square, and a position
/// off it is the one on its edge along the line from the centre.
#[test]
fn pan4_holds_the_power_and_projects_onto_the_square() {
    for (x, y) in [(0.0, 0.0), (0.3, -0.8), (-1.0, 1.0), (0.77, 0.12)] {
        let p: f32 = (0..4)
            .map(|k| gain("Pan4", &[x, y, 1.0, k as f32]).powi(2))
            .sum();
        assert!((p - 1.0).abs() < 1e-5, "({x}, {y}): {p}");
    }
    for k in 0..4 {
        let far = gain("Pan4", &[3.0, 1.5, 1.0, k as f32]);
        let edge = gain("Pan4", &[1.0, 0.5, 1.0, k as f32]);
        assert!((far - edge).abs() < 1e-6, "channel {k}");
    }
}

/// A source encoded at a speaker's direction decodes at full level there
/// and at none opposite: first-order B-format is a cardioid to each
/// speaker, the azimuth going round clockwise as the speakers do.
#[test]
fn b_format_decodes_back_to_its_direction() {
    for (az, loud, quiet) in [(0.0, 0, 2), (0.5, 1, 3), (-0.5, 3, 1), (1.0, 2, 0)] {
        let speaker = |k: usize| {
            render(
                json!([
                    {"kind": "DC", "inputs": [c(1.0)]},
                    {"kind": "PanB2", "inputs": [{"ugen": 0}, c(az), c(1.0), c(0.0)]},
                    {"kind": "PanB2", "inputs": [{"ugen": 0}, c(az), c(1.0), c(1.0)]},
                    {"kind": "PanB2", "inputs": [{"ugen": 0}, c(az), c(1.0), c(2.0)]},
                    {"kind": "DecodeB2", "inputs": [{"ugen": 1}, {"ugen": 2}, {"ugen": 3}, c(0.0), c(4.0), c(k as f32)]}
                ]),
                1,
            )[0]
        };
        assert!(
            (speaker(loud) - 1.0).abs() < 1e-5,
            "{az}: {}",
            speaker(loud)
        );
        assert!(speaker(quiet).abs() < 1e-5, "{az}: {}", speaker(quiet));
    }
    // In three dimensions, straight up is all Z and no X or Y.
    for (k, want) in [
        (0.0, std::f32::consts::FRAC_1_SQRT_2),
        (1.0, 0.0),
        (2.0, 0.0),
        (3.0, 1.0),
    ] {
        let g = gain("PanB", &[0.3, 1.0, 1.0, k]);
        assert!((g - want).abs() < 1e-6, "channel {k}: {g}");
    }
}

/// The time an impulse response takes to fall from 10 dB below its peak
/// to 40 dB below, in seconds, by its envelope over 10 ms windows.
fn fall_time(y: &[f32], sr: f32) -> f64 {
    let w = (sr / 100.0) as usize;
    let env: Vec<f64> = y
        .chunks(w)
        .map(|c| (c.iter().map(|v| (v * v) as f64).sum::<f64>() / c.len() as f64).sqrt())
        .collect();
    let peak = env.iter().cloned().fold(0.0, f64::max);
    let at = |db: f64| {
        let lim = peak * 10f64.powf(-db / 20.0);
        let first = env.iter().position(|&e| e == peak).unwrap();
        env[first..].iter().position(|&e| e < lim).unwrap() + first
    };
    (at(40.0) - at(10.0)) as f64 * w as f64 / sr as f64
}

/// Freeverb's room is the same room at every rate: its lines are its
/// tunings scaled to the rate, so its decay takes the same time.
#[test]
fn freeverb_decays_alike_at_every_rate() {
    let ir = |sr: f32| {
        render_at(
            json!([
                {"kind": "Impulse", "inputs": [c(0.2), c(0.999)]},
                {"kind": "FreeVerb", "inputs": [{"ugen": 0}, c(1.0), c(0.8), c(0.2)]}
            ]),
            (sr * 3.0) as usize,
            sr,
        )
    };
    let (a, b) = (
        fall_time(&ir(44_100.0), 44_100.0),
        fall_time(&ir(96_000.0), 96_000.0),
    );
    assert!(
        (a / b - 1.0).abs() < 0.1,
        "{a} s at 44.1 kHz, {b} s at 96 kHz"
    );
}

/// GVerb's tail falls 60 dB in `revtime`, at a room whose longest line is
/// not prime (10 m is 1411 samples) as at any other.
#[test]
fn gverb_tail_falls_by_its_reverberation_time() {
    for room in [10.0, 23.0] {
        let y = render(
            json!([
                {"kind": "Impulse", "inputs": [c(0.2), c(0.999)]},
                {"kind": "GVerb", "inputs": [{"ugen": 0}, c(room), c(2.0), c(0.0), c(1.0), c(15.0),
                    c(0.0), c(0.0), c(1.0), c(300.0), c(0.0)]}
            ]),
            48_000 * 3,
        );
        // 30 dB of the fall, so the whole 60 dB takes twice as long.
        let t60 = 2.0 * fall_time(&y, 48_000.0);
        assert!((t60 / 2.0 - 1.0).abs() < 0.2, "room {room}: {t60} s");
    }
}

/// A spring rings at `sqrt(spring * sampleRate) / 2 pi`, scsynth's units.
#[test]
fn a_spring_rings_at_its_frequency() {
    let k = 2000.0f32;
    let y = render(
        json!([
            {"kind": "Impulse", "inputs": [c(0.1), c(0.999)]},
            {"kind": "Spring", "inputs": [{"ugen": 0}, c(k), c(0.0)]}
        ]),
        4800,
    );
    // From the impulse on.
    let from = y.iter().position(|&v| v != 0.0).unwrap() + 1;
    let crossings: Vec<usize> = (from..y.len() - 1)
        .filter(|&i| y[i] <= 0.0 && y[i + 1] > 0.0)
        .collect();
    let (first, last) = (crossings[0], *crossings.last().unwrap());
    let measured = (crossings.len() - 1) as f64 / ((last - first) as f64 / 48_000.0);
    let want = (k as f64 * 48_000.0).sqrt() / std::f64::consts::TAU;
    assert!(
        (measured / want - 1.0).abs() < 0.02,
        "{measured} Hz against {want}"
    );
}

/// A ball resting on a floor that drops falls freely from where it rested,
/// `g'` above the floor (`g' = g / sampleRate`, its fall per sample per
/// sample): `g' n (n + 1) / 2` below it after `n` samples, until it nears
/// the floor.
#[test]
fn a_dropped_ball_falls_as_gravity_has_it() {
    let (h, g) = (0.5f32, 10.0f32);
    // The floor is at `h` for a block, the ball resting on it, then at 0.
    let mut floor = vec![h; BLOCK_SIZE];
    floor.resize(4800, 0.0);
    let def = json!({"name": "t", "ugens": [
        {"kind": "In", "inputs": [c(IN_BUS as f32)]},
        {"kind": "Ball", "inputs": [{"ugen": 0}, c(g), c(0.0), c(0.01)]},
        {"kind": "Out", "inputs": [c(0.0), {"ugen": 1}]}
    ]})
    .to_string();
    let y = render_def(&def, &Run::new(4800).input(&floor)).remove(0);
    let gp = g as f64 / 48_000.0;
    let rest = h as f64 + gp;
    assert!(
        (y[BLOCK_SIZE - 1] as f64 - rest).abs() < 1e-6,
        "rests at {}",
        y[BLOCK_SIZE - 1]
    );
    let mut n = 1usize;
    loop {
        let want = rest - gp * (n * (n + 1)) as f64 / 2.0;
        if want < 0.15 {
            break;
        }
        let got = y[BLOCK_SIZE - 1 + n] as f64;
        assert!(
            (got - want).abs() < 1e-4,
            "{n} samples into the fall: {got} against {want}"
        );
        n += 1;
    }
    assert!(n > 50, "fell for {n} samples");
}
