//! The demand family (U8): the pull protocol, the sources and the drivers.
//!
//! Two harnesses, because the family has two things to check. [`Values`] drives
//! a source directly through the [`DemandInputs`] trait — that is where a
//! stochastic stream's seed is reachable, so reproducibility and distribution
//! are tested there. Everything about *wiring* — nesting, resets propagating,
//! a driver's clock — goes through a real def and [`stream`], where an
//! `Impulse` clocks a `Demand` as fast as a trigger can be clocked and the
//! audio bus simply *is* the stream, one item every other frame.
//!
//! A driver's period is asserted **within a sample** rather than exactly: a
//! duration in seconds is an `f32` on the wire, so a "ten sample" period is
//! 10.000002 samples and the events land a sample late now and then. Pinning
//! exact indices would be pinning that rounding, not the behaviour.
//!
//! On the testing rules this family inherits: there is no filter and no
//! oscillator here, so the analytic-response and alias-SNR rules do not apply.
//! The long-run numerical rule does — [`Duty`]'s countdown is an accumulator,
//! exactly the kind of state that reads correctly for a second and drifts over
//! a minute — and so does the block-split rule, since a driver's clock has to
//! survive a scheduled bundle cutting its block in two.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::buffer::Buffer;
use clausters::dsp::demand::{Drandom, RandKind};
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, DemandInputs, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};

const SR: f32 = 48_000.0;

// ---------------------------------------------------------------------------
// Harnesses
// ---------------------------------------------------------------------------

/// A source's inputs as plain numbers: no nesting, so `pull` and `at` both just
/// read. Enough to drive one source in isolation, which is what the stochastic
/// tests need (the registry seeds an instance from a shared counter; a seed of
/// our own is only reachable here).
struct Values(Vec<f32>);

impl DemandInputs for Values {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_demand(&self, _k: usize) -> bool {
        false
    }
    fn pull(&mut self, k: usize) -> f32 {
        self.0.get(k).copied().unwrap_or(f32::NAN)
    }
    fn reset(&mut self, _k: usize) {}
    fn at(&self, k: usize) -> f32 {
        self.0.get(k).copied().unwrap_or(0.0)
    }
    fn seek(&mut self, _frame: usize) {}
}

/// `n` pulls of `source`, driven directly.
fn pulls(source: &mut dyn clausters::dsp::UGen, inputs: &[f32], n: usize) -> Vec<f32> {
    let buses = Buses::new(ControlBuses::new(16), 8);
    let ctx = ProcessCtx {
        sample_rate: SR,
        full_sample_rate: SR,
        buses: &buses,
        buffers: &[],
        offset: 0,
        frames: BLOCK_SIZE,
    };
    let mut vals = Values(inputs.to_vec());
    (0..n).map(|_| source.demand(&ctx, &mut vals)).collect()
}

/// One UGen of a def, as wire JSON: `kind` with a list of constant inputs.
fn ugen(kind: &str, args: &[f32]) -> String {
    let inputs: Vec<String> = args.iter().map(|a| format!(r#"{{"const":{a}}}"#)).collect();
    format!(
        r#"{{"kind":"{kind}","rate":"dr","inputs":[{}]}}"#,
        inputs.join(",")
    )
}

/// The same, but taking some inputs from earlier UGens: `wires` names the input
/// positions that are wires and which UGen each names.
fn ugen_wired(kind: &str, args: &[f32], wires: &[(usize, usize)]) -> String {
    let mut inputs: Vec<String> = args.iter().map(|a| format!(r#"{{"const":{a}}}"#)).collect();
    for &(slot, w) in wires {
        inputs[slot] = format!(r#"{{"ugen":{w}}}"#);
    }
    format!(
        r#"{{"kind":"{kind}","rate":"dr","inputs":[{}]}}"#,
        inputs.join(",")
    )
}

/// Renders `frames` samples of audio bus 0 from a def.
fn render(json: &str, frames: usize, buffers: &[Option<Arc<Buffer>>]) -> Vec<f32> {
    let def = compile(
        serde_json::from_str::<SynthDefSpec>(json).unwrap_or_else(|e| {
            panic!("bad json: {e}\n{json}");
        }),
    )
    .unwrap_or_else(|e| panic!("compile: {e}\n{json}"));
    let mut synth = UGenSynth::new(Arc::new(def), SR, SEED_STRIDE);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames.div_ceil(BLOCK_SIZE) {
        buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses: &buses,
            buffers,
            offset: 0,
            frames: BLOCK_SIZE,
        };
        synth.process(&mut ctx);
        out.extend_from_slice(buses.audio(0));
    }
    out.truncate(frames);
    out
}

/// The first `n` items of a demand stream, one every other sample.
///
/// `ugens` is the demand sub-graph (UGen 0 upwards); `src` names the one the
/// driver pulls. The clock is an `Impulse` at **half** the sample rate: a
/// trigger is a rising edge, and an impulse train at the sample rate is a
/// constant 1 with a single edge in it, so the fastest a `Demand` can be pulled
/// is every second frame. Every second sample of the bus is therefore one item.
/// Once the source is exhausted `Demand` holds its last value, which is how an
/// ended stream shows up here: as a plateau.
fn stream(ugens: &[String], src: usize, n: usize) -> Vec<f32> {
    stream_over(ugens, src, n, &[])
}

/// [`stream`], with a buffer pool for the sources that read one.
fn stream_over(ugens: &[String], src: usize, n: usize, bufs: &[Option<Arc<Buffer>>]) -> Vec<f32> {
    let imp = ugens.len();
    let mut all: Vec<String> = ugens.to_vec();
    all.push(format!(
        r#"{{"kind":"Impulse","inputs":[{{"const":{}}}]}}"#,
        SR / 2.0
    ));
    all.push(format!(
        r#"{{"kind":"Demand","inputs":[{{"ugen":{imp}}},{{"const":0.0}},{{"ugen":{src}}}]}}"#
    ));
    all.push(format!(
        r#"{{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":{}}}]}}"#,
        imp + 1
    ));
    let json = format!(r#"{{"name":"x","ugens":[{}]}}"#, all.join(","));
    render(&json, 2 * n, bufs)
        .iter()
        .step_by(2)
        .copied()
        .collect()
}

/// A single-source stream, the common shape.
fn stream1(kind: &str, args: &[f32], n: usize) -> Vec<f32> {
    stream(&[ugen(kind, args)], 0, n)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(b.abs()).max(1.0)
}

fn assert_finite(xs: &[f32], what: &str) {
    assert!(
        xs.iter().all(|x| x.is_finite()),
        "{what}: non-finite sample in {:?}",
        &xs[..xs.len().min(16)]
    );
}

// ---------------------------------------------------------------------------
// The ramps: Dseries, Dgeom
// ---------------------------------------------------------------------------

#[test]
fn dseries_counts_by_its_step() {
    let v = stream1("Dseries", &[4.0, 10.0, 2.5], 6);
    assert_eq!(&v[..4], &[10.0, 12.5, 15.0, 17.5]);
    // Exhausted after four: the driver holds the last value it got.
    assert_eq!(&v[4..6], &[17.5, 17.5]);
}

#[test]
fn dgeom_multiplies_by_its_growth() {
    let v = stream1("Dgeom", &[4.0, 1.0, 0.5], 4);
    assert_eq!(v, vec![1.0, 0.5, 0.25, 0.125]);
}

#[test]
fn a_negative_step_walks_backwards() {
    let v = stream1("Dseries", &[3.0, 0.0, -1.0], 3);
    assert_eq!(v, vec![0.0, -1.0, -2.0]);
}

#[test]
fn repeats_of_zero_is_the_endless_stream() {
    // sclang says `inf`; a def cannot carry one (`compile` rejects a non-finite
    // constant, and JSON has no spelling for it), so a count of none is the
    // endless stream here. Two hundred items and still counting.
    let v = stream1("Dseries", &[0.0, 0.0, 1.0], 200);
    assert_eq!(v[199], 199.0);
}

#[test]
fn a_count_is_rounded_rather_than_truncated() {
    // A stream carries floats; a count of 2.9999998 is a count of three.
    let v = stream1("Dseries", &[2.9999998, 0.0, 1.0], 5);
    assert_eq!(&v[..3], &[0.0, 1.0, 2.0]);
    assert_eq!(v[3], 2.0, "the third was the last");
}

// ---------------------------------------------------------------------------
// The stochastic sources
// ---------------------------------------------------------------------------

#[test]
fn the_same_seed_replays_the_same_stream() {
    // The property a reproducible render rests on. Not reachable through a def
    // (an instance seeds itself from the shared counter), so it is asserted on
    // the source itself.
    for kind in [
        RandKind::White,
        RandKind::IWhite,
        RandKind::Brown,
        RandKind::IBrown,
    ] {
        let inputs = [0.0, 0.0, 8.0, 2.0];
        let a = pulls(&mut Drandom::with_seed(kind, 12345), &inputs, 64);
        let b = pulls(&mut Drandom::with_seed(kind, 12345), &inputs, 64);
        let c = pulls(&mut Drandom::with_seed(kind, 12346), &inputs, 64);
        assert_eq!(a, b, "same seed, same stream");
        assert_ne!(a, c, "a different seed is a different stream");
    }
}

#[test]
fn dwhite_is_uniform_between_its_bounds() {
    // Uniform on [2, 6]: mean 4, variance (hi-lo)^2/12 = 4/3. Both are closed
    // form, so the assert is the theory rather than a recorded run; the
    // tolerance is what 100k samples of a uniform give (the standard error of
    // the mean is (hi-lo)/sqrt(12n) ~ 0.004, so 0.05 is ten sigma).
    let n = 100_000;
    let v = pulls(
        &mut Drandom::with_seed(RandKind::White, 7),
        &[0.0, 2.0, 6.0],
        n,
    );
    assert!(v.iter().all(|x| (2.0..=6.0).contains(x)), "within bounds");
    let mean = v.iter().sum::<f32>() / n as f32;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n as f32;
    assert!((mean - 4.0).abs() < 0.05, "mean {mean}");
    assert!((var - 4.0 / 3.0).abs() < 0.05, "variance {var}");
}

#[test]
fn diwhite_covers_both_ends_of_its_range() {
    // Integers on [1, 4] inclusive — four values, each about a quarter of the
    // draws. The inclusive upper end is the part that is easy to get wrong.
    let n = 20_000;
    let v = pulls(
        &mut Drandom::with_seed(RandKind::IWhite, 3),
        &[0.0, 1.0, 4.0],
        n,
    );
    assert!(v.iter().all(|x| x.fract() == 0.0), "integers only");
    for want in [1.0, 2.0, 3.0, 4.0] {
        let share = v.iter().filter(|x| **x == want).count() as f32 / n as f32;
        assert!(
            (share - 0.25).abs() < 0.02,
            "{want} appeared {share} of the time"
        );
    }
}

#[test]
fn dbrown_steps_no_further_than_it_was_told() {
    let step = 0.5;
    let v = pulls(
        &mut Drandom::with_seed(RandKind::Brown, 11),
        &[0.0, -1.0, 1.0, step],
        10_000,
    );
    assert!(v.iter().all(|x| (-1.0..=1.0).contains(x)), "inside bounds");
    let worst = v
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(worst <= step + 1e-6, "largest step {worst}");
}

#[test]
fn a_walk_folds_at_the_bound_rather_than_piling_up_against_it() {
    // The reason the walk folds instead of clipping: a clipped walk parks on
    // its bounds, and the histogram says so. With a step a fifth of the range,
    // no value should be within a hundredth of a bound more than a few percent
    // of the time — a clipping walk sits there roughly a third of it.
    let n = 20_000;
    let v = pulls(
        &mut Drandom::with_seed(RandKind::Brown, 5),
        &[0.0, 0.0, 1.0, 0.2],
        n,
    );
    let parked = v.iter().filter(|x| **x < 0.01 || **x > 0.99).count() as f32 / n as f32;
    assert!(parked < 0.06, "sat on a bound {parked} of the time");
}

#[test]
fn dibrown_walks_on_the_integers() {
    let v = pulls(
        &mut Drandom::with_seed(RandKind::IBrown, 9),
        &[0.0, 0.0, 10.0, 3.0],
        2_000,
    );
    assert!(v.iter().all(|x| x.fract() == 0.0), "integers only");
    assert!(v.iter().all(|x| (0.0..=10.0).contains(x)), "inside bounds");
    let worst = v
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(worst <= 3.0, "largest step {worst}");
}

#[test]
fn a_stochastic_source_counts_its_items() {
    let v = stream1("Dwhite", &[3.0, 5.0, 5.0], 5);
    assert_eq!(&v[..3], &[5.0, 5.0, 5.0]);
}

// ---------------------------------------------------------------------------
// The list sources: Dseq, Drand, Dxrand, Dshuf
// ---------------------------------------------------------------------------

#[test]
fn dseq_walks_its_list_and_repeats_it() {
    let v = stream1("Dseq", &[2.0, 10.0, 20.0, 30.0], 8);
    assert_eq!(&v[..6], &[10.0, 20.0, 30.0, 10.0, 20.0, 30.0]);
    assert_eq!(&v[6..8], &[30.0, 30.0], "two passes, then held");
}

#[test]
fn dseq_flattens_a_nested_stream() {
    // The point of the family: a value may be a phrase. `Dseq(1, Dseries(3, 0,
    // 1), 100)` is four items, not two.
    let g = [
        ugen("Dseries", &[3.0, 0.0, 1.0]),
        ugen_wired("Dseq", &[1.0, 0.0, 100.0], &[(1, 0)]),
    ];
    let v = stream(&g, 1, 5);
    assert_eq!(&v[..4], &[0.0, 1.0, 2.0, 100.0]);
}

#[test]
fn a_list_restarts_the_child_it_comes_back_to() {
    // Two passes over a list whose only value is a three-item stream. Without
    // the reset the second pass would find it spent and the whole thing would
    // stop after three.
    let g = [
        ugen("Dseries", &[3.0, 0.0, 1.0]),
        ugen_wired("Dseq", &[2.0, 0.0], &[(1, 0)]),
    ];
    let v = stream(&g, 1, 7);
    assert_eq!(&v[..6], &[0.0, 1.0, 2.0, 0.0, 1.0, 2.0]);
}

#[test]
fn a_list_slot_is_drained_rather_than_sampled_once() {
    // A nested stream is not one item of the list: the parent stays on the slot
    // until it answers `NaN`. So a three-item series followed by a constant is
    // a four-item pass, and the reset on the way round makes the next pass the
    // same one — 0 1 2 9, again and again, not 0 9 1 9.
    let g = [
        ugen("Dseries", &[3.0, 0.0, 1.0]),
        ugen_wired("Dseq", &[0.0, 0.0, 9.0], &[(1, 0)]),
    ];
    let v = stream(&g, 1, 8);
    assert_eq!(v, vec![0.0, 1.0, 2.0, 9.0, 0.0, 1.0, 2.0, 9.0]);
}

#[test]
fn drand_counts_items_where_dseq_counts_passes() {
    // scsynth's own asymmetry, kept because each is the useful reading: a
    // shuffle without a full pass would not be one, and a random pick has no
    // pass to complete. Four items out of a three-value list.
    let v = stream1("Drand", &[4.0, 7.0, 7.0, 7.0], 6);
    assert_eq!(&v[..4], &[7.0, 7.0, 7.0, 7.0]);
    assert_eq!(v[4], 7.0, "then held");
}

#[test]
fn drand_draws_from_the_whole_list() {
    let v = stream1("Drand", &[0.0, 1.0, 2.0, 3.0], 600);
    assert!(v.iter().all(|x| (1.0..=3.0).contains(x)));
    for want in [1.0, 2.0, 3.0] {
        assert!(v.contains(&want), "{want} never came up in 600 draws");
    }
}

#[test]
fn dxrand_never_picks_the_slot_it_just_used() {
    // The one property that distinguishes it from `Drand`.
    let v = stream1("Dxrand", &[0.0, 1.0, 2.0, 3.0], 400);
    assert!(
        v.windows(2).all(|w| w[0] != w[1]),
        "a value repeated back to back"
    );
    for want in [1.0, 2.0, 3.0] {
        assert!(v.contains(&want), "{want} never came up");
    }
}

#[test]
fn dshuf_replays_one_order() {
    // A shuffle is drawn once per stream, not once per pass — that is what
    // separates it from `Drand`. Four passes over four values: a permutation,
    // and the same one every time.
    let v = stream1("Dshuf", &[0.0, 1.0, 2.0, 3.0, 4.0], 16);
    let first: Vec<f32> = v[..4].to_vec();
    let mut sorted = first.clone();
    sorted.sort_by(f32::total_cmp);
    assert_eq!(
        sorted,
        vec![1.0, 2.0, 3.0, 4.0],
        "a permutation of the list"
    );
    assert_eq!(&v[4..8], &first[..], "the same order again");
    assert_eq!(&v[12..16], &first[..], "and again");
}

#[test]
fn an_empty_list_yields_nothing() {
    let v = stream1("Dseq", &[0.0], 4);
    assert!(v.iter().all(|x| *x == 0.0), "never got a value: {v:?}");
}

// ---------------------------------------------------------------------------
// Dstutter, Dswitch1, Dbufrd
// ---------------------------------------------------------------------------

#[test]
fn dstutter_repeats_each_item() {
    let g = [
        ugen("Dseries", &[3.0, 0.0, 1.0]),
        ugen_wired("Dstutter", &[3.0, 0.0], &[(1, 0)]),
    ];
    let v = stream(&g, 1, 9);
    assert_eq!(v, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0]);
}

#[test]
fn a_stutter_count_may_itself_be_a_stream() {
    // One repeat of the first item, three of the second.
    let g = [
        ugen("Dseq", &[0.0, 1.0, 3.0]),
        ugen("Dseries", &[0.0, 10.0, 10.0]),
        ugen_wired("Dstutter", &[0.0, 0.0], &[(0, 0), (1, 1)]),
    ];
    let v = stream(&g, 2, 8);
    assert_eq!(v, vec![10.0, 20.0, 20.0, 20.0, 30.0, 40.0, 40.0, 40.0]);
}

#[test]
fn dswitch1_takes_one_item_from_the_branch_it_picks() {
    // The `1` in the name. The index alternates between two series, and each
    // keeps its own place — an unselected branch is not advanced, which is what
    // separates this from a list source draining a slot.
    let g = [
        ugen("Dseq", &[0.0, 0.0, 1.0]),
        ugen("Dseries", &[0.0, 0.0, 1.0]),
        ugen("Dseries", &[0.0, 100.0, 1.0]),
        ugen_wired("Dswitch1", &[0.0, 0.0, 0.0], &[(0, 0), (1, 1), (2, 2)]),
    ];
    let v = stream(&g, 3, 6);
    assert_eq!(v, vec![0.0, 100.0, 1.0, 101.0, 2.0, 102.0]);
}

#[test]
fn dswitch1_wraps_an_index_off_the_end() {
    // A modulated index cannot fall off a list: -1 is the last branch, 2 the
    // first of two.
    let g = [
        ugen("Dseq", &[0.0, -1.0, 2.0]),
        ugen_wired("Dswitch1", &[0.0, 5.0, 6.0], &[(0, 0)]),
    ];
    let v = stream(&g, 1, 4);
    assert_eq!(v, vec![6.0, 5.0, 6.0, 5.0]);
}

#[test]
fn dbufrd_reads_the_frame_its_phase_names() {
    // A step sequencer: a series of frame indices reading a buffer of pitches.
    let data: Vec<f32> = (0..8).map(|i| 100.0 + i as f32).collect();
    let buffers = [Some(Arc::new(Buffer::new(data, 1, 8, SR as f64)))];
    let g = [
        ugen("Dseries", &[0.0, 0.0, 1.0]),
        ugen_wired("Dbufrd", &[0.0, 0.0, 1.0, 0.0], &[(1, 0)]),
    ];
    let v = stream_over(&g, 1, 10, &buffers);
    assert_eq!(
        &v[..8],
        &[100.0, 101.0, 102.0, 103.0, 104.0, 105.0, 106.0, 107.0]
    );
    // Looping: frame 8 wraps back to 0.
    assert_eq!(&v[8..10], &[100.0, 101.0]);
}

#[test]
fn dbufrd_not_looping_holds_the_last_frame_instead_of_wrapping() {
    // The other half of the `loop` input, and the one a step sequencer that is
    // meant to *stop* depends on. Past the end it clamps to the last frame;
    // before the start, to the first. Wrapping there would silently restart a
    // phrase, which is the failure that sounds like a bug in the pattern
    // rather than in the buffer read.
    let data: Vec<f32> = (0..8).map(|i| 100.0 + i as f32).collect();
    let buffers = [Some(Arc::new(Buffer::new(data, 1, 8, SR as f64)))];
    let g = [
        ugen("Dseries", &[0.0, 5.0, 1.0]),
        ugen_wired("Dbufrd", &[0.0, 0.0, 0.0, 0.0], &[(1, 0)]),
    ];
    let v = stream_over(&g, 1, 6, &buffers);
    // Frames 5, 6, 7, then 8, 9, 10 — all held at the last one.
    assert_eq!(v, vec![105.0, 106.0, 107.0, 107.0, 107.0, 107.0]);

    // And off the front, where a `rem_euclid` would have jumped to the end.
    let data: Vec<f32> = (0..8).map(|i| 100.0 + i as f32).collect();
    let buffers = [Some(Arc::new(Buffer::new(data, 1, 8, SR as f64)))];
    let g = [
        ugen("Dseries", &[0.0, -3.0, 1.0]),
        ugen_wired("Dbufrd", &[0.0, 0.0, 0.0, 0.0], &[(1, 0)]),
    ];
    let v = stream_over(&g, 1, 5, &buffers);
    assert_eq!(v, vec![100.0, 100.0, 100.0, 100.0, 101.0]);
}

// ---------------------------------------------------------------------------
// The compile-time refusal
// ---------------------------------------------------------------------------

#[test]
fn nesting_deeper_than_the_limit_is_refused_at_compile_time() {
    // A pull recurses once per level, inside the audio callback, so the depth
    // is a *compile* error rather than a runtime guard: the honest place to
    // say no is where a human is still watching, not on the thread that would
    // run off its stack. The limit is 16, and both sides of it are checked —
    // a cap nothing reaches is not a cap, and a cap that fires one level early
    // costs a legitimate def.
    let chain = |depth: usize| {
        let mut ugens = vec![ugen("Dseries", &[0.0, 0.0, 1.0])];
        // Each Dstutter takes the one before as its value, so the chain is
        // exactly `depth` levels of demand nesting.
        for i in 0..depth {
            ugens.push(ugen_wired("Dstutter", &[1.0, 0.0], &[(1, i)]));
        }
        let src = ugens.len() - 1;
        let imp = ugens.len();
        ugens.push(format!(
            r#"{{"kind":"Impulse","inputs":[{{"const":{}}}]}}"#,
            SR / 2.0
        ));
        ugens.push(format!(
            r#"{{"kind":"Demand","inputs":[{{"ugen":{imp}}},{{"const":0.0}},{{"ugen":{src}}}]}}"#
        ));
        ugens.push(format!(
            r#"{{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":{}}}]}}"#,
            imp + 1
        ));
        let json = format!(r#"{{"name":"deep","ugens":[{}]}}"#, ugens.join(","));
        compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap())
    };

    // 15 Dstutters over the Dseries is depth 15 at the last of them, 16 at the
    // driver: right at the limit, and it must build.
    assert!(
        chain(15).is_ok(),
        "a def at the limit must still compile: {:?}",
        chain(15).err()
    );

    // One more and it is refused, with a message naming the depth and the
    // limit rather than a bare failure.
    let err = chain(16).expect_err("nesting past the limit must be refused");
    // Both numbers, so the message stays diagnostic rather than merely
    // present: "ugens[18] (Demand): demand streams nested 17 deep; the limit
    // is 16".
    assert!(
        err.contains("nested 17 deep") && err.contains("the limit is 16"),
        "the refusal should say how deep it got and what the limit is: {err}"
    );
}

// ---------------------------------------------------------------------------
// The drivers
// ---------------------------------------------------------------------------

/// A `Duty`/`TDuty` def with a constant duration in seconds and a `Dseries` of
/// levels, rendered straight to the bus.
fn duty(kind: &str, dur: f32, extra: &[f32], frames: usize) -> Vec<f32> {
    let mut args = vec![dur, 0.0, 0.0, 0.0];
    args.extend_from_slice(extra);
    let inputs: Vec<String> = args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if i == 2 {
                r#"{"ugen":0}"#.to_string()
            } else {
                format!(r#"{{"const":{a}}}"#)
            }
        })
        .collect();
    let json = format!(
        r#"{{"name":"x","ugens":[{},{{"kind":"{kind}","inputs":[{}]}},
            {{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":1}}]}}]}}"#,
        ugen("Dseries", &[0.0, 1.0, 1.0]),
        inputs.join(",")
    );
    render(&json, frames, &[])
}

/// Every sample where the signal is non-zero, with its value — a `TDuty`'s
/// triggers.
fn fires(v: &[f32]) -> Vec<(usize, f32)> {
    v.iter()
        .enumerate()
        .filter(|(_, s)| **s != 0.0)
        .map(|(i, s)| (i, *s))
        .collect()
}

/// Every sample where a held signal takes a new value — a `Duty`'s steps.
fn steps(v: &[f32]) -> Vec<(usize, f32)> {
    let mut out = Vec::new();
    for (i, s) in v.iter().enumerate() {
        if i == 0 || v[i - 1] != *s {
            out.push((i, *s));
        }
    }
    out
}

/// Asserts that `events` are the levels `1, 2, 3, …` a period apart, each
/// within a sample of where the exact period puts it.
fn assert_periodic(events: &[(usize, f32)], period: f32, what: &str) {
    for (n, &(i, level)) in events.iter().enumerate() {
        assert_eq!(level, n as f32 + 1.0, "{what}: level of event {n}");
        let want = n as f32 * period;
        assert!(
            (i as f32 - want).abs() <= 1.0,
            "{what}: event {n} at {i}, exact {want}"
        );
    }
}

#[test]
fn duty_pulls_on_its_own_clock_and_holds_between_pulls() {
    // Ten samples per item at 48 kHz: three steps in the first thirty samples,
    // and the value in between is the one it last pulled.
    let v = duty("Duty", 10.0 / SR, &[], 32);
    let steps = steps(&v);
    assert_eq!(steps.len(), 4, "four levels in 32 samples: {steps:?}");
    assert_periodic(&steps, 10.0, "Duty");
    assert!(v[..8].iter().all(|s| *s == 1.0), "held between pulls");
}

#[test]
fn tduty_is_silent_between_its_triggers() {
    // The one difference from `Duty`: the level appears on its own sample and
    // nowhere else, so the output is a trigger stream rather than a staircase.
    let v = duty("TDuty", 10.0 / SR, &[0.0], 32);
    let fires = fires(&v);
    assert_eq!(fires.len(), 4, "four triggers in 32 samples: {fires:?}");
    assert_periodic(&fires, 10.0, "TDuty");
}

#[test]
fn tduty_can_open_with_a_gap_instead_of_a_trigger() {
    // `gap_first`: the first duration is spent before the first level is pulled
    // at all, so the stream starts one period late and still starts at its
    // first value — the trigger near sample 10 is level 1, not level 2.
    let v = duty("TDuty", 10.0 / SR, &[1.0], 32);
    let fires = fires(&v);
    assert!(v[..9].iter().all(|s| *s == 0.0), "opened with a gap");
    assert_eq!(fires[0].1, 1.0, "the first level, one period late");
    assert!((fires[0].0 as i32 - 10).abs() <= 1, "at {}", fires[0].0);
    assert_eq!(fires[1].1, 2.0);
}

#[test]
fn a_driver_stops_when_its_level_stream_runs_out() {
    // Three levels, then nothing: `Duty` holds the last one rather than asking
    // an exhausted stream for a value on every sample from then on.
    let json = format!(
        r#"{{"name":"x","ugens":[{},
            {{"kind":"Duty","inputs":[{{"const":{}}},{{"const":0.0}},{{"ugen":0}},{{"const":0.0}}]}},
            {{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":1}}]}}]}}"#,
        ugen("Dseries", &[3.0, 1.0, 1.0]),
        10.0 / SR
    );
    let v = render(&json, 64, &[]);
    assert_eq!(steps(&v).len(), 3, "three levels and no more");
    assert!(v[32..].iter().all(|s| *s == 3.0), "held after the end");
}

#[test]
fn duty_does_not_drift_over_ten_seconds() {
    // The long-run numerical test this family's one accumulator asks for. The
    // period is deliberately not a whole number of samples (1/300 s is 160
    // samples at 48 kHz — so use a prime-ish rate instead): rounding it per
    // item instead of carrying the remainder in `f64` would put the last event
    // of ten seconds several hundred samples off.
    let dur = 1.0 / 311.0;
    let seconds = 10.0;
    let frames = (SR * seconds) as usize;
    let json = format!(
        r#"{{"name":"x","ugens":[{},
            {{"kind":"TDuty","inputs":[{{"const":{dur}}},{{"const":0.0}},{{"const":1.0}},
              {{"const":0.0}},{{"const":0.0}}]}},
            {{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":1}}]}}]}}"#,
        ugen("Dseries", &[0.0, 1.0, 1.0])
    );
    let v = render(&json, frames, &[]);
    let fires: Vec<usize> = v
        .iter()
        .enumerate()
        .filter(|(_, s)| **s != 0.0)
        .map(|(i, _)| i)
        .collect();
    let period = dur * SR;
    assert!(fires.len() > 3000, "only {} triggers", fires.len());
    // Every trigger within a sample of where the exact period puts it — the
    // whole ten seconds, not just the first.
    for (n, &i) in fires.iter().enumerate() {
        let want = n as f32 * period;
        assert!(
            (i as f32 - want).abs() <= 1.0,
            "trigger {n} at {i}, exact {want}"
        );
    }
}

#[test]
fn a_reset_restarts_every_stream_a_driver_pulls() {
    // The reset edge reaches both `dur` and `level`, and through them whatever
    // they nest. Reset is a trigger control set from outside.
    let json = format!(
        r#"{{"name":"x","controls":[{{"name":"reset","default":0.0,"rate":"tr"}}],"ugens":[{},
            {{"kind":"Duty","inputs":[{{"const":{}}},{{"control":0}},{{"ugen":0}},{{"const":0.0}}]}},
            {{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":1}}]}}]}}"#,
        ugen("Dseries", &[0.0, 1.0, 1.0]),
        10.0 / SR
    );
    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR, SEED_STRIDE);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let run = |synth: &mut UGenSynth, buses: &mut Buses| {
        buses.clear_audio();
        let mut ctx = ProcessCtx {
            sample_rate: SR,
            full_sample_rate: SR,
            buses,
            buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
        };
        synth.process(&mut ctx);
    };
    run(&mut synth, &mut buses);
    assert_eq!(buses.audio(0)[0], 1.0);
    assert_eq!(buses.audio(0)[63], 7.0, "six items into the block");
    synth.set_control(0, 1.0);
    run(&mut synth, &mut buses);
    assert_eq!(buses.audio(0)[0], 1.0, "back to the top of the series");
}

// ---------------------------------------------------------------------------
// Structural properties (the rules the testing skill asks for)
// ---------------------------------------------------------------------------

#[test]
fn a_split_block_renders_the_same_samples() {
    // A scheduled bundle cuts a block in two; a driver's countdown must not
    // notice. Rendered whole against rendered in pieces.
    let json = format!(
        r#"{{"name":"x","ugens":[{},
            {{"kind":"Duty","inputs":[{{"const":{}}},{{"const":0.0}},{{"ugen":0}},{{"const":0.0}}]}},
            {{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":1}}]}}]}}"#,
        ugen("Dseq", &[0.0, 3.0, 5.0, 8.0]),
        7.0 / SR
    );
    let whole = render(&json, 4 * BLOCK_SIZE, &[]);

    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR, SEED_STRIDE);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut split = Vec::new();
    for _ in 0..4 {
        buses.clear_audio();
        for (offset, frames) in [(0, 13), (13, 20), (33, 31)] {
            let mut ctx = ProcessCtx {
                sample_rate: SR,
                full_sample_rate: SR,
                buses: &buses,
                buffers: &[],
                offset,
                frames,
            };
            synth.process(&mut ctx);
        }
        split.extend_from_slice(buses.audio(0));
    }
    assert_eq!(whole, split);
}

#[test]
fn no_input_produces_a_non_finite_sample() {
    // Adversarial: an empty list, a zero duration (a pull every sample), a
    // duration far past any block, an index nowhere near the list, a walk with
    // no width to fold into, a buffer that is not there.
    assert_finite(&stream1("Dseq", &[0.0], 128), "empty list");
    assert_finite(&stream1("Dseries", &[1e30, 1e30, 1e30], 128), "huge series");
    assert_finite(
        &stream1("Dgeom", &[0.0, 1e30, 1e30], 128),
        "runaway geometric",
    );
    assert_finite(
        &stream1("Dwhite", &[0.0, 5.0, -5.0], 128),
        "bounds inverted",
    );
    assert_finite(
        &stream1("Dbrown", &[0.0, 1.0, 1.0, 0.0], 128),
        "no width to walk",
    );
    assert_finite(
        &stream1("Dibrown", &[0.0, 0.0, 0.0, 1e30], 128),
        "no width, huge step",
    );
    assert_finite(
        &stream1("Dstutter", &[-5.0, 1.0], 128),
        "negative repeat count",
    );
    assert_finite(
        &stream1("Dswitch1", &[1e30, 1.0, 2.0], 128),
        "index off the map",
    );
    assert_finite(
        &stream1("Dbufrd", &[9999.0, 1e30, 1.0, 0.0], 128),
        "no such buffer",
    );
    assert_finite(&duty("Duty", 0.0, &[], 128), "zero duration");
    assert_finite(&duty("Duty", -1.0, &[], 128), "negative duration");
    assert_finite(&duty("TDuty", 1e30, &[0.0], 128), "duration past the block");
}

#[test]
fn a_pull_reads_a_modulated_input_at_the_sample_it_happens_on() {
    // An ordinary (`ar`) input of a demand UGen is read at the frame the pull
    // lands on, not at the top of the block — the frame propagates down the
    // nesting, scsynth's `inNumSamples` in spirit. A `Duty` whose level is a
    // ramping signal therefore steps up across the block.
    let json = format!(
        r#"{{"name":"x","ugens":[
            {{"kind":"Line","inputs":[{{"const":0.0}},{{"const":64.0}},{{"const":{}}},{{"const":0.0}}]}},
            {{"kind":"Duty","inputs":[{{"const":{}}},{{"const":0.0}},{{"ugen":0}},{{"const":0.0}}]}},
            {{"kind":"Out","inputs":[{{"const":0.0}},{{"ugen":1}}]}}]}}"#,
        64.0 / SR,
        8.0 / SR
    );
    let v = render(&json, 64, &[]);
    assert!(
        close(v[0], 0.0),
        "first pull at the top of the ramp: {}",
        v[0]
    );
    assert!(
        v[8] > v[0] && v[16] > v[8],
        "each pull reads further along the ramp"
    );
    assert!(
        close(v[8], 8.0),
        "the pull at sample 8 read the ramp there: {}",
        v[8]
    );
}
