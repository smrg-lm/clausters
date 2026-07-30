//! Triggers and control (U5): the UGens that read a rising edge, plus the two
//! envelope followers.
//!
//! Most of these are state machines, so the asserts are about *when* something
//! happened rather than about a spectrum: the sample a pulse starts on, the
//! number of triggers it takes to come round, the exact interval a timer
//! reports. The three that are numeric — `Decay`, `Decay2`, `Timer`'s
//! sub-sample crossing — are checked against the closed form, per the rules in
//! the `audio-testing` skill.
//!
//! Rule 5 comes from two places. The shared table splits every one of the
//! seventeen rows in two (`tests/subjects.rs`); `a_counter_is_unmoved_by_a_
//! block_split` below stays because it does something the table does not —
//! nine slices per block rather than one cut — which is the shape a burst of
//! scheduled events actually produces.

#![cfg(feature = "synth")]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{Cmd, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};

const SR: f32 = 48_000.0;

/// Renders `n` samples of a def given as its ugen list; the **last** ugen is
/// written to bus 0.
fn render(ugens: &str, n: usize) -> Vec<f32> {
    let count = ugens.matches("\"kind\"").count();
    let json = format!(
        r#"{{"name": "t", "ugens": [{ugens},
            {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": {}}}]}}]}}"#,
        count - 1
    );
    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR, SEED_STRIDE);
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

/// A single impulse at sample 0, scaled — `Impulse` at 0 Hz fires once and
/// never re-arms, which is the cleanest trigger a test can ask for.
fn one_shot(level: f32) -> String {
    format!(
        r#"{{"kind": "Impulse", "inputs": [{{"const": 0.0}}]}},
           {{"kind": "Mul", "inputs": [{{"ugen": 0}}, {{"const": {level}}}]}}"#
    )
}

/// The sample indices where a trigger source actually fires.
///
/// `Impulse` accumulates its phase in `f64`, so a 100 Hz train at 48 kHz lands
/// on 0, 481, 961 … rather than on exact multiples of 480 — a tenth of a
/// sample of drift per period, which is the oscillator being honest and not
/// something the counters under test should be asserted against. Every test
/// below that needs "the fourth trigger" asks the train where it is.
fn trigger_samples(freq: f32, n: usize) -> Vec<usize> {
    edges(&render(
        &format!(r#"{{"kind": "Impulse", "inputs": [{{"const": {freq}}}]}}"#),
        n,
    ))
}

/// Seconds for `n` samples at the test rate.
fn secs(n: usize) -> f32 {
    n as f32 / SR
}

/// Indices where the signal rises through 0.5 — a trigger, counted the way the
/// server counts one.
fn edges(sig: &[f32]) -> Vec<usize> {
    let mut prev = 0.0;
    let mut out = Vec::new();
    for (i, &s) in sig.iter().enumerate() {
        if s > 0.5 && prev <= 0.5 {
            out.push(i);
        }
        prev = s;
    }
    out
}

// ---- the timed pulses: Trig, Trig1, TDelay ----

#[test]
fn trig_holds_the_triggering_level_for_exactly_its_duration() {
    let dur = secs(10);
    let sig = render(
        &format!(
            r#"{}, {{"kind": "Trig", "inputs": [{{"ugen": 1}}, {{"const": {dur}}}]}}"#,
            one_shot(0.7)
        ),
        64,
    );
    // It is the *level at the trigger* that is held, not 1 — that is the whole
    // difference from Trig1, and it is what makes Trig usable as a sampler.
    for (i, s) in sig.iter().enumerate().take(10) {
        assert!((*s - 0.7).abs() < 1e-6, "sample {i}: {s} != 0.7");
    }
    assert!(
        sig[10..].iter().all(|s| *s == 0.0),
        "silent after 10 samples"
    );
}

#[test]
fn trig1_holds_one_whatever_the_level_was() {
    let dur = secs(10);
    let sig = render(
        &format!(
            r#"{}, {{"kind": "Trig1", "inputs": [{{"ugen": 1}}, {{"const": {dur}}}]}}"#,
            one_shot(0.7)
        ),
        64,
    );
    assert!(
        sig[..10].iter().all(|s| (*s - 1.0).abs() < 1e-6),
        "unit pulse"
    );
    assert!(
        sig[10..].iter().all(|s| *s == 0.0),
        "silent after 10 samples"
    );
}

#[test]
fn tdelay_puts_one_sample_exactly_n_samples_later() {
    let dur = secs(20);
    let sig = render(
        &format!(
            r#"{}, {{"kind": "TDelay", "inputs": [{{"ugen": 1}}, {{"const": {dur}}}]}}"#,
            one_shot(1.0)
        ),
        64,
    );
    assert_eq!(
        edges(&sig),
        vec![20],
        "one pulse, 20 samples after the trigger"
    );
    assert!(
        sig.iter().filter(|s| **s > 0.5).count() == 1,
        "and it lasts exactly one sample"
    );
}

#[test]
fn tdelay_swallows_a_trigger_while_one_is_pending() {
    // 100 Hz of triggers (every 480 samples) through a 20 ms delay (960
    // samples): every other trigger arrives while one is already in flight and
    // must be dropped, not queued — otherwise a burst turns into a pile-up.
    let sig = render(
        &format!(
            r#"{{"kind": "Impulse", "inputs": [{{"const": 100.0}}]}},
               {{"kind": "TDelay", "inputs": [{{"ugen": 0}}, {{"const": {}}}]}}"#,
            secs(960)
        ),
        4800,
    );
    let got = edges(&sig).len();
    // 4800 samples hold 10 input triggers; a 2-trigger-long delay lets through
    // every other one, minus the tail still in flight at the end.
    assert!(
        (4..=5).contains(&got),
        "expected 4-5 delayed pulses, got {got}"
    );
}

// ---- the held values: Latch, Gate ----

#[test]
fn latch_takes_one_sample_per_trigger_where_gate_follows_the_whole_time() {
    // The same ramp into both, gated by the same 50 %-duty square. The Latch
    // freezes the value at each rising edge; the Gate tracks the ramp for as
    // long as the square is high. Two different things, and the names say
    // which is which.
    let ramp = r#"{"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 1.0},
                   {"const": 0.0}, {"const": 4800.0}, {"const": 0.0}]}"#;
    let gate = r#"{"kind": "LFPulse", "inputs": [{"const": 10.0}, {"const": 0.0},
                   {"const": 0.5}]}"#;
    let latched = render(
        &format!(
            r#"{ramp}, {gate},
               {{"kind": "Latch", "inputs": [{{"ugen": 0}}, {{"ugen": 1}}]}}"#
        ),
        4800,
    );
    let gated = render(
        &format!(
            r#"{ramp}, {gate},
               {{"kind": "Gate", "inputs": [{{"ugen": 0}}, {{"ugen": 1}}]}}"#
        ),
        4800,
    );
    // Where the square actually falls, asked rather than assumed.
    let square = render(gate, 4800);
    let close = (1..4800)
        .find(|&i| square[i] <= 0.0)
        .expect("the square closes within the cycle");

    // The latch takes one sample per rising edge and holds it right across the
    // cycle, open or closed...
    assert!(
        latched[1..4800].windows(2).all(|w| w[0] == w[1]),
        "a latch changes only on an edge"
    );
    // ...while the gate climbs with the ramp for as long as the square is up,
    // then freezes at the value it had when it fell.
    assert!(
        gated[..close].windows(2).all(|w| w[1] >= w[0]) && gated[close - 1] > gated[0],
        "the gate follows the ramp while open"
    );
    assert!(
        gated[close..4800]
            .iter()
            .all(|s| (*s - gated[close - 1]).abs() < 1e-6),
        "and freezes at the last value when it closes"
    );
}

// ---- the comparators: Schmidt, Changed ----

#[test]
fn schmidt_needs_the_whole_band_to_change_its_mind() {
    // One cycle of a triangle: 0 -> 1 -> -1 -> 0 over 4800 samples, against a
    // band of [-0.5, 0.5]. It goes high crossing +0.5 on the way up (sample
    // 600) and only comes back down crossing -0.5 (sample 3000). A plain
    // `> 0.5` comparator would have dropped at 1800 — that gap *is* the
    // hysteresis, and it is why a noisy input does not chatter here.
    let sig = render(
        r#"{"kind": "LFTri", "inputs": [{"const": 10.0}, {"const": 0.0}]},
           {"kind": "Schmidt", "inputs": [{"ugen": 0}, {"const": -0.5}, {"const": 0.5}]}"#,
        4800,
    );
    let high: Vec<usize> = (0..4800).filter(|&i| sig[i] > 0.5).collect();
    // The comparisons are strict, so the sample sitting exactly *on* a
    // threshold does not move it: the triangle reads +0.5 at sample 600 and
    // -0.5 at 3000, and the state changes on the sample after each.
    assert_eq!(*high.first().unwrap(), 601, "rises just past +0.5");
    assert_eq!(*high.last().unwrap(), 3000, "falls only past -0.5");
    assert_eq!(high.len(), 3000 - 601 + 1, "and stays high in between");
}

#[test]
fn changed_reports_the_halved_difference() {
    // sclang builds Changed out of HPZ1, whose gain is 0.5, so a step of 1.0
    // registers as 0.5. Reproduced deliberately: a def ported from there must
    // not change value. A threshold of 0.4 sees the square's edges; 0.6 sees
    // nothing at all, which would be surprising if you expected the full step.
    for (threshold, expect) in [(0.4, true), (0.6, false)] {
        let sig = render(
            &format!(
                r#"{{"kind": "LFPulse", "inputs": [{{"const": 10.0}}, {{"const": 0.0}},
                      {{"const": 0.5}}]}},
                   {{"kind": "Changed", "inputs": [{{"ugen": 0}}, {{"const": {threshold}}}]}}"#
            ),
            4800,
        );
        assert_eq!(
            !edges(&sig).is_empty(),
            expect,
            "threshold {threshold} against a step of 1.0 (halved to 0.5)"
        );
    }
}

// ---- the flip-flops and counters ----

#[test]
fn toggle_ff_halves_the_trigger_rate() {
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "ToggleFF", "rate": "ar", "inputs": [{"ugen": 0}]}"#,
        4800,
    );
    // 10 input triggers in 4800 samples; the output rises on every other one.
    assert_eq!(edges(&sig).len(), 5);
}

#[test]
fn set_reset_ff_latches_on_the_first_trigger_and_reset_wins_a_tie() {
    // Set at 100 Hz, reset never: it latches at the first trigger and stays.
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "SetResetFF", "rate": "ar",
            "inputs": [{"ugen": 0}, {"const": 0.0}]}"#,
        4800,
    );
    assert_eq!(edges(&sig).len(), 1, "one rise, then held");
    assert!(sig[100..].iter().all(|s| *s == 1.0), "held at 1");

    // The same signal into both inlets: every set is also a reset, and reset is
    // applied second, so the output never leaves 0.
    let tie = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "SetResetFF", "rate": "ar",
            "inputs": [{"ugen": 0}, {"ugen": 0}]}"#,
        4800,
    );
    assert!(tie.iter().all(|s| *s == 0.0), "a simultaneous reset wins");
}

#[test]
fn pulse_count_counts_triggers_and_a_reset_zeroes_it() {
    // One count per trigger, from the first.
    let t = trigger_samples(100.0, 4800);
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "PulseCount", "rate": "ar",
            "inputs": [{"ugen": 0}, {"const": 0.0}]}"#,
        4800,
    );
    for (k, &sample) in t.iter().enumerate() {
        assert_eq!(
            sig[sample],
            k as f32 + 1.0,
            "trigger {k} at sample {sample}"
        );
    }

    // With a reset at 10 Hz it drops back to zero on each one, having climbed
    // in between.
    let reset = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "Impulse", "inputs": [{"const": 10.0}]},
           {"kind": "PulseCount", "rate": "ar",
            "inputs": [{"ugen": 0}, {"ugen": 1}]}"#,
        4800 * 2,
    );
    let r = trigger_samples(10.0, 4800 * 2);
    assert_eq!(reset[r[1]], 0.0, "the reset zeroes it");
    assert!(
        reset[r[1] - 1] > 5.0,
        "after a second of counting: {}",
        reset[r[1] - 1]
    );
}

#[test]
fn pulse_divider_fires_every_n_triggers_and_start_phases_it() {
    // div = 4 from a stopped counter fires on the 4th, 8th, ... trigger.
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "PulseDivider", "rate": "ar",
            "inputs": [{"ugen": 0}, {"const": 4.0}, {"const": 0.0}]}"#,
        4800,
    );
    let t = trigger_samples(100.0, 4800);
    let want: Vec<usize> = t.iter().skip(3).step_by(4).copied().collect();
    assert_eq!(edges(&sig), want, "the 4th trigger, and every 4th after it");

    // start = div - 1 puts the counter one short, so the *first* trigger fires
    // — which is how two dividers are phased against each other.
    let phased = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "PulseDivider", "rate": "ar",
            "inputs": [{"ugen": 0}, {"const": 4.0}, {"const": 3.0}]}"#,
        4800,
    );
    assert_eq!(edges(&phased)[0], 0, "fires on the first trigger");
}

#[test]
fn stepper_walks_its_range_and_wraps_through_both_ends() {
    // min 0, max 3, step 1, from resetval 0: the first trigger lands on 1 (a
    // stepper is defined by its transitions), then 2, 3, 0, 1, ...
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "Stepper", "rate": "ar", "inputs": [{"ugen": 0}, {"const": 0.0},
            {"const": 0.0}, {"const": 3.0}, {"const": 1.0}, {"const": 0.0}]}"#,
        4800,
    );
    let t = trigger_samples(100.0, 4800);
    let seen: Vec<f32> = t.iter().take(10).map(|&i| sig[i]).collect();
    assert_eq!(seen, vec![1.0, 2.0, 3.0, 0.0, 1.0, 2.0, 3.0, 0.0, 1.0, 2.0]);
    assert_eq!(
        sig[0], 1.0,
        "it sits at resetval until the first trigger steps it"
    );

    // A negative step walks the same ring the other way.
    let down = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "Stepper", "rate": "ar", "inputs": [{"ugen": 0}, {"const": 0.0},
            {"const": 0.0}, {"const": 3.0}, {"const": -1.0}, {"const": 0.0}]}"#,
        4800,
    );
    let seen: Vec<f32> = t.iter().take(5).map(|&i| down[i]).collect();
    assert_eq!(
        seen,
        vec![3.0, 2.0, 1.0, 0.0, 3.0],
        "max is a position, not a limit"
    );
}

// ---- elapsed time: Timer, Sweep ----

#[test]
fn timer_reports_the_interval_between_triggers() {
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "Timer", "rate": "ar", "inputs": [{"ugen": 0}]}"#,
        4800 * 2,
    );
    assert_eq!(sig[0], 0.0, "nothing to measure before the second trigger");
    let measured = sig[9000];
    assert!(
        (measured - 0.01).abs() < 1e-7,
        "100 Hz is a 10 ms interval, got {measured}"
    );
}

#[test]
fn timer_measures_a_crossing_that_falls_between_two_samples() {
    // 997 Hz at 48 kHz is 48.144... samples per period — deliberately not a
    // whole number. Rounding the crossing to the nearest sample would report
    // 48/48000 or 49/48000, i.e. an error of up to 1.04e-5 s. Interpolating
    // where the input actually crossed zero gets it far closer, and that is
    // what this asserts: not "about right", but *better than rounding could
    // be*. The residual is the sine's curvature between the two samples.
    let sig = render(
        r#"{"kind": "Sine", "inputs": [{"const": 997.0}]},
           {"kind": "Timer", "rate": "ar", "inputs": [{"ugen": 0}]}"#,
        48_000,
    );
    let truth = 1.0 / 997.0;
    let measured = sig[40_000];
    let err = (measured - truth).abs();
    let rounding_floor = 0.5 / SR as f64;
    assert!(
        (err as f64) < rounding_floor / 10.0,
        "measured {measured} against {truth} — error {err:e}, which must beat \
         sample rounding ({rounding_floor:e}) by an order of magnitude"
    );
}

#[test]
fn sweep_rises_at_its_rate_and_restarts_at_each_trigger() {
    // rate 2 means two units per second whatever the sample rate: after 4800
    // samples (0.1 s) it reads 0.2.
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 1.0}]},
           {"kind": "Sweep", "inputs": [{"ugen": 0}, {"const": 2.0}]}"#,
        48_000,
    );
    assert!(
        (sig[4800] - 0.2).abs() < 1e-5,
        "0.1 s at 2/s: {}",
        sig[4800]
    );
    assert!(
        (sig[24_000] - 1.0).abs() < 1e-5,
        "0.5 s at 2/s: {}",
        sig[24_000]
    );
    // The next trigger lands at 1 s and puts it back to zero.
    assert!(sig[47_999] > 1.9, "still climbing at the end of the second");
    let restarted = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 2.0}]},
           {"kind": "Sweep", "inputs": [{"ugen": 0}, {"const": 2.0}]}"#,
        48_000,
    );
    assert!(
        restarted[24_000] < 1e-6,
        "the trigger at 0.5 s restarts it: {}",
        restarted[24_000]
    );
}

// ---- the envelope followers: Decay, Decay2 ----

/// The pole `Decay` uses: 60 dB down over `time` seconds.
fn decay_pole(time: f64) -> f64 {
    (-6.907_755_278_982_137 / (time * SR as f64)).exp()
}

#[test]
fn decay_is_the_analytic_one_pole_and_falls_60_db_on_time() {
    let time = 0.05_f64;
    let sig = render(
        &format!(
            r#"{}, {{"kind": "Decay", "inputs": [{{"ugen": 1}}, {{"const": {time}}}]}}"#,
            one_shot(1.0)
        ),
        4800,
    );
    // The impulse response of `y[n] = x[n] + b·y[n-1]` is exactly b^n.
    let b = decay_pole(time);
    for n in [0usize, 1, 100, 1000, 2399] {
        let want = b.powi(n as i32) as f32;
        assert!(
            (sig[n] - want).abs() < 1e-6,
            "sample {n}: {} != {want}",
            sig[n]
        );
    }
    // And the name's promise: -60 dB exactly at the decay time.
    let at_time = sig[(time * SR as f64) as usize];
    assert!(
        (at_time - 0.001).abs() < 1e-5,
        "-60 dB at t = decaytime, got {at_time}"
    );
}

#[test]
fn decay2_peaks_where_its_two_exponentials_cross() {
    // Decay2 is the decay minus a faster attack, so its peak sits where the two
    // slopes match — a closed form, not a shape to eyeball:
    //   n* = ln(ln b_a / ln b_d) / ln(b_d / b_a)
    let (attack, decay) = (0.01_f64, 0.2_f64);
    let sig = render(
        &format!(
            r#"{}, {{"kind": "Decay2", "inputs": [{{"ugen": 1}},
                     {{"const": {attack}}}, {{"const": {decay}}}]}}"#,
            one_shot(1.0)
        ),
        48_000,
    );
    let (ba, bd) = (decay_pole(attack), decay_pole(decay));
    let want = (ba.ln() / bd.ln()).ln() / (bd / ba).ln();
    let peak = sig
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap()
        .0;
    assert!(
        (peak as f64 - want).abs() < 2.0,
        "peak at sample {peak}, analytic {want:.1}"
    );
    assert!(sig[0].abs() < 1e-6, "and it starts from zero, unlike Decay");
}

// ---- DetectSilence ----

#[test]
fn detect_silence_fires_after_exactly_its_time_and_raises_the_done_flag() {
    // A 20 ms burst then nothing, watched with a 10 ms window: silence is
    // declared 10 ms after the burst ends, and `Done` sees the same flag.
    let ugens = format!(
        r#"{{"kind": "Trig1", "inputs": [{{"const": 1.0}}, {{"const": {burst}}}]}},
           {{"kind": "DetectSilence", "inputs": [{{"ugen": 0}}, {{"const": 0.1}},
             {{"const": {window}}}, {{"const": 0.0}}]}}"#,
        burst = secs(960),
        window = secs(480),
    );
    let sig = render(&ugens, 4800);
    // The burst covers samples 0..=959, so silence starts at 960 and its 480th
    // sample — the one that completes the window — is 1439.
    assert_eq!(
        edges(&sig),
        vec![960 + 480 - 1],
        "silence declared on the sample that completes the window"
    );

    // The flag, read the way a graph reads it. It agrees to the **block**, not
    // to the sample, and that is inherent rather than sloppy: a done flag is
    // one bool per UGen, read once when the watcher runs, so a watcher sees it
    // for the whole of the block in which it was raised — here from 1408, the
    // start of the block containing 1439. At `Done`'s own default rate (`kr`)
    // that is exactly its resolution anyway.
    let watched =
        format!(r#"{ugens}, {{"kind": "Done", "rate": "ar", "inputs": [{{"ugen": 1}}]}}"#);
    let flag = render(&watched, 4800);
    let raised = 960 + 480 - 1;
    let block_start = raised / BLOCK_SIZE * BLOCK_SIZE;
    assert_eq!(
        edges(&flag),
        vec![block_start],
        "Done agrees with the output to the block it was raised in"
    );
}

#[test]
fn detect_silence_frees_the_node_through_its_done_action() {
    // The whole point of the row: a voice that has nothing left to say leaves.
    let json = format!(
        r#"{{"name": "sil", "ugens": [
            {{"kind": "Trig1", "inputs": [{{"const": 1.0}}, {{"const": {burst}}}]}},
            {{"kind": "DetectSilence", "inputs": [{{"ugen": 0}}, {{"const": 0.1}},
              {{"const": {window}}}, {{"const": 2.0}}]}},
            {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}
        ]}}"#,
        burst = secs(128),
        window = secs(128),
    );
    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let (mut engine, mut handle) = engine_pair(SR, 2);
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(Arc::new(def), SR, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..2 {
        engine.process_block(&mut out);
    }
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        1,
        "alive while it still sounds"
    );
    for _ in 0..4 {
        engine.process_block(&mut out);
    }
    assert_eq!(
        handle.counters().synths.load(Ordering::Relaxed),
        0,
        "freed once it fell silent"
    );
}

// ---- the rate contract ----

#[test]
fn a_trigger_means_the_same_at_either_rate() {
    // The same 100 Hz clock timed at ar and at kr reports the same interval:
    // choosing kr costs resolution, never meaning (see the calculation-rate
    // note in docs/decisions.md).
    let ar = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
           {"kind": "Timer", "rate": "ar", "inputs": [{"ugen": 0}]}"#,
        4800 * 2,
    );
    let kr = render(
        r#"{"kind": "Impulse", "rate": "kr", "inputs": [{"const": 100.0}]},
           {"kind": "Timer", "rate": "kr", "inputs": [{"ugen": 0}]}"#,
        4800 * 2,
    );
    assert!((ar[9000] - 0.01).abs() < 1e-7, "ar: {}", ar[9000]);
    // At kr a period is quantized to the 64-sample control block, so the
    // tolerance is one control period rather than one sample.
    let block = BLOCK_SIZE as f32 / SR;
    assert!(
        (kr[9000] - 0.01).abs() <= block,
        "kr: {} (one control period is {block})",
        kr[9000]
    );
}

#[test]
fn a_ten_second_sweep_still_reads_the_time_it_has_run() {
    // Rule 4 for this family, and it lands on the one piece of accumulated
    // state here: `Elapsed`'s `f64` accumulator, shared by `Sweep` and
    // `Timer`. Everything else is a flag or an `i64` count, which does not
    // lose anything -- `PulseCount`'s state is exact; only the value it
    // *reports* quantizes, at 2^24, because the wire is `f32`.
    //
    // The separation is large and worth writing down. Stepping the same
    // accumulator by 1/48000 for ten seconds: `f64` lands on 10 s to 1e-11,
    // `f32` on 10.0357 -- **36 ms fast**, which is a third of a percent and
    // plainly audible in anything the sweep drives. The tolerance below sits
    // between the two by orders of magnitude in both directions.
    let n = 48_000 * 10;
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 0.0}]},
           {"kind": "Sweep", "inputs": [{"ugen": 0}, {"const": 1.0}]}"#,
        n,
    );
    let last = n - 1;
    let want = last as f32 / SR;
    assert!(
        (sig[last] - want).abs() < 1e-4,
        "after {want:.4} s the sweep reads {} ({:+.4} s)",
        sig[last],
        sig[last] - want
    );

    // `Timer` reports the same accumulator as an interval, so a long gap
    // between two triggers is the same measurement seen from the other end.
    let sig = render(
        r#"{"kind": "Impulse", "inputs": [{"const": 0.1}]},
           {"kind": "Timer", "rate": "ar", "inputs": [{"ugen": 0}]}"#,
        48_000 * 11,
    );
    let reported = sig[48_000 * 10 + 100];
    assert!(
        (reported - 10.0).abs() < 1e-4,
        "a ten second interval reported as {reported}"
    );
}

#[test]
fn a_gate_that_closes_and_reopens_follows_again() {
    // The gate is tested beside the latch over one cycle, which shows it
    // tracking and then freezing. What that cannot show is whether it *stays*
    // frozen: a gate whose closed state also latched its input would look
    // identical for one cycle and diverge on the second.
    let ramp = r#"{"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 1.0},
                   {"const": 0.0}, {"const": 100000.0}, {"const": 0.0}]}"#;
    let gate = r#"{"kind": "LFPulse", "inputs": [{"const": 10.0}, {"const": 0.0},
                   {"const": 0.5}]}"#;
    let n = 4800 * 3;
    let gated = render(
        &format!(
            r#"{ramp}, {gate},
               {{"kind": "Gate", "inputs": [{{"ugen": 0}}, {{"ugen": 1}}]}}"#
        ),
        n,
    );
    let square = render(gate, n);

    // Over three cycles of the square, the gate's output must equal the ramp
    // while the square is up and hold its last value while it is down --
    // checked against the ramp itself rather than against a shape, so a gate
    // that drifted or re-latched would fail on any sample.
    let mut held = 0.0f32;
    for i in 0..n {
        if square[i] > 0.0 {
            held = i as f32; // the Phasor's value at this sample
        }
        assert!(
            (gated[i] - held).abs() < 1e-6,
            "sample {i} (square {}): gate reads {} not {held}",
            square[i],
            gated[i]
        );
    }
    // And it really did reopen more than once, or the loop above proves little.
    let reopenings = (1..n)
        .filter(|&i| square[i] > 0.0 && square[i - 1] <= 0.0)
        .count();
    assert!(
        reopenings >= 2,
        "only {reopenings} reopenings in the window"
    );
}

#[test]
fn a_counter_is_unmoved_by_a_block_split() {
    // The scheduling machinery cuts a block at an event's sample and runs the
    // tree once per slice; a trigger counter must see each edge exactly once
    // however the block was cut.
    let ugens = r#"{"kind": "Impulse", "inputs": [{"const": 100.0}]},
                   {"kind": "PulseCount", "rate": "ar",
                    "inputs": [{"ugen": 0}, {"const": 0.0}]}"#;
    let whole = render(ugens, 4800);

    let json = format!(
        r#"{{"name": "t", "ugens": [{ugens},
            {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 1}}]}}]}}"#
    );
    let def = compile(serde_json::from_str::<SynthDefSpec>(&json).unwrap()).unwrap();
    let mut synth = UGenSynth::new(Arc::new(def), SR, SEED_STRIDE);
    let mut buses = Buses::new(ControlBuses::new(16), 8);
    let mut split = Vec::with_capacity(4800);
    while split.len() < 4800 {
        buses.clear_audio();
        let mut offset = 0;
        while offset < BLOCK_SIZE {
            let frames = 7.min(BLOCK_SIZE - offset);
            let mut ctx = ProcessCtx {
                sample_rate: SR,
                full_sample_rate: SR,
                buses: &buses,
                buffers: &[],
                offset,
                frames,
            };
            synth.process(&mut ctx);
            offset += frames;
        }
        split.extend_from_slice(buses.audio(0));
    }
    split.truncate(4800);
    assert_eq!(whole, split, "the count must not depend on the slicing");
}
