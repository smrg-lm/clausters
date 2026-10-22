//! **A reset voice sounds as a fresh one does**, over every UGen kind.
//!
//! A `Voices` node keeps its voices built and hands a free one to each new
//! note, reset on its own memory (`SynthNode::reset`, every UGen's
//! `UGen::reset`). A kind that forgot part of its state would carry the last
//! note into the next -- a phase, a filter's memory, an envelope's stage -- and
//! no other test would see it, since a node is otherwise built fresh. So each
//! case renders a stretch, resets, renders the same stretch again and requires
//! the two to be bit-identical, done actions included.
//!
//! The cases are the subject table's stateful rows (`tests/common/subjects.json`)
//! plus the kinds that table does not hold, written here. The last test is the
//! guard: every row of the catalog is either reset-tested or named, with the
//! reason, as having no state to reset, a stream that goes on, or no reset at
//! all -- so a new kind cannot arrive untested.

#![cfg(feature = "synth")]

#[path = "common/bench.rs"]
mod bench;

use std::collections::HashSet;
use std::sync::Arc;

use bench::*;
use clausters::dsp::buffer::Buffer;
use clausters::dsp::conv::layout;
use clausters::dsp::registry::{BuildCtx, UGenConfig, all, lookup};
use clausters::dsp::wavetable::{GenCommand, signal_to_wavetable};
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use serde_json::{Value, json};

/// Samples per half: enough for the slow cases (a 1024-point FFT chain fills
/// its first window, a prepared kernel its first partition).
const HALF: usize = BLOCK_SIZE * 64;

/// The kinds with no state that outlives a block: what they output is a
/// function of this block's inputs and the context.
const STATELESS: &[(&str, &str)] = &[
    ("Add", "arithmetic"),
    ("Sub", "arithmetic"),
    ("Mul", "arithmetic"),
    ("Div", "arithmetic"),
    ("BinaryOpUGen", "arithmetic"),
    ("UnaryOpUGen", "arithmetic"),
    ("RangeMapUGen", "arithmetic"),
    ("MulAdd", "arithmetic"),
    ("Sum3", "arithmetic"),
    ("Sum4", "arithmetic"),
    ("Clip", "arithmetic"),
    ("Wrap", "arithmetic"),
    ("Fold", "arithmetic"),
    ("EnvShape", "a curve read at its phase input"),
    ("SampleRate", "the engine's rate"),
    ("In", "bus I/O"),
    ("Out", "bus I/O"),
    ("ReplaceOut", "bus I/O"),
    ("InCtl", "bus I/O"),
    ("OutCtl", "bus I/O"),
    ("BufRd", "reads the buffer at its phase input"),
    ("BufWr", "writes the buffer at its phase input"),
    ("BufSampleRate", "a buffer's facts"),
    ("BufRateScale", "a buffer's facts"),
    ("BufFrames", "a buffer's facts"),
    ("BufChannels", "a buffer's facts"),
    ("BufDur", "a buffer's facts"),
    ("TransportPos", "the transport's position, from the context"),
    (
        "TransportBufRd",
        "reads the buffer at the transport's position, from the context",
    ),
    ("TransportFade", "the transport's fade, from the context"),
    ("FreeSelf", "its action is recomputed every block"),
    ("PauseSelf", "its action is recomputed every block"),
    ("PV_Kernel", "its stack is scratch, rewritten every frame"),
    ("PV_MagAbove", "per frame, no memory"),
    ("PV_MagBelow", "per frame, no memory"),
    ("PV_MagClip", "per frame, no memory"),
    ("PV_BrickWall", "per frame, no memory"),
    ("Dswitch1", "its position is its inputs'"),
];

/// Random sources: their stream goes on across a reset, so a restarted voice
/// draws new numbers and two halves cannot agree. What they reset besides the
/// stream is covered by their split and seeded tests (`tests/noise.rs`,
/// `tests/demand.rs`).
const STOCHASTIC: &[&str] = &[
    "Rand",
    "RandOpUGen",
    "Dwhite",
    "Diwhite",
    "Dbrown",
    "Dibrown",
    "Drand",
    "Dxrand",
    "Dshuf",
    "WhiteNoise",
    "PinkNoise",
    "BrownNoise",
    "GrayNoise",
    "ClipNoise",
    "LFNoise0",
    "LFNoise1",
    "LFNoise2",
    "LFClipNoise",
    "Dust",
    "Dust2",
];

/// Kinds whose state is not the voice's to reset.
const NOT_RESET: &[(&str, &str)] = &[
    (
        "DiskIn",
        "a stream is its I/O thread's; a Voices node refuses the def",
    ),
    (
        "DiskOut",
        "a stream is its I/O thread's; a Voices node refuses the def",
    ),
    (
        "TransportDiskIn",
        "a stream is its I/O thread's; a Voices node refuses the def",
    ),
    (
        "PeaksOut",
        "a file is its writer's; a Voices node refuses the def",
    ),
    ("BufDelayN", "the line is a pool buffer, the client's"),
    ("BufDelayL", "the line is a pool buffer, the client's"),
    ("BufDelayC", "the line is a pool buffer, the client's"),
    ("BufCombN", "the line is a pool buffer, the client's"),
    ("BufCombL", "the line is a pool buffer, the client's"),
    ("BufCombC", "the line is a pool buffer, the client's"),
    ("BufAllpassN", "the line is a pool buffer, the client's"),
    ("BufAllpassL", "the line is a pool buffer, the client's"),
    ("BufAllpassC", "the line is a pool buffer, the client's"),
];

/// The reply kinds keep no audible state: they are reset-tested by the
/// replies they make, in [`a_reply_kind_fires_again_after_a_reset`].
const REPLIES: &[&str] = &["SendTrig", "SendReply", "Poll"];

/// A case the subject table does not hold: a def, the signal on its `In`
/// (bus [`IN_BUS`]) and the control sets it runs with.
struct Case {
    name: &'static str,
    ugens: Value,
    source: Option<&'static str>,
    sets: Vec<(usize, u32, f32)>,
    controls: Value,
}

impl Case {
    fn new(name: &'static str, ugens: Value) -> Self {
        Self {
            name,
            ugens,
            source: None,
            sets: Vec::new(),
            controls: json!([]),
        }
    }
    fn source(mut self, kind: &'static str) -> Self {
        self.source = Some(kind);
        self
    }
    fn def(&self) -> String {
        json!({"name": self.name, "controls": self.controls, "ugens": self.ugens}).to_string()
    }
    fn kinds(&self) -> Vec<String> {
        self.ugens
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["kind"].as_str().unwrap().to_string())
            .collect()
    }
}

fn c(v: f32) -> Value {
    json!({ "const": v })
}
fn u(i: usize) -> Value {
    json!({ "ugen": i })
}
fn input() -> Value {
    json!({"kind": "In", "inputs": [c(IN_BUS as f32)]})
}
fn out(i: usize) -> Value {
    json!({"kind": "Out", "inputs": [c(0.0), u(i)]})
}

/// The kernel `Conv` reads, as `/buffer_gen prepare_partconv` writes it.
fn prepared(ir: &[f32], fft_size: usize) -> Buffer {
    let parts = ir.len().div_ceil(fft_size / 2);
    let target = Buffer::zeroed(layout::frames(fft_size, parts), 1, SR as f64);
    GenCommand::PreparePartConv {
        src: Arc::new(Buffer::new(ir.to_vec(), 1, ir.len(), SR as f64)),
        fft_size,
    }
    .apply(&target)
}

/// Buffers 0 and 1: wavetables; 2: plain samples; 3: a prepared kernel; 4: a
/// short buffer a recorder fills and finishes.
fn pool() -> Vec<Option<Arc<Buffer>>> {
    let table = |f: &dyn Fn(f32) -> f32| {
        let period: Vec<f32> = (0..512)
            .map(|i| f(std::f32::consts::TAU * i as f32 / 512.0))
            .collect();
        let t = signal_to_wavetable(&period, true);
        Some(Arc::new(Buffer::new(t.clone(), 1, t.len(), SR as f64)))
    };
    let samples: Vec<f32> = (0..4096)
        .map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0)
        .collect();
    let ir: Vec<f32> = (0..1500)
        .map(|i| 0.9f32.powi(i % 40) * if i % 3 == 0 { 1.0 } else { -0.5 })
        .collect();
    vec![
        table(&|x| x.sin()),
        table(&|x| (2.0 * x).sin() * 0.5 + x.sin() * 0.5),
        Some(Arc::new(Buffer::new(samples, 1, 4096, SR as f64))),
        Some(Arc::new(prepared(&ir, 256))),
        Some(Arc::new(Buffer::zeroed(1000, 1, SR as f64))),
    ]
}

/// An envelope over the `gate` control (index 0): attack, decay to a
/// sustain, release -- three short segments, released at node 1.
fn envgen(done_action: f32) -> Value {
    let mut inputs = vec![
        json!({"control": 0}),
        c(1.0),
        c(0.0),
        c(1.0),
        c(done_action),
    ];
    inputs.extend([c(0.0), c(3.0), c(1.0), c(-99.0)]);
    for seg in [
        [1.0, 0.003, 1.0, 0.0],
        [0.4, 0.004, 5.0, -2.0],
        [0.0, 0.005, 2.0, 0.0],
    ] {
        inputs.extend(seg.iter().map(|v| c(*v)));
    }
    json!({"kind": "EnvGen", "inputs": inputs})
}

fn gated(mut case: Case) -> Case {
    case.controls = json!([{"name": "gate", "default": 1.0}]);
    // Each note opens the gate and closes it, as a voice's note-on and
    // note-off do.
    case.sets = vec![(0, 0, 1.0), (BLOCK_SIZE * 20, 0, 0.0)];
    case
}

fn cases() -> Vec<Case> {
    let spectral_tail = |first: usize| {
        // FFT -> each single-chain transform -> IFFT, from UGen `first`.
        json!([
            {"kind": "PV_MagFreeze", "inputs": [u(first), c(0.0)]},
            {"kind": "PV_MagSmear", "inputs": [u(first + 1), c(2.0)]},
            {"kind": "PV_BinShift", "inputs": [u(first + 2), c(1.1), c(2.0)]},
            {"kind": "PV_MagShift", "inputs": [u(first + 3), c(0.9), c(-1.0)]},
            {"kind": "IFFT", "inputs": [u(first + 4)]},
        ])
    };
    let mut spectral = vec![input(), json!({"kind": "FFT", "inputs": [u(0)]})];
    spectral.extend(spectral_tail(1).as_array().unwrap().iter().cloned());
    spectral.push(out(6));
    let pair = |ops: [&str; 3]| {
        let mut ugens = vec![
            input(),
            json!({"kind": "FFT", "inputs": [u(0)]}),
            json!({"kind": "Sine", "inputs": [c(700.0)]}),
            json!({"kind": "FFT", "inputs": [u(2)]}),
        ];
        for (k, op) in ops.iter().enumerate() {
            ugens.push(json!({"kind": op, "inputs": [u(if k == 0 { 1 } else { 3 + k }), u(3)]}));
        }
        ugens.push(json!({"kind": "IFFT", "inputs": [u(6)]}));
        ugens.push(out(7));
        Value::from(ugens)
    };
    vec![
        Case::new(
            "sine",
            json!([{"kind": "Sine", "inputs": [c(330.0)]}, out(0)]),
        ),
        Case::new(
            "impulse",
            json!([{"kind": "Impulse", "inputs": [c(900.0)]}, out(0)]),
        ),
        Case::new(
            "lag",
            json!([input(), {"kind": "Lag", "inputs": [u(0), c(0.01)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "varlag",
            json!([input(), {"kind": "VarLag", "inputs": [u(0), c(0.004), c(0.02)]}, out(1)]),
        )
        .source("noise"),
        gated(Case::new("envgen", json!([envgen(0.0), out(0)]))),
        // Reset while it sustains, the gate still open: a closed envelope
        // retriggers on its own and would pass without a reset.
        {
            let mut held = gated(Case::new("envgenheld", json!([envgen(0.0), out(0)])));
            held.sets.truncate(1);
            held
        },
        gated(Case::new(
            "done",
            json!([envgen(0.0), {"kind": "Done", "inputs": [u(0)]}, out(1)]),
        )),
        gated(Case::new(
            "freeselfwhendone",
            json!([envgen(0.0), {"kind": "FreeSelfWhenDone", "inputs": [u(0)]}, out(1)]),
        )),
        Case::new(
            "localfeedback",
            json!([
                input(),
                {"kind": "LocalIn", "inputs": [c(0.0)]},
                {"kind": "Mul", "inputs": [u(1), c(0.7)]},
                {"kind": "Add", "inputs": [u(0), u(2)]},
                {"kind": "LocalOut", "inputs": [c(0.0), u(3)]},
                out(3),
            ]),
        )
        .source("impulse"),
        Case::new(
            "demand",
            json!([
                {"kind": "Dgeom", "inputs": [c(400.0), c(1.0), c(1.01)]},
                {"kind": "Dstutter", "inputs": [c(2.0), u(0)]},
                {"kind": "Dswitch1", "inputs": [c(0.0), u(1)]},
                {"kind": "Impulse", "inputs": [c(3000.0)]},
                {"kind": "Demand", "inputs": [u(3), c(0.0), u(2)]},
                out(4),
            ]),
        ),
        Case::new(
            "dbufrd",
            json!([
                {"kind": "Dseries", "inputs": [c(1000.0), c(0.0), c(3.0)]},
                {"kind": "Dbufrd", "inputs": [c(2.0), u(0)]},
                {"kind": "Impulse", "inputs": [c(2000.0)]},
                {"kind": "Demand", "inputs": [u(2), c(0.0), u(1)]},
                out(3),
            ]),
        ),
        Case::new(
            "detectsilence",
            json!([input(), {"kind": "DetectSilence", "inputs": [u(0), c(0.0001), c(0.01), c(2.0)]}, out(1)]),
        )
        .source("impulse"),
        Case::new(
            "meter",
            json!([input(), {"kind": "Meter", "inputs": [u(0), c(20.0), c(0.01)]}, out(1)]),
        )
        .source("sine"),
        Case::new(
            "truepeak",
            json!([input(), {"kind": "TruePeak", "inputs": [u(0), c(20.0), c(0.01)]}, out(1)]),
        )
        .source("sine"),
        Case::new(
            "clipcount",
            json!([input(), {"kind": "Mul", "inputs": [u(0), c(2.0)]},
                   {"kind": "ClipCount", "inputs": [u(1), c(1.0), c(1.0)]}, out(2)]),
        )
        .source("sine"),
        Case::new("spectral", Value::from(spectral)).source("noise"),
        Case::new(
            "spectralpair1",
            pair(["PV_Add", "PV_MagMul", "PV_CopyPhase"]),
        )
        .source("noise"),
        Case::new("spectralpair2", pair(["PV_Min", "PV_Max", "PV_Mul"])).source("noise"),
        Case::new(
            "osc",
            json!([{"kind": "Osc", "inputs": [c(0.0), c(330.0), c(0.0)]}, out(0)]),
        ),
        Case::new(
            "oscn",
            json!([{"kind": "OscN", "inputs": [c(2.0), c(30.0), c(0.0)]}, out(0)]),
        ),
        Case::new(
            "vosc",
            json!([{"kind": "VOsc", "inputs": [c(0.4), c(330.0), c(0.0)]}, out(0)]),
        ),
        Case::new(
            "shaper",
            json!([input(), {"kind": "Shaper", "inputs": [c(1.0), u(0)]}, out(1)]),
        )
        .source("sine"),
        Case::new(
            "playbuf",
            json!([{"kind": "PlayBuf", "inputs": [c(2.0), c(0.0), c(1.3), c(1.0), c(0.0), c(0.0), c(0.0)]}, out(0)]),
        ),
        Case::new(
            "recordbuf",
            json!([input(), {"kind": "RecordBuf", "inputs": [c(4.0), c(0.0), u(0), c(0.0), c(1.0),
                   c(0.0), c(1.0), c(0.0), c(0.0), c(2.0)]}, out(1)]),
        )
        .source("sine"),
        Case::new(
            "conv",
            json!([input(), {"kind": "Conv", "inputs": [u(0), c(3.0)], "fft_size": 256}, out(1)]),
        )
        .source("noise"),
    ]
}

#[test]
fn every_stateful_subject_restarts_after_a_reset() {
    for (_, s) in all_subjects() {
        // Its subject listens to noise, which is never silent, so it never
        // acts: a case gives it a silence to detect.
        if s.has("stateful") && !s.has("stochastic") && s.name != "detectsilence" {
            assert_subject_resets(&s, HALF);
        }
    }
}

#[test]
fn every_kind_the_subjects_lack_restarts_after_a_reset() {
    let buffers = pool();
    for case in cases() {
        let input = case.source.map(|k| source_signal(k, HALF, 440.0));
        let mut run = Run::new(HALF);
        run.input = input.as_deref();
        run.buffers = &buffers;
        run.sets = case.sets.clone();
        assert_reset_restarts(case.name, &case.def(), run, HALF);
    }
}

#[test]
fn a_reply_kind_fires_again_after_a_reset() {
    let buses = Buses::new(ControlBuses::new(4), 4);
    for name in REPLIES {
        let d = lookup(name).unwrap();
        let config = UGenConfig {
            label: Some("/r".into()),
            ..UGenConfig::default()
        };
        let mut ugen = (d.build)(&config, &BuildCtx::new(SR, BLOCK_SIZE, 1));
        let high = [1.0f32; BLOCK_SIZE];
        let zero = [0.0f32];
        let inputs: Vec<&[f32]> = vec![&high, &zero, &zero];
        let mut fired = 0;
        for reset in [false, true, false] {
            if reset {
                ugen.reset();
            }
            let mut ctx = ProcessCtx {
                sample_rate: SR,
                full_sample_rate: SR,
                buses: &buses,
                buffers: &[],
                offset: 0,
                frames: BLOCK_SIZE,
                transport: Default::default(),
            };
            let mut output = [0.0f32; BLOCK_SIZE];
            ugen.process(&mut ctx, &inputs, &mut output[..1]);
            ugen.drain_replies(1, &mut |_| fired += 1);
        }
        // A held trigger fires on its rising edge; a reset forgets the edge,
        // so it fires again, and the block after the reset does not.
        assert_eq!(
            fired, 2,
            "{name}: a reset voice must hear its trigger rise again"
        );
    }
}

#[test]
fn every_kind_of_the_catalog_is_reset_tested_or_named() {
    let mut tested: HashSet<String> = HashSet::new();
    let mut memoryless: HashSet<String> = HashSet::new();
    for (_, s) in all_subjects() {
        let kinds = std::iter::once(s.kind.clone()).chain(
            s.prelude
                .iter()
                .map(|p| p["kind"].as_str().unwrap().to_string()),
        );
        if s.has("stateful") && !s.has("stochastic") {
            tested.extend(kinds);
        } else if !s.has("stochastic") {
            memoryless.extend(kinds);
        }
    }
    for case in cases() {
        tested.extend(case.kinds());
    }
    let named: HashSet<&str> = STATELESS
        .iter()
        .map(|(k, _)| *k)
        .chain(NOT_RESET.iter().map(|(k, _)| *k))
        .chain(STOCHASTIC.iter().copied())
        .chain(REPLIES.iter().copied())
        .collect();
    let missing: Vec<&str> = all()
        .map(|d| d.name)
        .filter(|k| !tested.contains(*k) && !memoryless.contains(*k) && !named.contains(k))
        .collect();
    assert!(
        missing.is_empty(),
        "kinds neither reset-tested nor named with a reason: {missing:?}"
    );
}
