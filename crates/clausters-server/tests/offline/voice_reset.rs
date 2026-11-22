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

use crate::bench;

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
    ("LinExp", "arithmetic"),
    ("InRange", "arithmetic"),
    ("InRect", "arithmetic"),
    ("AmpComp", "arithmetic"),
    ("AmpCompA", "arithmetic"),
    ("A2K", "this block's first sample"),
    ("T2K", "this block's largest sample"),
    ("Index", "reads the buffer at its index input"),
    ("IndexL", "reads the buffer at its index input"),
    ("WrapIndex", "reads the buffer at its index input"),
    ("FoldIndex", "reads the buffer at its index input"),
    ("IndexInBetween", "searches the buffer for its input"),
    (
        "DetectIndex",
        "searches the buffer for its input; the last search is a cache",
    ),
    ("DegreeToKey", "reads the buffer at its degree input"),
    ("BlockSize", "the engine's facts"),
    ("ControlRate", "the engine's facts"),
    ("ControlDur", "the engine's facts"),
    ("SampleDur", "the engine's facts"),
    ("RadiansPerSample", "the engine's facts"),
    ("SubsampleOffset", "the engine's facts"),
    ("NumAudioBuses", "the engine's facts"),
    ("NumControlBuses", "the engine's facts"),
    ("NumInputBuses", "the engine's facts"),
    ("NumOutputBuses", "the engine's facts"),
    ("NumBuffers", "the engine's facts"),
    ("NodeID", "the engine's facts"),
    ("NumRunningSynths", "the engine's facts"),
    ("BufSamples", "a buffer's facts"),
    ("InFeedback", "bus I/O"),
    ("OffsetOut", "bus I/O"),
    ("SharedIn", "bus I/O"),
    ("SharedOut", "bus I/O"),
    ("Hasher", "arithmetic"),
    ("MantissaMask", "arithmetic"),
    (
        "IEnvGen",
        "a curve read at its index input; the last read is a cache",
    ),
    (
        "MaxLocalBufs",
        "does nothing: the compiler counts the buffers",
    ),
    (
        "ClearBuf",
        "acts at the start of each note, from its inputs",
    ),
];

/// Random sources: their stream goes on across a reset, so a restarted voice
/// draws new numbers and two halves cannot agree. What they reset besides the
/// stream is covered by their split and seeded tests (`tests/offline/noise.rs`,
/// `tests/offline/demand.rs`).
const STOCHASTIC: &[&str] = &[
    "Rand",
    "IRand",
    "ExpRand",
    "LinRand",
    "NRand",
    "TRand",
    "TIRand",
    "TExpRand",
    "CoinGate",
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
    "Crackle",
    "Vibrato",
    "TWindex",
    "LFDNoise0",
    "LFDNoise1",
    "LFDNoise3",
    "LFDClipNoise",
    "Gendy1",
    "Gendy2",
    "Gendy3",
];

/// Kinds whose state is not the voice's to reset.
const NOT_RESET: &[(&str, &str)] = &[
    (
        "RandID",
        "the generator a synth draws from is the engine's, and so is its choice",
    ),
    (
        "RandSeed",
        "it reseeds the engine's generator, which a voice's reset does not own",
    ),
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
        "VDiskIn",
        "a stream is its I/O thread's; a Voices node refuses the def",
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

/// The kinds whose state shows in what they ask rather than in a signal:
/// [`what_a_kind_asks_restarts_after_a_reset`] resets them.
const ASKING: &[&str] = &["Free", "Pause", "SendPeakRMS"];

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
            "xout",
            json!([
                input(),
                {"kind": "LFTri", "rate": "kr", "inputs": [c(7.0), c(0.0)]},
                {"kind": "MulAdd", "rate": "kr", "inputs": [u(1), c(0.5), c(0.5)]},
                {"kind": "XOut", "inputs": [c(0.0), u(2), u(0)]}
            ]),
        )
        .source("noise"),
        Case::new(
            "lagin",
            json!([
                {"kind": "LFPulse", "rate": "kr", "inputs": [c(5.0), c(0.0), c(0.5)]},
                {"kind": "OutCtl", "inputs": [c(3.0), u(0)]},
                {"kind": "LagIn", "inputs": [c(3.0), c(0.02)]},
                out(2)
            ]),
        ),
        Case::new(
            "intrig",
            json!([
                {"kind": "LFPulse", "rate": "kr", "inputs": [c(40.0), c(0.0), c(0.5)]},
                {"kind": "OutCtl", "inputs": [c(4.0), u(0)]},
                {"kind": "InTrig", "inputs": [c(4.0)]},
                out(2)
            ]),
        ),
        Case::new(
            "peak",
            json!([input(), {"kind": "Impulse", "inputs": [c(97.0), c(0.5)]},
                {"kind": "Peak", "inputs": [u(0), u(1)]}, out(2)]),
        )
        .source("noise"),
        Case::new(
            "runningmax",
            json!([input(), {"kind": "Impulse", "inputs": [c(97.0), c(0.5)]},
                {"kind": "RunningMax", "inputs": [u(0), u(1)]}, out(2)]),
        )
        .source("noise"),
        Case::new(
            "runningmin",
            json!([input(), {"kind": "Impulse", "inputs": [c(97.0), c(0.5)]},
                {"kind": "RunningMin", "inputs": [u(0), u(1)]}, out(2)]),
        )
        .source("noise"),
        Case::new(
            "peakfollower",
            json!([input(), {"kind": "PeakFollower", "inputs": [u(0), c(0.999)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "runningsum",
            json!([input(), {"kind": "RunningSum", "inputs": [u(0), c(40.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lastvalue",
            json!([input(), {"kind": "LastValue", "inputs": [u(0), c(0.3)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "mostchange",
            json!([input(), {"kind": "LFTri", "inputs": [c(311.0), c(0.0)]},
                {"kind": "MostChange", "inputs": [u(0), u(1)]}, out(2)]),
        )
        .source("noise"),
        Case::new(
            "leastchange",
            json!([input(), {"kind": "LFTri", "inputs": [c(311.0), c(0.0)]},
                {"kind": "LeastChange", "inputs": [u(0), u(1)]}, out(2)]),
        )
        .source("noise"),
        Case::new(
            "zerocrossing",
            json!([input(), {"kind": "ZeroCrossing", "inputs": [u(0)]}, out(1)]),
        )
        .source("sine"),
        Case::new(
            "median",
            json!([input(), {"kind": "Median", "inputs": [c(7.0), u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "moogff",
            json!([input(), {"kind": "MoogFF", "inputs": [u(0), c(900.0), c(3.0), c(0.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "amplitude",
            json!([input(), {"kind": "Amplitude", "inputs": [u(0), c(0.002), c(0.05)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "compander",
            json!([input(), {"kind": "Compander", "inputs": [u(0), u(0), c(0.3), c(1.0), c(0.4), c(0.002), c(0.05)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "normalizer",
            json!([input(), {"kind": "Normalizer", "inputs": [u(0), c(0.7), c(0.005)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "limiter",
            json!([input(), {"kind": "Limiter", "inputs": [u(0), c(0.3), c(0.005)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "hilbert",
            json!([input(), {"kind": "Hilbert", "inputs": [u(0), c(1.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "freqshift",
            json!([input(), {"kind": "FreqShift", "inputs": [u(0), c(213.0), c(0.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "logistic",
            json!([{"kind": "Logistic", "inputs": [c(3.7), c(3000.0), c(0.3)]}, out(0)]),
        ),
        Case::new(
            "fsinosc",
            json!([{"kind": "FSinOsc", "inputs": [c(431.0), c(0.1)]}, out(0)]),
        ),
        Case::new(
            "sinoscfb",
            json!([{"kind": "SinOscFB", "inputs": [c(431.0), c(0.8)]}, out(0)]),
        ),
        Case::new(
            "psingrain",
            json!([{"kind": "PSinGrain", "inputs": [c(1500.0), c(0.05), c(0.4)]}, out(0)]),
        ),
        Case::new(
            "blip",
            json!([{"kind": "Blip", "inputs": [c(301.0), c(12.0)]}, out(0)]),
        ),
        Case::new(
            "formant",
            json!([{"kind": "Formant", "inputs": [c(211.0), c(1300.0), c(600.0)]}, out(0)]),
        ),
        Case::new(
            "cosc",
            json!([{"kind": "COsc", "inputs": [c(0.0), c(301.0), c(2.0)]}, out(0)]),
        ),
        Case::new(
            "vosc3",
            json!([{"kind": "VOsc3", "inputs": [c(0.4), c(101.0), c(202.0), c(303.0)]}, out(0)]),
        ),
        Case::new(
            "klang",
            json!([{"kind": "Klang", "inputs": [c(1.0), c(0.0),
                c(300.0), c(0.3), c(0.0), c(451.0), c(0.2), c(0.25)]}, out(0)]),
        ),
        Case::new(
            "klank",
            json!([input(), {"kind": "Klank", "inputs": [u(0), c(1.0), c(0.0), c(1.0),
                c(800.0), c(0.3), c(0.05), c(1231.0), c(0.2), c(0.02)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lfcub",
            json!([{"kind": "LFCub", "inputs": [c(431.0), c(0.1)]}, out(0)]),
        ),
        Case::new(
            "lfpar",
            json!([{"kind": "LFPar", "inputs": [c(431.0), c(0.1)]}, out(0)]),
        ),
        Case::new(
            "syncsaw",
            json!([{"kind": "SyncSaw", "inputs": [c(301.0), c(977.0)]}, out(0)]),
        ),
        Case::new(
            "lfgauss",
            json!([{"kind": "LFGauss", "inputs": [c(0.01), c(0.2), c(0.0), c(0.0), c(0.0)]}, out(0)]),
        ),
        gated(Case::new(
            "linen",
            json!([
                {"kind": "Linen", "rate": "kr", "inputs": [{"control": 0}, c(0.005), c(0.8), c(0.01), c(0.0)]},
                out(0)
            ]),
        )),
        Case::new(
            "moddif",
            json!([
                input(),
                {"kind": "LFTri", "rate": "kr", "inputs": [c(7.0), c(0.0)]},
                {"kind": "ModDif", "inputs": [u(0), u(1), c(0.7)]},
                out(2)
            ]),
        )
        .source("noise"),
        Case::new(
            "unwrap",
            json!([
                {"kind": "LFSaw", "inputs": [c(1201.0), c(0.0)]},
                {"kind": "Unwrap", "inputs": [u(0), c(-1.0), c(1.0)]},
                out(1)
            ]),
        ),
        Case::new(
            "k2a",
            json!([
                {"kind": "LFSaw", "rate": "kr", "inputs": [c(31.0), c(0.0)]},
                {"kind": "K2A", "inputs": [u(0)]},
                out(1)
            ]),
        ),
        Case::new(
            "t2a",
            json!([
                {"kind": "LFPulse", "rate": "kr", "inputs": [c(300.0), c(0.0), c(0.5)]},
                {"kind": "T2A", "inputs": [u(0), c(3.0)]},
                out(1)
            ]),
        ),
        Case::new(
            "dc",
            json!([{"kind": "DC", "inputs": [c(0.3)]}, out(0)]),
        ),
        Case::new(
            "delay1",
            json!([input(), {"kind": "Delay1", "inputs": [u(0), c(0.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "delay2",
            json!([input(), {"kind": "Delay2", "inputs": [u(0), c(0.0), c(0.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lpz1",
            json!([input(), {"kind": "LPZ1", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "hpz1",
            json!([input(), {"kind": "HPZ1", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lpz2",
            json!([input(), {"kind": "LPZ2", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "hpz2",
            json!([input(), {"kind": "HPZ2", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "bpz2",
            json!([input(), {"kind": "BPZ2", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "brz2",
            json!([input(), {"kind": "BRZ2", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "slope",
            json!([input(), {"kind": "Slope", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "flip",
            json!([input(), {"kind": "Flip", "inputs": [u(0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "twozero",
            json!([input(), {"kind": "TwoZero", "inputs": [u(0), c(800.0), c(0.9)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "fos",
            json!([input(), {"kind": "FOS", "inputs": [u(0), c(0.5), c(0.3), c(0.6)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "sos",
            json!([input(), {"kind": "SOS", "inputs": [u(0), c(0.3), c(0.2), c(0.1), c(1.2), c(-0.5)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "ramp",
            json!([input(), {"kind": "Ramp", "inputs": [u(0), c(0.002)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "slew",
            json!([input(), {"kind": "Slew", "inputs": [u(0), c(400.0), c(900.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "twopole",
            json!([input(), {"kind": "TwoPole", "inputs": [u(0), c(800.0), c(0.9)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "apf",
            json!([input(), {"kind": "APF", "inputs": [u(0), c(800.0), c(0.9)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "ringz",
            json!([input(), {"kind": "Ringz", "inputs": [u(0), c(1200.0), c(0.05)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "formlet",
            json!([input(), {"kind": "Formlet", "inputs": [u(0), c(900.0), c(0.002), c(0.01)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "mideq",
            json!([input(), {"kind": "MidEQ", "inputs": [u(0), c(1000.0), c(0.5), c(6.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "blowpass",
            json!([input(), {"kind": "BLowPass", "inputs": [u(0), c(1500.0), c(0.7)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "bhipass",
            json!([input(), {"kind": "BHiPass", "inputs": [u(0), c(1500.0), c(0.7)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "ballpass",
            json!([input(), {"kind": "BAllPass", "inputs": [u(0), c(1500.0), c(0.7)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "bbandpass",
            json!([input(), {"kind": "BBandPass", "inputs": [u(0), c(1500.0), c(1.2)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "bbandstop",
            json!([input(), {"kind": "BBandStop", "inputs": [u(0), c(1500.0), c(1.2)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "bpeakeq",
            json!([input(), {"kind": "BPeakEQ", "inputs": [u(0), c(1500.0), c(0.7), c(-9.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "blowshelf",
            json!([input(), {"kind": "BLowShelf", "inputs": [u(0), c(600.0), c(1.0), c(6.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "bhishelf",
            json!([input(), {"kind": "BHiShelf", "inputs": [u(0), c(3000.0), c(1.0), c(-6.0)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lag2",
            json!([input(), {"kind": "Lag2", "inputs": [u(0), c(0.003)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lag3",
            json!([input(), {"kind": "Lag3", "inputs": [u(0), c(0.003)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lag2ud",
            json!([input(), {"kind": "Lag2UD", "inputs": [u(0), c(0.002), c(0.006)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lag3ud",
            json!([input(), {"kind": "Lag3UD", "inputs": [u(0), c(0.002), c(0.006)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "lagud",
            json!([input(), {"kind": "LagUD", "inputs": [u(0), c(0.004), c(0.02)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "varlag",
            json!([input(), {"kind": "VarLag", "inputs": [u(0), c(0.004), c(0.0)]}, out(1)]),
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
        gated(Case::new(
            "pauseselfwhendone",
            json!([envgen(0.0), {"kind": "PauseSelfWhenDone", "inputs": [u(0)]}, out(1)]),
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
            "pluck",
            json!([input(), {"kind": "Pluck", "inputs": [u(0), c(1.0), c(0.01), c(0.005), c(0.5), c(0.3)]}, out(1)]),
        )
        .source("noise"),
        Case::new(
            "pitch_shift",
            json!([input(), {"kind": "PitchShift", "inputs": [u(0), c(0.03), c(1.5), c(0.0), c(0.0)]}, out(1)]),
        )
        .source("sine"),
        Case::new(
            "del_tap",
            json!([input(), {"kind": "DelTapWr", "inputs": [c(4.0), u(0)]},
                   {"kind": "DelTapRd", "inputs": [c(4.0), u(1), c(0.0031), c(4.0)]}, out(2)]),
        )
        .source("noise"),
        Case::new(
            "local_buf",
            json!([input(),
                   {"kind": "LocalBuf", "rate": "ir", "inputs": [c(1.0), c(256.0)]},
                   {"kind": "SetBuf", "rate": "ir", "inputs": [u(1), c(10.0), c(2.0), c(0.5), c(-0.5)]},
                   {"kind": "Phasor", "inputs": [c(0.0), c(0.7), c(0.0), c(256.0), c(0.0)]},
                   {"kind": "BufRd", "inputs": [u(1), c(0.0), u(3), c(1.0), c(2.0)]},
                   {"kind": "BufWr", "inputs": [u(1), c(0.0), u(3), c(1.0), u(0)]},
                   out(4)]),
        )
        .source("sine"),
        Case::new(
            "scope_out",
            json!([input(), {"kind": "ScopeOut", "inputs": [c(4.0), u(0)]},
                   {"kind": "Index", "inputs": [c(4.0), c(10.0)]}, out(2)]),
        )
        .source("sine"),
        Case::new(
            "conv",
            json!([input(), {"kind": "Conv", "inputs": [u(0), c(3.0)], "fft_size": 256}, out(1)]),
        )
        .source("noise"),
    ]
    .into_iter()
    .chain(chaos_cases())
    .collect()
}

/// Every chaotic map, at a rate that steps it every few samples.
fn chaos_cases() -> Vec<Case> {
    let params: &[(&'static str, &[f32])] = &[
        ("CuspN", &[1.0, 1.9, 0.0]),
        ("CuspL", &[1.0, 1.9, 0.0]),
        ("GbmanN", &[1.2, 2.1]),
        ("GbmanL", &[1.2, 2.1]),
        ("HenonN", &[1.4, 0.3, 0.0, 0.0]),
        ("HenonL", &[1.4, 0.3, 0.0, 0.0]),
        ("HenonC", &[1.4, 0.3, 0.0, 0.0]),
        ("LatoocarfianN", &[1.0, 3.0, 0.5, 0.5, 0.5, 0.5]),
        ("LatoocarfianL", &[1.0, 3.0, 0.5, 0.5, 0.5, 0.5]),
        ("LatoocarfianC", &[1.0, 3.0, 0.5, 0.5, 0.5, 0.5]),
        ("LinCongN", &[1.1, 0.13, 1.0, 0.0]),
        ("LinCongL", &[1.1, 0.13, 1.0, 0.0]),
        ("LinCongC", &[1.1, 0.13, 1.0, 0.0]),
        ("LorenzL", &[10.0, 28.0, 2.667, 0.05, 0.1, 0.0, 0.0]),
        ("QuadN", &[-1.0, 0.0, 0.75, 0.0]),
        ("QuadL", &[-1.0, 0.0, 0.75, 0.0]),
        ("QuadC", &[-1.0, 0.0, 0.75, 0.0]),
        ("StandardN", &[1.0, 0.5, 0.0]),
        ("StandardL", &[1.0, 0.5, 0.0]),
        ("FBSineN", &[1.0, 0.1, 1.1, 0.5, 0.1, 0.1]),
        ("FBSineL", &[1.0, 0.1, 1.1, 0.5, 0.1, 0.1]),
        ("FBSineC", &[1.0, 0.1, 1.1, 0.5, 0.1, 0.1]),
    ];
    params
        .iter()
        .map(|&(kind, p)| {
            let mut inputs = vec![c(9000.0)];
            inputs.extend(p.iter().map(|&v| c(v)));
            Case::new(kind, json!([{"kind": kind, "inputs": inputs}, out(0)]))
        })
        .collect()
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
                local_buffers: &[],
                offset: 0,
                frames: BLOCK_SIZE,
                transport: Default::default(),
                server: clausters::dsp::ServerFacts::default(),
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

/// `Free` and `Pause` ask again after a reset what they asked fresh, and
/// `SendPeakRMS` measures a new interval from the reset.
#[test]
fn what_a_kind_asks_restarts_after_a_reset() {
    let buses = Buses::new(ControlBuses::new(4), 4);
    let run = |ugen: &mut Box<dyn clausters::dsp::UGen>, inputs: &[&[f32]], kr: bool| {
        let mut ctx = ProcessCtx {
            sample_rate: if kr { SR / BLOCK_SIZE as f32 } else { SR },
            full_sample_rate: SR,
            buses: &buses,
            buffers: &[],
            local_buffers: &[],
            offset: 0,
            frames: BLOCK_SIZE,
            transport: Default::default(),
            server: clausters::dsp::ServerFacts::default(),
        };
        let mut output = [0.0f32; BLOCK_SIZE];
        let n = if kr { 1 } else { BLOCK_SIZE };
        ugen.process(&mut ctx, inputs, &mut output[..n]);
    };
    let ctx = BuildCtx::new(SR, BLOCK_SIZE, 1);
    for (name, gate) in [("Free", 1.0f32), ("Pause", 0.0)] {
        let mut ugen = (lookup(name).unwrap().build)(&UGenConfig::default(), &ctx);
        let (g, id) = ([gate], [7.0f32]);
        let mut asked = Vec::new();
        for reset in [false, false, true] {
            if reset {
                ugen.reset();
            }
            run(&mut ugen, &[&g, &id], true);
            asked.push(ugen.node_request());
        }
        assert!(
            asked[0].is_some() && asked[1].is_none(),
            "{name}: {asked:?}"
        );
        assert_eq!(asked[2], asked[0], "{name}: a reset asks again");
    }
    let config = UGenConfig {
        inputs: 4,
        ..UGenConfig::default()
    };
    let mut ugen = (lookup("SendPeakRMS").unwrap().build)(&config, &ctx);
    let x: Vec<f32> = (0..BLOCK_SIZE)
        .map(|i| (i as f32 * 0.3).sin() * 0.5)
        .collect();
    let (rate, lag, id) = ([SR / BLOCK_SIZE as f32 / 2.0], [0.0f32], [3.0f32]);
    let mut replies = Vec::new();
    for reset in [false, false, true, false] {
        if reset {
            ugen.reset();
        }
        run(&mut ugen, &[&rate, &lag, &id, &x], false);
        ugen.drain_replies(1, &mut |m| replies.push(m));
    }
    // Two blocks an interval: one reply before the reset, one after it.
    assert_eq!(replies.len(), 2, "{} replies", replies.len());
    assert_eq!(
        format!("{:?}", replies[0]),
        format!("{:?}", replies[1]),
        "a reset measures afresh"
    );
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
        .chain(ASKING.iter().copied())
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
