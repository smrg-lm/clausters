//! Engine-level tests, offline and without an audio device: commands go in
//! through the same FIFO the network thread uses, audio comes out of
//! `process_block`, and signal asserts do the listening.

use std::sync::Arc;

use claudesufa::node::{AddAction, SynthNode};
use claudesufa::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use claudesufa::synthdef::instance::UGenSynth;
use claudesufa::synthdef::{SynthDef, compile, default_spec};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
const CTL_FREQ: u32 = 0;

fn make_engine() -> (Engine, EngineHandle) {
    engine_pair(SR, CHANNELS)
}

fn default_def() -> Arc<SynthDef> {
    Arc::new(compile(default_spec()).unwrap())
}

fn add_synth(id: i32, freq: f32, amp: f32) -> Cmd {
    let mut synth = Box::new(UGenSynth::new(default_def()));
    synth.set_control(0, freq);
    synth.set_control(1, amp);
    Cmd::AddSynth {
        id,
        synth,
        action: AddAction::Tail,
    }
}

/// Renders `blocks` blocks and returns the left channel.
fn render_left(engine: &mut Engine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut left = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        left.extend(out.iter().step_by(CHANNELS).copied());
    }
    left
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

fn estimated_freq(buf: &[f32]) -> f32 {
    let crossings = buf
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    crossings as f32 * SR / buf.len() as f32
}

#[test]
fn empty_engine_is_silent() {
    let (mut engine, _handle) = make_engine();
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) < 1e-9);
}

#[test]
fn added_synth_produces_sine() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.2)).ok().unwrap();

    let left = render_left(&mut engine, 750); // exactly 1 s
    assert!(left.iter().all(|x| x.is_finite()));

    let expected_rms = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (rms(&left) - expected_rms).abs() < 0.002,
        "rms = {}, expected ≈ {expected_rms}",
        rms(&left)
    );
    let freq = estimated_freq(&left);
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");
}

#[test]
fn two_synths_mix() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.1)).ok().unwrap();
    handle.send(add_synth(1001, 440.0, 0.1)).ok().unwrap();

    // same freq and phase: amplitudes add, RMS doubles vs a single 0.1 synth
    let left = render_left(&mut engine, 750);
    let expected_rms = 0.2 * std::f32::consts::FRAC_1_SQRT_2;
    assert!((rms(&left) - expected_rms).abs() < 0.002);
}

#[test]
fn set_control_changes_pitch() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.2)).ok().unwrap();
    render_left(&mut engine, 100); // warmup at 440

    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: CTL_FREQ,
            value: 660.0,
        })
        .ok()
        .unwrap();
    let left = render_left(&mut engine, 750);
    let freq = estimated_freq(&left);
    assert!((freq - 660.0).abs() < 7.0, "estimated freq = {freq}");
}

#[test]
fn freed_synth_goes_silent_and_returns_garbage() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.2)).ok().unwrap();
    render_left(&mut engine, 10);

    handle.send(Cmd::FreeNode { id: 1000 }).ok().unwrap();
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) < 1e-9, "bus must be clean after /n_free");

    assert_eq!(handle.collect_garbage(), 1);
}

#[test]
fn duplicate_id_is_rejected_as_garbage() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.1)).ok().unwrap();
    handle.send(add_synth(1000, 880.0, 0.1)).ok().unwrap(); // duplicate

    render_left(&mut engine, 10);
    assert_eq!(handle.collect_garbage(), 1); // the rejected one came back
    let freq = estimated_freq(&render_left(&mut engine, 750));
    assert!((freq - 440.0).abs() < 5.0, "the original synth must survive");
}
