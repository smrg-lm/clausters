//! The two measuring UGens a meter is built out of: `Meter` (the level a
//! person reads) and `ClipCount` (how many times the signal was flattened).
//!
//! Both are driven by a signal written in Rust rather than by an oscillator, so
//! what goes in is exactly what the assert is about: a run of samples at full
//! scale is three samples, not "roughly the top of a sine".

#![cfg(feature = "synth")]

#[path = "common/bench.rs"]
mod bench;

use bench::{SR, render_with_input, render_with_input_split};
use clausters::dsp::BLOCK_SIZE;

/// `n` samples that are `value` everywhere except a run of `run` samples at
/// `peak` starting at `at`.
fn with_run(n: usize, value: f32, at: usize, run: usize, peak: f32) -> Vec<f32> {
    let mut x = vec![value; n];
    for s in x.iter_mut().skip(at).take(run) {
        *s = peak;
    }
    x
}

fn clip_count(input: &[f32], ceiling: f32, run: f32) -> Vec<f32> {
    render_with_input(
        &format!(
            r#"{{"kind": "ClipCount", "rate": "ar", "inputs": [{{"ugen": 0}},
               {{"const": {ceiling}}}, {{"const": {run}}}]}}"#
        ),
        input,
    )
}

fn meter(input: &[f32], decay: f32, hold: f32) -> Vec<f32> {
    render_with_input(
        &format!(
            r#"{{"kind": "Meter", "rate": "ar", "inputs": [{{"ugen": 0}},
               {{"const": {decay}}}, {{"const": {hold}}}]}}"#
        ),
        input,
    )
}

// ---- ClipCount ----

/// The rule the red mark reports: a **run** at full scale, not a sample that
/// touched it.
#[test]
fn a_lone_full_scale_sample_is_not_an_over() {
    let one = with_run(BLOCK_SIZE, 0.2, 10, 1, 1.0);
    assert_eq!(*clip_count(&one, 1.0, 3.0).last().unwrap(), 0.0);

    let three = with_run(BLOCK_SIZE, 0.2, 10, 3, 1.0);
    assert_eq!(*clip_count(&three, 1.0, 3.0).last().unwrap(), 1.0);
}

/// However long the flattening lasts, it is one event: a second of square wave
/// is not forty thousand overs.
#[test]
fn one_run_is_one_over_however_long() {
    let long = with_run(BLOCK_SIZE, 0.2, 4, 40, 1.0);
    assert_eq!(*clip_count(&long, 1.0, 3.0).last().unwrap(), 1.0);

    let mut two = with_run(BLOCK_SIZE, 0.2, 4, 10, 1.0);
    two[30..40].fill(1.0);
    assert_eq!(
        *clip_count(&two, 1.0, 3.0).last().unwrap(),
        2.0,
        "a break between the runs is a second over"
    );
}

/// The pessimistic meter is the same UGen with `run` of 1 -- which is why the
/// rule is a parameter and not a constant in the drawing.
#[test]
fn a_run_of_one_is_the_pessimistic_meter() {
    let one = with_run(BLOCK_SIZE, 0.2, 10, 1, 1.0);
    assert_eq!(*clip_count(&one, 1.0, 1.0).last().unwrap(), 1.0);
}

/// The count is the signal's and grows across the render; a reader differences
/// it to notice an over it has not seen.
#[test]
fn the_count_only_grows() {
    let mut x = vec![0.1f32; BLOCK_SIZE * 4];
    for block in 0..4 {
        x[block * BLOCK_SIZE..block * BLOCK_SIZE + 5].fill(1.0);
    }
    let out = clip_count(&x, 1.0, 3.0);
    let counts: Vec<f32> = (0..4).map(|b| out[b * BLOCK_SIZE]).collect();
    assert_eq!(counts, vec![1.0, 2.0, 3.0, 4.0]);
}

/// Rule 5, the block split: a run that a scheduled bundle cuts in half is
/// still one run, because the state is the counter's and not the block's.
#[test]
fn a_run_cut_by_a_block_split_is_still_one_run() {
    let x = with_run(BLOCK_SIZE, 0.2, 30, 6, 1.0);
    let whole = *clip_count(&x, 1.0, 3.0).last().unwrap();
    let cut = *render_with_input_split(
        r#"{"kind": "ClipCount", "rate": "ar", "inputs": [{"ugen": 0},
           {"const": 1.0}, {"const": 3.0}]}"#,
        &x,
        33,
    )
    .last()
    .unwrap();
    assert_eq!((whole, cut), (1.0, 1.0));
}

/// The ceiling is a parameter, so a meter can be read against a level that is
/// not full scale.
#[test]
fn the_ceiling_is_where_it_is_told() {
    let x = with_run(BLOCK_SIZE, 0.2, 10, 5, 0.6);
    assert_eq!(*clip_count(&x, 1.0, 3.0).last().unwrap(), 0.0);
    assert_eq!(*clip_count(&x, 0.5, 3.0).last().unwrap(), 1.0);
}

// ---- Meter ----

/// The attack is instantaneous: the block that held the peak reads it, whole.
#[test]
fn the_peak_of_a_block_is_read_at_once() {
    let x = with_run(BLOCK_SIZE, 0.1, 40, 1, 0.8);
    let out = meter(&x, 20.0, 0.0);
    assert!(
        (out[0] - 0.8).abs() < 1e-6,
        "one block is one measurement, at the block's own peak: {}",
        out[0]
    );
}

/// And the fall is the declared number of decibels per second, which is what
/// makes a reader slower than the engine see a transient at all.
#[test]
fn the_fall_is_the_declared_decibels_per_second() {
    let n = BLOCK_SIZE * 64;
    let mut x = vec![0.0f32; n];
    x[..BLOCK_SIZE].fill(1.0);
    let out = meter(&x, 20.0, 0.0);
    let seconds = (n - BLOCK_SIZE) as f32 / SR;
    let expected = 10f32.powf(-20.0 * seconds / 20.0);
    let last = *out.last().unwrap();
    assert!(
        (20.0 * (last / expected).log10()).abs() < 0.5,
        "20 dB/s from unity over {seconds:.3} s is {expected:.4}, read {last:.4}"
    );
}

/// A hold keeps the mark up for the seconds it was given before any of that
/// happens -- the half of a meter that waits to be read.
#[test]
fn a_held_peak_does_not_move_while_it_is_held() {
    let n = BLOCK_SIZE * 64;
    let mut x = vec![0.0f32; n];
    x[..BLOCK_SIZE].fill(1.0);
    let held = meter(&x, 20.0, 1.0);
    assert_eq!(*held.last().unwrap(), 1.0, "still held, so still at unity");
}

// ---- the rate the reading is taken at ----

/// **A control-rate meter still reads every sample.** It emits one number a
/// block -- which is what a control bus carries -- and its audio-rate input is
/// still the whole block, so the peak it reports is the block's and not the
/// first sample of it. This is the reading a meter exists to not get wrong, and
/// it was wrong here: both UGens walked their own output, which at `kr` is one
/// sample in sixty-four.
#[test]
fn a_control_rate_meter_reads_the_whole_block() {
    // A transient between two control samples: silent at the block's edges,
    // loud in the middle, where a per-block reader would never look.
    let mut x = vec![0.0f32; BLOCK_SIZE];
    x[BLOCK_SIZE / 2] = 0.9;
    let out = render_with_input(
        r#"{"kind": "Meter", "rate": "kr", "inputs": [{"ugen": 0},
           {"const": 20.0}, {"const": 0.0}]}"#,
        &x,
    );
    assert!(
        (out[0] - 0.9).abs() < 1e-6,
        "a transient inside the block is the block's peak, got {}",
        out[0]
    );
}

/// The same for the count: a run is the whole of what it counts, so a reader
/// taking one sample a block would not be coarse -- it would be counting a
/// different thing.
#[test]
fn a_control_rate_count_sees_the_run() {
    let x = with_run(BLOCK_SIZE, 0.2, 20, 4, 1.0);
    let out = render_with_input(
        r#"{"kind": "ClipCount", "rate": "kr", "inputs": [{"ugen": 0},
           {"const": 1.0}, {"const": 3.0}]}"#,
        &x,
    );
    assert_eq!(out[0], 1.0, "one run inside the block is one over");
}

// ---- TruePeak ----

fn true_peak(input: &[f32], rate: &str) -> Vec<f32> {
    render_with_input(
        &format!(
            r#"{{"kind": "TruePeak", "rate": "{rate}", "inputs": [{{"ugen": 0}},
               {{"const": 20.0}}, {{"const": 1.0}}]}}"#
        ),
        input,
    )
}

/// **The level between the samples.** A tone at a quarter of the sample rate,
/// sampled at 45 degrees, sits at +-1 on every sample and reaches sqrt(2)
/// between them: `Meter` reads full scale and `TruePeak` reads three decibels
/// over it -- the reading a sample meter structurally cannot make.
#[test]
fn a_true_peak_reads_what_the_samples_hide() {
    let x: Vec<f32> = (0..BLOCK_SIZE * 8)
        .map(|i| if (i / 2) % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let sample = *meter(&x, 20.0, 1.0).last().unwrap();
    let truth = *true_peak(&x, "ar").last().unwrap();
    assert_eq!(sample, 1.0, "every sample is at full scale");
    let over = 20.0 * truth.log10();
    assert!(
        (over - 3.01).abs() < 0.25,
        "the signal between them is sqrt(2): {over:.3} dBTP"
    );
}

/// At control rate it still reads the whole block, as `Meter` does: the
/// output is one number a block and the input is not.
#[test]
fn a_control_rate_true_peak_reads_the_whole_block() {
    let x: Vec<f32> = (0..BLOCK_SIZE * 8)
        .map(|i| if (i / 2) % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let truth = *true_peak(&x, "kr").last().unwrap();
    assert!(
        truth > 1.3,
        "the block's reconstruction, not one sample: {truth}"
    );
}

/// It is never below the sample meter on the same signal: every sample lies on
/// the reconstructed curve.
#[test]
fn a_true_peak_is_never_below_the_sample_meter() {
    let x: Vec<f32> = (0..BLOCK_SIZE * 16)
        .map(|i| 0.7 * (i as f32 * 0.37).sin() + 0.2 * (i as f32 * 2.1).sin())
        .collect();
    let sample = meter(&x, 20.0, 1.0);
    let truth = true_peak(&x, "ar");
    for (s, t) in sample.iter().zip(&truth).step_by(BLOCK_SIZE) {
        assert!(t >= &(s - 1e-4), "true peak {t} under the sample peak {s}");
    }
}
