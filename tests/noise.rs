//! Noise (U6): the stochastic sources.
//!
//! A random signal cannot be asserted sample by sample, so the claims here are
//! about **distributions and spectra**, per the rules in the `audio-testing`
//! skill: the measured slope in dB/octave (the whole reason `PinkNoise` and
//! `BrownNoise` are separate kinds), the mean and variance, the mean density of
//! an impulse source, and — for every generator — bit-exact reproducibility
//! from a seed, which is what lets a noisy patch have a golden file at all.
//!
//! **Rule 5 lives here rather than in the shared table**, which is the one
//! place these rows differ from every other family. Splitting a block means
//! comparing two renders, and through the def path that means two instances,
//! each of which draws its own seed on purpose — correlated noise summed with
//! itself is a comb filter — while the wire has no seed input to pin. So the
//! table refuses these, and `every_generator_is_unmoved_by_a_block_split`
//! below discharges the rule one level down, against `with_seed` constructors,
//! where the comparison is exact.

#![cfg(feature = "synth")]

#[path = "common/signal.rs"]
mod signal;

use std::sync::Arc;

use clausters::dsp::noise::{
    BrownNoise, ClipNoise, Crackle, Dust, DustMode, GrayNoise, LfNoise, LfNoiseShape, PinkNoise,
    WhiteNoise,
};
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx, UGen};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use signal::*;

const SR: f32 = 48_000.0;

/// Renders `n` samples straight from a UGen, with no graph around it — the
/// form the seeded constructors need, since a def has no way to name a seed.
fn run(ugen: &mut dyn UGen, inputs: &[&[f32]], n: usize) -> Vec<f32> {
    let buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = vec![0.0f32; n];
    for chunk in out.chunks_mut(BLOCK_SIZE) {
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses: &buses,
            buffers: &[],
            offset: 0,
            frames: chunk.len(),
        };
        ugen.process(&mut ctx, inputs, chunk);
    }
    out
}

/// Renders `n` samples of a one-UGen def written to bus 0 — the path a real
/// def takes, seeds and all.
fn render(ugen: &str, n: usize) -> Vec<f32> {
    let json = format!(
        r#"{{"name": "n", "ugens": [{ugen},
            {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}]}}"#
    );
    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses: &buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
        };
        synth.process(&mut ctx);
        out.extend_from_slice(buses.audio(0));
    }
    out.truncate(n);
    out
}

const N: usize = 1 << 17; // ~2.7 s — enough frames for a stable Welch average

// ---- the spectral shapes ----

#[test]
fn the_three_spectral_shapes_measure_zero_minus_three_and_minus_six() {
    // This *is* the reason they are three kinds rather than one. The slope is
    // measured over 40 Hz - 10 kHz: below that a 2.7 s window has too few
    // periods to average, and above it the walk's own reflection shows.
    let white = run(&mut WhiteNoise::with_seed(1), &[], N);
    let pink = run(&mut PinkNoise::with_seed(1), &[], N);
    let brown = run(&mut BrownNoise::with_seed(1), &[], N);

    let slope = |x: &[f32]| spectral_slope_db_per_octave(x, SR, 40.0, 10_000.0);
    let (sw, sp, sb) = (slope(&white), slope(&pink), slope(&brown));
    println!("measured slopes: white {sw:.2}, pink {sp:.2}, brown {sb:.2} dB/octave");

    assert!(sw.abs() < 0.3, "white should be flat, measured {sw:.2}");
    // The name's whole content: equal energy per octave is -3.01 dB/octave.
    assert!(
        (sp + 3.01).abs() < 0.35,
        "pink should be -3.01 dB/octave, measured {sp:.2}"
    );
    // A random walk integrates white noise, so -6.02.
    assert!(
        (sb + 6.02).abs() < 0.6,
        "brown should be -6.02 dB/octave, measured {sb:.2}"
    );
}

#[test]
fn pink_noise_is_quiet_and_centred_like_the_one_a_def_was_written_against() {
    // Seventeen uniforms mapped onto the full scale: the peak is reachable and
    // almost never reached. A def ported from sclang expects this level, so it
    // is worth pinning rather than "some noise came out".
    let x = run(&mut PinkNoise::with_seed(7), &[], N);
    assert_finite(&x, "PinkNoise");
    let (r, d, p) = (rms(&x), dc(&x), peak(&x));
    println!("pink: rms {r:.3}, dc {d:.4}, peak {p:.3}");
    assert!((0.08..0.20).contains(&r), "rms {r:.3}");
    assert!(d.abs() < 0.02, "should be centred, dc {d:.4}");
    assert!(p <= 1.0, "must not leave [-1, 1], peak {p:.3}");
}

#[test]
fn brown_noise_reflects_instead_of_resting_against_a_rail() {
    // Clamping would let the walk sit at ±1 — a constant, audible as silence
    // with a click at each end. Reflection keeps it moving: no run of equal
    // samples anywhere near the rails, and the distribution stays flat rather
    // than piling up there.
    let x = run(&mut BrownNoise::with_seed(3), &[], N);
    assert_finite(&x, "BrownNoise");
    assert!(peak(&x) <= 1.0, "stays in range");
    let longest = x
        .windows(2)
        .fold((0usize, 0usize), |(best, run), w| {
            let run = if w[0] == w[1] { run + 1 } else { 0 };
            (best.max(run), run)
        })
        .0;
    assert_eq!(longest, 0, "a reflecting walk never repeats a sample");
    // Flat-ish: the extreme decile is not over-represented the way clamping
    // would make it.
    let extreme = x.iter().filter(|v| v.abs() > 0.9).count() as f32 / x.len() as f32;
    println!("brown: fraction beyond ±0.9 = {extreme:.4}");
    assert!(extreme < 0.15, "piling up at the rails: {extreme:.4}");
}

// ---- the bit and sign sources ----

#[test]
fn clip_noise_is_only_ever_plus_or_minus_one_and_fair() {
    let x = run(&mut ClipNoise::with_seed(11), &[], N);
    assert!(
        x.iter().all(|v| v.abs() == 1.0),
        "every sample must be at full scale"
    );
    let mean = dc(&x);
    // A fair coin over 131072 flips: the standard error is 1/sqrt(N) ≈ 0.003,
    // so 0.02 is six sigma and still a real test of fairness.
    assert!(mean.abs() < 0.02, "biased coin, mean {mean:.4}");
    assert!(
        (rms(&x) - 1.0).abs() < 1e-6,
        "and its RMS is 1 by construction"
    );
}

#[test]
fn gray_noise_moves_by_one_bit_at_a_time() {
    // One bit of the *integer* word flips per sample, so the step is exactly a
    // power of two — in the integer. It is not recoverable from the output,
    // because the output is `word / 2^31` in `f32` and an `f32` significand is
    // 24 bits against the word's 31: the conversion rounds, by an amount that
    // depends on the word's magnitude, which the flip itself changes. Flipping
    // bit 28 of 0x0001F3A5 reads as a step of 268435451 rather than 2^28, and
    // flipping bit 0 of a word near 2^29 reads as no step at all. What is
    // observable
    // is the distribution the bit flipping produces, and that is the character
    // the kind exists for: steps spanning every order of magnitude, so the
    // median step is a tiny fraction of the mean one. White noise, whose steps
    // are a sum of two uniforms, has them within a factor of two.
    let gray = run(&mut GrayNoise::with_seed(5), &[], 1 << 15);
    let white = run(&mut WhiteNoise::with_seed(5), &[], 1 << 15);
    assert_finite(&gray, "GrayNoise");
    assert!(peak(&gray) <= 1.0, "stays in range");

    let ratio = |x: &[f32]| {
        let mut steps: Vec<f32> = x.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = steps[steps.len() / 2];
        let mean = steps.iter().sum::<f32>() / steps.len() as f32;
        mean / median.max(1e-12)
    };
    let (rg, rw) = (ratio(&gray), ratio(&white));
    println!("mean/median step: gray {rg:.1}, white {rw:.2}");
    assert!(rw < 3.0, "white noise steps are all of a size: {rw:.2}");
    assert!(
        rg > 100.0,
        "gray noise steps should span orders of magnitude: {rg:.1}"
    );
    // And its spectrum is **not** flat, which is easy to assume and wrong: the
    // high bits flip rarely (one in 32 samples for the top one) and the low
    // ones carry almost no weight, so the energy sits low. Measured -2.9
    // dB/octave, near enough pink. sclang's help says the same in words; this
    // is the number.
    let slope = spectral_slope_db_per_octave(&gray, SR, 40.0, 10_000.0);
    println!("gray slope {slope:.2} dB/octave");
    assert!(
        (-3.6..-2.2).contains(&slope),
        "gray noise leans low, around -2.9 dB/octave, measured {slope:.2}"
    );
}

// ---- the held and interpolated shapes ----

#[test]
fn lf_noise0_holds_each_value_for_one_period() {
    // 100 Hz at 48 kHz is 480 samples a segment, so a second holds 100 values.
    let x = run(
        &mut LfNoise::with_seed(LfNoiseShape::Step, 2),
        &[&[100.0]],
        48_000,
    );
    let steps = x.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(
        (99..=101).contains(&steps),
        "100 Hz should step ~100 times a second, got {steps}"
    );
    assert!(peak(&x) <= 1.0, "stays in range");
}

#[test]
fn lf_clip_noise_holds_but_only_ever_at_the_rails() {
    let x = run(
        &mut LfNoise::with_seed(LfNoiseShape::Clip, 4),
        &[&[100.0]],
        48_000,
    );
    assert!(x.iter().all(|v| v.abs() == 1.0), "±1 only");
    let steps = x.windows(2).filter(|w| w[0] != w[1]).count();
    // Half the draws repeat the previous side, so the visible steps are about
    // half the segments — which is itself the check that it is *drawing* each
    // segment rather than alternating.
    println!("LFClipNoise: {steps} visible steps in 100 segments");
    assert!((30..=70).contains(&steps), "{steps} steps");
}

#[test]
fn lf_noise1_is_piecewise_linear_and_lf_noise2_has_no_corners() {
    // The difference between the two, stated as the property that separates
    // them: a linear ramp has a *constant* first difference inside a segment
    // and a jump in it at the boundary; the quadratic has a first difference
    // that changes smoothly and never jumps.
    let n = 48_000;
    let one = run(
        &mut LfNoise::with_seed(LfNoiseShape::Linear, 6),
        &[&[100.0]],
        n,
    );
    let two = run(
        &mut LfNoise::with_seed(LfNoiseShape::Quadratic, 6),
        &[&[100.0]],
        n,
    );
    assert_finite(&one, "LFNoise1");
    assert_finite(&two, "LFNoise2");

    let jumps = |x: &[f32]| {
        let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        // The largest change in slope from one sample to the next, in units of
        // the typical slope.
        let typical = d.iter().map(|v| v.abs()).fold(0.0f32, f32::max).max(1e-9);
        d.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
            / typical
    };
    let (j1, j2) = (jumps(&one), jumps(&two));
    println!("slope discontinuity: LFNoise1 {j1:.2}, LFNoise2 {j2:.4} (of the peak slope)");
    assert!(j1 > 0.5, "LFNoise1 must have corners at its boundaries");
    assert!(j2 < 0.05, "LFNoise2 must not, measured {j2:.4}");

    // The quadratic **overshoots** its draws, because it aims at the midpoints
    // between them and carries its slope across the boundary. scsynth's does
    // too, and it is worth pinning rather than discovering: measured 1.67 here,
    // and stable — the peak is the same over one second and over ten, at 5 Hz,
    // 100 Hz and 2 kHz, so the carried slope does not accumulate.
    println!("LFNoise2 peak {:.3}", peak(&two));
    assert!(peak(&two) < 1.8, "bounded, if not by 1: {:.3}", peak(&two));
}

#[test]
fn an_lf_noise_at_control_rate_steps_at_the_same_speed() {
    // The calculation-rate contract: the frequency is in hertz at either rate.
    let ar = render(
        r#"{"kind": "LFNoise0", "inputs": [{"const": 10.0}]}"#,
        48_000,
    );
    let kr = render(
        r#"{"kind": "LFNoise0", "rate": "kr", "inputs": [{"const": 10.0}]}"#,
        48_000,
    );
    let steps = |x: &[f32]| x.windows(2).filter(|w| w[0] != w[1]).count();
    assert!((9..=11).contains(&steps(&ar)), "ar: {}", steps(&ar));
    assert!((9..=11).contains(&steps(&kr)), "kr: {}", steps(&kr));
}

// ---- the impulsive and chaotic ----

#[test]
fn dust_fires_at_its_mean_density_with_random_amplitudes() {
    for density in [10.0f32, 200.0] {
        let x = run(
            &mut Dust::with_seed(DustMode::Unipolar, 8),
            &[&[density]],
            48_000 * 4,
        );
        let hits = x.iter().filter(|v| **v != 0.0).count() as f32 / 4.0;
        // A Poisson count over four seconds: the standard deviation is
        // sqrt(4·density)/4, so this bound is about four sigma.
        let sigma = (4.0 * density).sqrt() / 4.0;
        println!("Dust({density}): {hits:.1} impulses/second (sigma {sigma:.2})");
        assert!(
            (hits - density).abs() < 4.0 * sigma,
            "{hits} impulses/second against a density of {density}"
        );
        assert!(
            x.iter().all(|v| (0.0..1.0).contains(v)),
            "Dust is unipolar in [0, 1)"
        );
    }
}

#[test]
fn dust2_fires_both_ways_at_the_same_mean_density() {
    // The bipolar sibling had been checked only for having some negative
    // samples, riding on `Dust`'s density measurement. It is the same Poisson
    // process, so it owes the same figure — and the sign has to be a fair coin
    // on top of it, which is the part that "some are negative" does not say.
    for density in [10.0f32, 200.0] {
        let x = run(
            &mut Dust::with_seed(DustMode::Bipolar, 8),
            &[&[density]],
            48_000 * 4,
        );
        let hits: Vec<f32> = x.iter().copied().filter(|v| *v != 0.0).collect();
        let rate = hits.len() as f32 / 4.0;
        let sigma = (4.0 * density).sqrt() / 4.0;
        println!("Dust2({density}): {rate:.1} impulses/second (sigma {sigma:.2})");
        assert!(
            (rate - density).abs() < 4.0 * sigma,
            "{rate} impulses/second against a density of {density}"
        );
        assert!(
            x.iter().all(|v| (-1.0..1.0).contains(v)),
            "Dust2 is bipolar in [-1, 1)"
        );
        // Fair: over `n` impulses the count of negative ones is binomial, so
        // its standard error is sqrt(n)/2. Four of those.
        let down = hits.iter().filter(|v| **v < 0.0).count() as f32;
        let (half, se) = (hits.len() as f32 / 2.0, (hits.len() as f32).sqrt() / 2.0);
        assert!(
            (down - half).abs() < 4.0 * se,
            "{down} of {} impulses fired downward, expected about {half}",
            hits.len()
        );
    }
}

#[test]
fn dust_is_not_a_clock() {
    // The property that separates it from `Impulse`, and the reason to say so
    // in the docs: the intervals are exponential, not constant. Over a long
    // run the shortest gap is a small fraction of the longest.
    let x = run(
        &mut Dust::with_seed(DustMode::Unipolar, 9),
        &[&[100.0]],
        48_000 * 4,
    );
    let hits: Vec<usize> = x
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != 0.0)
        .map(|(i, _)| i)
        .collect();
    let gaps: Vec<usize> = hits.windows(2).map(|w| w[1] - w[0]).collect();
    let (lo, hi) = (
        *gaps.iter().min().unwrap() as f32,
        *gaps.iter().max().unwrap() as f32,
    );
    println!("Dust(100) gaps: shortest {lo}, longest {hi} samples (mean 480)");
    assert!(hi > 5.0 * lo, "an exponential spread, not a clock");
}

#[test]
fn crackle_is_deterministic_bounded_and_carries_dc() {
    let a = run(&mut Crackle::default(), &[&[1.5]], 1 << 15);
    let b = run(&mut Crackle::default(), &[&[1.5]], 1 << 15);
    assert_eq!(a, b, "no RNG at all: the same chaos gives the same signal");
    assert_finite(&a, "Crackle");
    assert!(peak(&a) < 2.0, "bounded, peak {:.3}", peak(&a));
    // The absolute value is part of the map, so it is one-sided.
    let mean = dc(&a);
    println!("Crackle(1.5): mean {mean:.3}, rms {:.3}", rms(&a));
    assert!(mean > 0.05, "unipolar, so it carries DC: mean {mean:.3}");
    // It does not repeat: no period up to 512 samples anywhere in the tail.
    // (A longer or quasi-period would not be caught by this, which is the
    // honest limit of what a test can say about a chaotic map.)
    let tail = &a[a.len() - 8192..];
    let period = (1..=512).find(|&p| {
        tail[..4096]
            .iter()
            .zip(tail[p..p + 4096].iter())
            .all(|(x, y)| (x - y).abs() < 1e-6)
    });
    assert_eq!(period, None, "found a short period: {period:?}");

    // `chaos` materially changes the signal, and **not monotonically** — the
    // measured spread runs 0.56, 0.20, 0.08, 0.05, 0.19, 0.05, 0.06 across
    // chaos 0.3 to 1.9. It is a map, not a level control: reach for it by ear.
    let spread = |x: &[f32]| {
        let t = &x[x.len() * 3 / 4..];
        let m = dc(t);
        (t.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / t.len() as f32).sqrt()
    };
    let low = run(&mut Crackle::default(), &[&[0.3]], 1 << 15);
    println!(
        "Crackle spread: chaos 0.3 -> {:.5}, chaos 1.5 -> {:.5}",
        spread(&low),
        spread(&a)
    );
    assert!(
        spread(&low) > 5.0 * spread(&a),
        "the parameter must change the signal, not just its seed"
    );
}

// ---- reproducibility ----

#[test]
fn every_generator_replays_exactly_from_its_seed() {
    // The rule a golden file for a noisy patch depends on. `Crackle` has no
    // seed because it has no RNG; it is covered above.
    // The constructor is taken as a closure over the seed, so each kind can be
    // built twice from the same one and once from another: reproducibility and
    // "the seed is actually read" are different claims, and a generator that
    // ignored its seed entirely would satisfy the first alone.
    macro_rules! same {
        ($what:expr, $make:expr, $inputs:expr) => {{
            let mk = $make;
            let a = run(&mut mk(42), $inputs, 4096);
            let b = run(&mut mk(42), $inputs, 4096);
            assert_eq!(a, b, "{} is not reproducible from its seed", $what);
            let c = run(&mut mk(43), $inputs, 4096);
            assert_ne!(a, c, "{} gives the same stream for two seeds", $what);
        }};
    }
    same!("WhiteNoise", WhiteNoise::with_seed, &[]);
    same!("PinkNoise", PinkNoise::with_seed, &[]);
    same!("BrownNoise", BrownNoise::with_seed, &[]);
    same!("GrayNoise", GrayNoise::with_seed, &[]);
    same!("ClipNoise", ClipNoise::with_seed, &[]);
    same!(
        "LFNoise0",
        |s| LfNoise::with_seed(LfNoiseShape::Step, s),
        &[&[300.0]]
    );
    same!(
        "LFNoise1",
        |s| LfNoise::with_seed(LfNoiseShape::Linear, s),
        &[&[300.0]]
    );
    same!(
        "LFNoise2",
        |s| LfNoise::with_seed(LfNoiseShape::Quadratic, s),
        &[&[300.0]]
    );
    same!(
        "LFClipNoise",
        |s| LfNoise::with_seed(LfNoiseShape::Clip, s),
        &[&[300.0]]
    );
    same!(
        "Dust",
        |s| Dust::with_seed(DustMode::Unipolar, s),
        &[&[500.0]]
    );
    same!(
        "Dust2",
        |s| Dust::with_seed(DustMode::Bipolar, s),
        &[&[500.0]]
    );
}

// ---- rule 5, one level down ----

/// Renders `n` samples the way [`run`] does, but cutting every block at `at`:
/// two `process` calls over the two halves of the output slice, which is what
/// the synth does when a scheduled bundle splits a block.
///
/// Only valid for constant inputs — a signal input would need its own slice
/// per call — and every generator here takes constants.
fn run_split(ugen: &mut dyn UGen, inputs: &[&[f32]], n: usize, at: usize) -> Vec<f32> {
    let buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = vec![0.0f32; n];
    for chunk in out.chunks_mut(BLOCK_SIZE) {
        let cut = at.min(chunk.len());
        let (head, tail) = chunk.split_at_mut(cut);
        for (offset, part) in [(0, head), (cut, tail)] {
            if part.is_empty() {
                continue;
            }
            let mut ctx = ProcessCtx {
                sample_rate: SR,
                full_sample_rate: SR,
                buses: &buses,
                buffers: &[],
                offset,
                frames: part.len(),
            };
            ugen.process(&mut ctx, inputs, part);
        }
    }
    out
}

#[test]
fn every_generator_is_unmoved_by_a_block_split() {
    // Rule 5 for the stochastic sources, and the reason it is here rather than
    // in the shared table: comparing two renders means comparing two
    // instances, and through the def path each one draws its own seed on
    // purpose (correlated noise summed with itself is a comb filter), while
    // the wire has no seed input to pin. One level down the stream *is*
    // pinned, so the comparison is exact -- not a tolerance, equality.
    //
    // What would fail here and nowhere else: a generator that refilled a
    // buffer per `process` call rather than per sample, or one holding a
    // countdown in samples-until-next-block.
    macro_rules! unmoved {
        ($what:expr, $make:expr, $inputs:expr) => {{
            let n = BLOCK_SIZE * 32;
            let whole = run(&mut $make, $inputs, n);
            let split = run_split(&mut $make, $inputs, n, 23);
            assert_eq!(whole, split, "{} differs when the block is cut", $what);
        }};
    }
    unmoved!("WhiteNoise", WhiteNoise::with_seed(7), &[]);
    unmoved!("PinkNoise", PinkNoise::with_seed(7), &[]);
    unmoved!("BrownNoise", BrownNoise::with_seed(7), &[]);
    unmoved!("GrayNoise", GrayNoise::with_seed(7), &[]);
    unmoved!("ClipNoise", ClipNoise::with_seed(7), &[]);
    unmoved!(
        "LFNoise0",
        LfNoise::with_seed(LfNoiseShape::Step, 7),
        &[&[300.0]]
    );
    unmoved!(
        "LFNoise1",
        LfNoise::with_seed(LfNoiseShape::Linear, 7),
        &[&[300.0]]
    );
    unmoved!(
        "LFNoise2",
        LfNoise::with_seed(LfNoiseShape::Quadratic, 7),
        &[&[300.0]]
    );
    unmoved!(
        "LFClipNoise",
        LfNoise::with_seed(LfNoiseShape::Clip, 7),
        &[&[300.0]]
    );
    unmoved!("Dust", Dust::with_seed(DustMode::Unipolar, 7), &[&[500.0]]);
    unmoved!("Dust2", Dust::with_seed(DustMode::Bipolar, 7), &[&[500.0]]);
    unmoved!("Crackle", Crackle::default(), &[&[1.5]]);
}

// ---- rule 4: the long run ----

#[test]
fn the_slow_generators_stay_bounded_over_ten_seconds() {
    // `LFNoise2` overshoots its range by construction -- it aims at the
    // midpoint between two draws and carries its slope through, so it can
    // swing past either. The question a long run answers is whether that
    // overshoot is *bounded* or whether it grows.
    //
    // Compared **half against half of one run**, not one second against ten.
    // The tempting version of this test is the wrong one: at 5 Hz a second
    // holds five segments and ten seconds hold fifty, so the longer window
    // peaks higher for having drawn more, which says nothing about the bound.
    // Two halves of the same run hold the same number of draws.
    for hz in [5.0f32, 100.0, 2000.0] {
        let n = 48_000 * 10;
        let x = run(
            &mut LfNoise::with_seed(LfNoiseShape::Quadratic, 3),
            &[&[hz]],
            n,
        );
        assert_finite(&x, "LFNoise2 over ten seconds");
        let (first, second) = (peak(&x[..n / 2]), peak(&x[n / 2..]));
        println!("LFNoise2({hz}): peak {first:.4} then {second:.4} over 10 s");
        // The construction allows +/-1.7. What it actually reaches over ten
        // seconds is less: 1.22 at 5 Hz, 1.21 at 100, 1.09 at 2 kHz -- the
        // worst case needs two extreme draws in a row and does not come up.
        // The bound asserted is the construction's, since that is the claim
        // the documentation makes.
        assert!(
            peak(&x) < 1.7,
            "LFNoise2 at {hz} Hz peaks at {} over ten seconds",
            peak(&x)
        );
        assert!(
            (second - first).abs() < 0.3,
            "LFNoise2 at {hz} Hz: peak {first} in the first half, {second} in \
             the second -- the overshoot is trending, not bounded"
        );
    }

    // `Crackle` is a chaotic map with no RNG, so the risk is the other one: a
    // map that leaves its attractor lands on a rail or on a NaN, and only a
    // long run gets far enough in to find out.
    for chaos in [1.0f32, 1.5, 1.9] {
        let x = run(&mut Crackle::default(), &[&[chaos]], 48_000 * 10);
        assert_finite(&x, "Crackle over ten seconds");
        let p = peak(&x);
        assert!(p <= 2.0, "Crackle at chaos {chaos} reached {p}");
        // And it is still moving: a map that collapsed to a fixed point would
        // be finite and in range, and dead.
        let tail = &x[x.len() - 48_000..];
        assert!(
            rms(tail) > 0.01,
            "Crackle at chaos {chaos} went quiet by the tenth second"
        );
    }
}

#[test]
fn two_instances_in_one_graph_are_not_the_same_stream() {
    // Correlated "noise" summed with itself is a comb filter, not more noise.
    // The per-instance seeding is what prevents it, and it is invisible until
    // someone puts two of the same kind in one def.
    let x = render(
        r#"{"kind": "WhiteNoise", "inputs": []},
           {"kind": "WhiteNoise", "inputs": []},
           {"kind": "BinaryOpUGen", "op": "sub", "inputs": [{"ugen": 0}, {"ugen": 1}]}"#,
        1 << 14,
    );
    // Identical streams would cancel exactly.
    assert!(rms(&x) > 0.5, "the two streams cancelled: rms {}", rms(&x));
}
