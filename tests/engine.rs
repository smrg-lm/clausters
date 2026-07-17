//! Engine-level tests, offline and without an audio device: commands go in
//! through the same FIFO the network thread uses, audio comes out of
//! `process_block`, and signal asserts do the listening.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::node::{AddAction, Group, Place, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDef, SynthDefSpec, compile, default_spec};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
const CTL_FREQ: u32 = 0;

fn make_engine() -> (Engine, EngineHandle) {
    engine_pair(SR, CHANNELS)
}

fn default_def() -> Arc<SynthDef> {
    Arc::new(compile(default_spec()).unwrap())
}

/// A synth that overwrites buses 0 and 1 with a constant — execution order
/// becomes audible: whatever runs after it on the bus wins.
fn silencer_def() -> Arc<SynthDef> {
    let json = r#"{
        "name": "silencer",
        "ugens": [
            {"kind": "ReplaceOut", "inputs": [{"const": 0.0}, {"const": 0.0}]},
            {"kind": "ReplaceOut", "inputs": [{"const": 1.0}, {"const": 0.0}]}
        ]
    }"#;
    let spec: SynthDefSpec = serde_json::from_str(json).unwrap();
    Arc::new(compile(spec).unwrap())
}

fn add_synth(id: i32, freq: f32, amp: f32) -> Cmd {
    add_synth_in(id, freq, amp, ROOT_NODE_ID, AddAction::Tail)
}

fn add_synth_in(id: i32, freq: f32, amp: f32, target: i32, action: AddAction) -> Cmd {
    let mut synth = Box::new(UGenSynth::new(default_def()));
    synth.set_control(0, freq);
    synth.set_control(1, amp);
    Cmd::AddSynth {
        id,
        target,
        action,
        synth,
        usage: Default::default(),
    }
}

fn add_silencer(id: i32, target: i32, action: AddAction) -> Cmd {
    Cmd::AddSynth {
        id,
        target,
        action,
        synth: Box::new(UGenSynth::new(silencer_def())),
        usage: Default::default(),
    }
}

fn add_group(id: i32, target: i32, action: AddAction) -> Cmd {
    Cmd::AddGroup {
        id,
        target,
        action,
        group: Group::new(),
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
    let crossings = buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
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
fn two_synths_mix_on_the_bus() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.1)).ok().unwrap();
    handle.send(add_synth(1001, 440.0, 0.1)).ok().unwrap();

    // same freq and phase, both Out-summing into bus 0: RMS doubles
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
    assert!(
        (freq - 440.0).abs() < 5.0,
        "the original synth must survive"
    );
}

#[test]
fn replaceout_after_the_source_silences_the_bus() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.2)).ok().unwrap();
    // tail of root: runs after the sine, overwrites buses 0 and 1
    handle
        .send(add_silencer(1001, ROOT_NODE_ID, AddAction::Tail))
        .ok()
        .unwrap();

    let left = render_left(&mut engine, 100);
    assert!(rms(&left) < 1e-9, "silencer at the tail must win the bus");
}

#[test]
fn node_order_is_audible_and_movable() {
    let (mut engine, mut handle) = make_engine();
    // head of root: the silencer runs *before* the sine, so the sine survives
    handle.send(add_synth(1000, 440.0, 0.2)).ok().unwrap();
    handle
        .send(add_silencer(1001, ROOT_NODE_ID, AddAction::Head))
        .ok()
        .unwrap();
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) > 0.1, "silencer at the head must lose the bus");

    // /n_after: move the silencer after the sine — silence
    handle
        .send(Cmd::MoveNode {
            id: 1001,
            target: 1000,
            place: Place::After,
        })
        .ok()
        .unwrap();
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) < 1e-9, "moved after the sine, must win the bus");

    // /n_before: move it back — sine again
    handle
        .send(Cmd::MoveNode {
            id: 1001,
            target: 1000,
            place: Place::Before,
        })
        .ok()
        .unwrap();
    render_left(&mut engine, 2); // let the move apply and the bus settle
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) > 0.1, "moved before the sine, must lose the bus");
}

#[test]
fn add_before_and_after_place_relative_to_sibling() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.2)).ok().unwrap();
    // addAction After relative to the sine: silencer runs later, wins
    handle
        .send(add_silencer(1001, 1000, AddAction::After))
        .ok()
        .unwrap();
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) < 1e-9);

    handle.send(Cmd::FreeNode { id: 1001 }).ok().unwrap();
    // addAction Before relative to the sine: silencer runs first, loses
    handle
        .send(add_silencer(1002, 1000, AddAction::Before))
        .ok()
        .unwrap();
    render_left(&mut engine, 2);
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) > 0.1);
}

#[test]
fn replace_action_swaps_the_node() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 440.0, 0.2)).ok().unwrap();
    render_left(&mut engine, 10);

    // Replace the sine with another at a different pitch.
    handle
        .send(add_synth_in(1001, 880.0, 0.2, 1000, AddAction::Replace))
        .ok()
        .unwrap();
    render_left(&mut engine, 10);
    assert_eq!(handle.collect_garbage(), 1, "the replaced synth came back");
    let freq = estimated_freq(&render_left(&mut engine, 750));
    assert!((freq - 880.0).abs() < 8.0, "estimated freq = {freq}");
}

#[test]
fn freeing_a_group_frees_its_subtree() {
    let (mut engine, mut handle) = make_engine();
    handle
        .send(add_group(1, ROOT_NODE_ID, AddAction::Tail))
        .ok()
        .unwrap();
    handle
        .send(add_synth_in(1000, 440.0, 0.2, 1, AddAction::Tail))
        .ok()
        .unwrap();
    handle
        .send(add_synth_in(1001, 660.0, 0.2, 1, AddAction::Tail))
        .ok()
        .unwrap();

    let left = render_left(&mut engine, 100);
    assert!(rms(&left) > 0.1, "synths inside a group must be processed");

    handle.send(Cmd::FreeNode { id: 1 }).ok().unwrap();
    let left = render_left(&mut engine, 100);
    assert!(
        rms(&left) < 1e-9,
        "freeing the group must silence its synths"
    );
    // the group and its two synths all come back as garbage
    assert_eq!(handle.collect_garbage(), 3);
}

#[test]
fn free_all_empties_a_group_but_keeps_it() {
    let (mut engine, mut handle) = make_engine();
    handle
        .send(add_group(1, ROOT_NODE_ID, AddAction::Tail))
        .ok()
        .unwrap();
    handle
        .send(add_synth_in(1000, 440.0, 0.2, 1, AddAction::Tail))
        .ok()
        .unwrap();
    render_left(&mut engine, 10);

    handle.send(Cmd::FreeAllInGroup { id: 1 }).ok().unwrap();
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) < 1e-9);
    assert_eq!(handle.collect_garbage(), 1, "only the synth was freed");

    // the group is still there and usable
    handle
        .send(add_synth_in(1001, 440.0, 0.2, 1, AddAction::Tail))
        .ok()
        .unwrap();
    render_left(&mut engine, 2);
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) > 0.1, "the emptied group must accept new synths");
}

#[test]
fn deep_free_keeps_nested_groups() {
    let (mut engine, mut handle) = make_engine();
    handle
        .send(add_group(1, ROOT_NODE_ID, AddAction::Tail))
        .ok()
        .unwrap();
    handle.send(add_group(2, 1, AddAction::Tail)).ok().unwrap();
    handle
        .send(add_synth_in(1000, 440.0, 0.2, 1, AddAction::Head))
        .ok()
        .unwrap();
    handle
        .send(add_synth_in(1001, 660.0, 0.2, 2, AddAction::Tail))
        .ok()
        .unwrap();
    render_left(&mut engine, 10);

    handle.send(Cmd::DeepFreeGroup { id: 1 }).ok().unwrap();
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) < 1e-9, "deep free must silence both synths");
    assert_eq!(handle.collect_garbage(), 2, "only the synths were freed");

    // the nested group survived and still works
    handle
        .send(add_synth_in(1002, 440.0, 0.2, 2, AddAction::Tail))
        .ok()
        .unwrap();
    render_left(&mut engine, 2);
    let left = render_left(&mut engine, 100);
    assert!(rms(&left) > 0.1, "nested group must survive a deep free");
}

#[test]
fn node_events_report_go_and_end() {
    use clausters::server::engine::NodeEventKind;

    let (mut engine, mut handle) = make_engine();
    handle
        .send(add_group(1, ROOT_NODE_ID, AddAction::Tail))
        .ok()
        .unwrap();
    handle
        .send(add_synth_in(1000, 440.0, 0.2, 1, AddAction::Tail))
        .ok()
        .unwrap();
    render_left(&mut engine, 1);

    let ev = handle.pop_event().expect("group go event");
    assert_eq!(ev.kind, NodeEventKind::Go);
    assert_eq!((ev.id, ev.parent_id, ev.is_group), (1, ROOT_NODE_ID, true));
    let ev = handle.pop_event().expect("synth go event");
    assert_eq!(ev.kind, NodeEventKind::Go);
    assert_eq!((ev.id, ev.parent_id, ev.is_group), (1000, 1, false));

    handle.send(Cmd::FreeNode { id: 1 }).ok().unwrap();
    render_left(&mut engine, 1);
    let ev = handle.pop_event().expect("group end event");
    assert_eq!(ev.kind, NodeEventKind::End);
    assert_eq!((ev.id, ev.parent_id, ev.is_group), (1, ROOT_NODE_ID, true));
    let ev = handle.pop_event().expect("synth end event");
    assert_eq!(ev.kind, NodeEventKind::End);
    assert_eq!((ev.id, ev.parent_id, ev.is_group), (1000, 1, false));
    assert!(handle.pop_event().is_none());
}

#[test]
fn control_buses_feed_the_audio_thread() {
    // InCtl reads control bus 7, drives an oscillator's frequency.
    let json = r#"{
        "name": "ctl",
        "ugens": [
            {"kind": "InCtl",  "inputs": [{"const": 7.0}]},
            {"kind": "Sine", "inputs": [{"ugen": 0}]},
            {"kind": "Mul",    "inputs": [{"ugen": 1}, {"const": 0.2}]},
            {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 2}]}
        ]
    }"#;
    let spec: SynthDefSpec = serde_json::from_str(json).unwrap();
    let def = Arc::new(compile(spec).unwrap());

    let (mut engine, mut handle) = make_engine();
    handle.control_buses().set(7, 440.0);
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(def)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    let freq = estimated_freq(&render_left(&mut engine, 750));
    assert!((freq - 440.0).abs() < 5.0, "estimated freq = {freq}");

    handle.control_buses().set(7, 660.0); // /c_set, no engine round-trip
    let freq = estimated_freq(&render_left(&mut engine, 750));
    assert!((freq - 660.0).abs() < 7.0, "estimated freq = {freq}");
}

#[test]
fn cpu_meter_publishes_load_and_peak_resets_per_read() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_synth(1000, 220.0, 0.1)).ok().unwrap();
    render_left(&mut engine, 200);

    let counters = handle.counters();
    // Offline the fraction is just render speed, but it must be a positive,
    // finite load — the meter ran and published.
    let avg = counters.avg_cpu();
    assert!(avg > 0.0 && avg.is_finite(), "avg_cpu = {avg}");
    let peak = counters.take_peak_cpu();
    assert!(peak > 0.0 && peak.is_finite(), "peak_cpu = {peak}");
    assert!(
        peak >= avg * 0.5,
        "peak {peak} should not sit far below avg {avg}"
    );

    // Reading the peak resets its window: with no block processed in
    // between, a second read reports zero.
    assert_eq!(counters.take_peak_cpu(), 0.0);

    // The next block starts a fresh window.
    render_left(&mut engine, 1);
    assert!(counters.take_peak_cpu() > 0.0);
}
