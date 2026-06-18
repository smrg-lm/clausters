//! M11: `/n_map` and `/n_mapa` — buses as a live source of node parameters.
//! Engine-level, no audio device: commands go in through the FIFO, audio comes
//! out of `process_block`, and a zero-crossing estimate does the listening.

use std::sync::Arc;

use clausters::node::{AddAction, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDef, SynthDefSpec, compile, default_spec};

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
const CTL_FREQ: u32 = 0;
/// A non-output audio bus to carry test signals (0/1 are the hardware out).
const SCRATCH_BUS: usize = 5;

fn make_engine() -> (Engine, EngineHandle) {
    engine_pair(SR, CHANNELS)
}

fn default_def() -> Arc<SynthDef> {
    Arc::new(compile(default_spec()).unwrap())
}

/// Writes its `level` control (default 660) into an audio bus every block —
/// a DC source to drive `/n_mapa` from.
fn dc_def() -> Arc<SynthDef> {
    let json = format!(
        r#"{{
            "name": "dc",
            "controls": [{{"name": "level", "default": 660.0}}],
            "ugens": [
                {{"kind": "Out", "inputs": [{{"const": {SCRATCH_BUS}.0}}, {{"control": 0}}]}}
            ]
        }}"#
    );
    let spec: SynthDefSpec = serde_json::from_str(&json).unwrap();
    Arc::new(compile(spec).unwrap())
}

fn add_default(id: i32, freq: f32, amp: f32) -> Cmd {
    let mut synth = Box::new(UGenSynth::new(default_def()));
    synth.set_control(CTL_FREQ, freq);
    synth.set_control(1, amp);
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth,
        usage: Default::default(),
    }
}

fn add_dc(id: i32) -> Cmd {
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth: Box::new(UGenSynth::new(dc_def())),
        usage: Default::default(),
    }
}

fn map(id: i32, index: u32, bus: i32, audio: bool) -> Cmd {
    Cmd::MapControl {
        id,
        index,
        bus,
        audio,
    }
}

fn render_left(engine: &mut Engine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut left = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        left.extend(out.iter().step_by(CHANNELS).copied());
    }
    left
}

fn estimated_freq(buf: &[f32]) -> f32 {
    let crossings = buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f32 * SR / buf.len() as f32
}

fn assert_freq(engine: &mut Engine, expected: f32) {
    let left = render_left(engine, 750);
    assert!(left.iter().all(|x| x.is_finite()));
    let freq = estimated_freq(&left);
    assert!(
        (freq - expected).abs() < 7.0,
        "estimated freq = {freq}, expected ≈ {expected}"
    );
}

#[test]
fn n_map_tracks_a_control_bus_live() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_default(1000, 440.0, 0.2)).ok().unwrap();
    handle
        .send(map(1000, CTL_FREQ, SCRATCH_BUS as i32, false))
        .ok()
        .unwrap();

    // The mapped control follows the bus, every block, with no further /n_set.
    handle
        .send(Cmd::SetControlBus {
            index: SCRATCH_BUS,
            value: 660.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 660.0);

    handle
        .send(Cmd::SetControlBus {
            index: SCRATCH_BUS,
            value: 330.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 330.0);
}

#[test]
fn unmap_holds_the_last_value() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_default(1000, 440.0, 0.2)).ok().unwrap();
    handle
        .send(map(1000, CTL_FREQ, SCRATCH_BUS as i32, false))
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetControlBus {
            index: SCRATCH_BUS,
            value: 330.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 330.0);

    // -1 unmaps: the control keeps the last value it read…
    handle.send(map(1000, CTL_FREQ, -1, false)).ok().unwrap();
    handle
        .send(Cmd::SetControlBus {
            index: SCRATCH_BUS,
            value: 880.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 330.0);

    // …and a plain /n_set still works afterwards.
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: CTL_FREQ,
            value: 550.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 550.0);
}

#[test]
fn n_set_overrides_and_breaks_a_mapping() {
    let (mut engine, mut handle) = make_engine();
    handle.send(add_default(1000, 440.0, 0.2)).ok().unwrap();
    handle
        .send(map(1000, CTL_FREQ, SCRATCH_BUS as i32, false))
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetControlBus {
            index: SCRATCH_BUS,
            value: 660.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 660.0);

    // An explicit set fixes the value and clears the mapping (scsynth).
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: CTL_FREQ,
            value: 440.0,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetControlBus {
            index: SCRATCH_BUS,
            value: 880.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 440.0);
}

#[test]
fn n_mapa_samples_an_audio_bus() {
    let (mut engine, mut handle) = make_engine();
    // Source runs first (added first), writing the scratch bus the target reads.
    handle.send(add_dc(900)).ok().unwrap();
    handle.send(add_default(1000, 440.0, 0.2)).ok().unwrap();
    handle
        .send(map(1000, CTL_FREQ, SCRATCH_BUS as i32, true))
        .ok()
        .unwrap();

    // The target's freq follows the (DC) audio bus, sampled per block.
    assert_freq(&mut engine, 660.0);

    // Move the source's level and the mapped control follows.
    handle
        .send(Cmd::SetControl {
            id: 900,
            index: 0,
            value: 330.0,
        })
        .ok()
        .unwrap();
    assert_freq(&mut engine, 330.0);
}
