//! scsynth's test UGens, `CheckBadValues` and `Sanitize`, driven directly: a
//! denormal only reaches a UGen from outside the arithmetic (the engine runs
//! flush-to-zero), so the inputs are written by hand rather than computed.

#![cfg(feature = "synth")]

use clausters::dsp::check::{CheckBadValues, Sanitize};
use clausters::dsp::{
    BLOCK_SIZE, Buses, ControlBuses, ProcessCtx, ReplyKind, ReplyMsg, UGen, bad_value_line,
};

/// One block of `ugen` over `inputs`, and the replies it drained.
fn run(ugen: &mut dyn UGen, inputs: &[&[f32]], frames: usize) -> (Vec<f32>, Vec<ReplyMsg>) {
    let buses = Buses::new(ControlBuses::new(4), 2);
    let mut ctx = ProcessCtx {
        sample_rate: 48_000.0,
        full_sample_rate: 48_000.0,
        buses: &buses,
        buffers: &[],
        local_buffers: &[],
        offset: 0,
        frames: BLOCK_SIZE,
        transport: Default::default(),
        server: Default::default(),
    };
    let mut out = vec![0.0; frames];
    ugen.process(&mut ctx, inputs, &mut out);
    let mut replies = Vec::new();
    ugen.drain_replies(7, &mut |m| replies.push(m));
    (out, replies)
}

const DENORMAL: f32 = 1.0e-40;

/// Two of each kind of number, in runs: normal, NaN, infinity, denormal,
/// zero (which counts as normal).
fn signal() -> Vec<f32> {
    vec![
        0.5,
        -2.0,
        f32::NAN,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        DENORMAL,
        -DENORMAL,
        0.0,
        3.0,
    ]
}

#[test]
fn check_bad_values_outputs_the_class_of_every_sample() {
    let x = signal();
    let (out, replies) = run(&mut CheckBadValues::new(), &[&x, &[1.0], &[0.0]], x.len());
    assert_eq!(out, vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 0.0, 0.0]);
    assert!(replies.is_empty(), "post 0 posts nothing");
}

#[test]
fn check_bad_values_posts_every_bad_sample_or_every_change() {
    let x = signal();
    let (_, every) = run(&mut CheckBadValues::new(), &[&x, &[4.0], &[1.0]], x.len());
    assert_eq!(every.len(), 6, "one post per bad sample");
    let (_, changes) = run(&mut CheckBadValues::new(), &[&x, &[4.0], &[2.0]], x.len());
    let lines: Vec<String> = changes
        .iter()
        .map(|m| {
            assert_eq!(m.kind, ReplyKind::BadValue);
            bad_value_line(m)
        })
        .collect();
    assert_eq!(
        lines,
        vec![
            "CheckBadValues: NaN found in node 7, id 4 (the 2 values before were normal)",
            "CheckBadValues: infinity found in node 7, id 4 (the 2 values before were NaN)",
            "CheckBadValues: denormal found in node 7, id 4 (the 2 values before were infinity)",
            "CheckBadValues: normal found in node 7, id 4 (the 2 values before were denormal)",
        ]
    );
}

#[test]
fn check_bad_values_counts_a_run_across_blocks() {
    // A NaN at the very start has no run before it; the normal values after
    // it are counted over two calls.
    let mut check = CheckBadValues::new();
    let (_, first) = run(&mut check, &[&[f32::NAN, 1.0, 1.0], &[0.0], &[2.0]], 3);
    assert_eq!(
        bad_value_line(&first[0]),
        "CheckBadValues: NaN found in node 7, id 0"
    );
    let (_, second) = run(&mut check, &[&[1.0, f32::INFINITY], &[0.0], &[2.0]], 2);
    assert_eq!(
        bad_value_line(&second[0]),
        "CheckBadValues: infinity found in node 7, id 0 (the 3 values before were normal)"
    );
}

#[test]
fn sanitize_replaces_every_bad_value_and_keeps_the_rest() {
    let x = signal();
    let (out, _) = run(&mut Sanitize, &[&x, &[0.25]], x.len());
    assert_eq!(
        out,
        vec![0.5, -2.0, 0.25, 0.25, 0.25, 0.25, 0.25, 0.25, 0.0, 3.0]
    );
    // An audio-rate replacement is read at the sample it replaces.
    let r: Vec<f32> = (0..x.len()).map(|i| i as f32).collect();
    let (out, _) = run(&mut Sanitize, &[&x, &r], x.len());
    assert_eq!(&out[2..8], &[2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
}
