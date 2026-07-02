//! Registry of available UGen kinds: name parsing, input arity, construction.

use crate::dsp::binop::{BinOp, BinaryOp};
use crate::dsp::buf::{BufInfo, BufInfoKind, BufRd, PlayBuf};
use crate::dsp::demand::{Demand, Dseq};
use crate::dsp::disk::{DiskIn, DiskOut};
use crate::dsp::envgen::EnvGen;
use crate::dsp::impulse::Impulse;
use crate::dsp::io::{In, InCtl, Out, ReplaceOut};
use crate::dsp::local::{LocalIn, LocalOut};
use crate::dsp::noise::WhiteNoise;
use crate::dsp::scalar::{Rand, SampleRate};
use crate::dsp::sinosc::SinOsc;
use crate::dsp::{Rate, UGen};

/// Input slot of a [`UGenKind::Demand`] driver that names its demand source
/// (after `trig`, `reset`): must be a wire to a demand-rate (`dr`) UGen.
pub const DEMAND_SOURCE_SLOT: usize = 2;

/// Static, per-UGen parameters that are not signal inputs: set in the SynthDef
/// spec and resolved at compile time, consumed by [`build`]. Empty for almost
/// every UGen; `DiskIn`/`DiskOut` use it to carry their file path and options.
#[derive(Clone, Debug, Default)]
pub struct UGenConfig {
    /// File path for `DiskIn`/`DiskOut`.
    pub path: Option<String>,
    /// `DiskIn`: restart from the top of the file at end of stream.
    pub looping: bool,
    /// `DiskOut` WAV sample format (`int16` | `int24` | `float`).
    pub format: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UGenKind {
    SinOsc,
    Impulse,
    WhiteNoise,
    Add,
    Sub,
    Mul,
    Div,
    In,
    InCtl,
    Out,
    ReplaceOut,
    PlayBuf,
    BufRd,
    BufSampleRate,
    BufRateScale,
    BufFrames,
    BufChannels,
    BufDur,
    DiskIn,
    DiskOut,
    LocalIn,
    LocalOut,
    EnvGen,
    SampleRate,
    Rand,
    Demand,
    Dseq,
}

pub fn parse_kind(name: &str) -> Option<UGenKind> {
    match name {
        "SinOsc" => Some(UGenKind::SinOsc),
        "Impulse" => Some(UGenKind::Impulse),
        "WhiteNoise" => Some(UGenKind::WhiteNoise),
        "Add" => Some(UGenKind::Add),
        "Sub" => Some(UGenKind::Sub),
        "Mul" => Some(UGenKind::Mul),
        "Div" => Some(UGenKind::Div),
        "In" => Some(UGenKind::In),
        "InCtl" => Some(UGenKind::InCtl),
        "Out" => Some(UGenKind::Out),
        "ReplaceOut" => Some(UGenKind::ReplaceOut),
        "PlayBuf" => Some(UGenKind::PlayBuf),
        "BufRd" => Some(UGenKind::BufRd),
        "BufSampleRate" => Some(UGenKind::BufSampleRate),
        "BufRateScale" => Some(UGenKind::BufRateScale),
        "BufFrames" => Some(UGenKind::BufFrames),
        "BufChannels" => Some(UGenKind::BufChannels),
        "BufDur" => Some(UGenKind::BufDur),
        "DiskIn" => Some(UGenKind::DiskIn),
        "DiskOut" => Some(UGenKind::DiskOut),
        "LocalIn" => Some(UGenKind::LocalIn),
        "LocalOut" => Some(UGenKind::LocalOut),
        "EnvGen" => Some(UGenKind::EnvGen),
        "SampleRate" => Some(UGenKind::SampleRate),
        "Rand" => Some(UGenKind::Rand),
        "Demand" => Some(UGenKind::Demand),
        "Dseq" => Some(UGenKind::Dseq),
        _ => None,
    }
}

pub fn arity(kind: UGenKind) -> usize {
    match kind {
        UGenKind::WhiteNoise | UGenKind::SampleRate => 0,
        UGenKind::SinOsc
        | UGenKind::Impulse
        | UGenKind::In
        | UGenKind::InCtl
        | UGenKind::LocalIn
        | UGenKind::BufSampleRate
        | UGenKind::BufRateScale
        | UGenKind::BufFrames
        | UGenKind::BufChannels
        | UGenKind::BufDur
        | UGenKind::DiskIn
        | UGenKind::DiskOut => 1,
        UGenKind::Add
        | UGenKind::Sub
        | UGenKind::Mul
        | UGenKind::Div
        | UGenKind::Out
        | UGenKind::ReplaceOut
        | UGenKind::LocalOut
        // (lo, hi).
        | UGenKind::Rand => 2,
        // Demand: (trig, reset, source).
        UGenKind::Demand => 3,
        // (bufnum, chan, rate, loop) and (bufnum, chan, phase, loop).
        UGenKind::PlayBuf | UGenKind::BufRd => 4,
        // EnvGen: five fixed + the envelope array. Dseq: (repeats, values…).
        UGenKind::EnvGen | UGenKind::Dseq => usize::MAX,
    }
}

/// The output rate a kind takes when the def does not name one explicitly.
/// Everything defaults to [`Rate::Ar`] (the pre-S1 shape, so existing defs are
/// unchanged); the scalar-info UGens default to [`Rate::Ir`] and `Dseq` is
/// demand-rate by nature.
pub fn default_rate(kind: UGenKind) -> Rate {
    match kind {
        UGenKind::SampleRate | UGenKind::Rand => Rate::Ir,
        UGenKind::Dseq => Rate::Dr,
        _ => Rate::Ar,
    }
}

/// Whether a kind may be instantiated at `rate`. The compiler rejects any
/// explicit `rate` outside this set (naming the kind), so a def can only ask
/// for rates a UGen actually implements.
///
/// **The default is the signal-processor case** (`kr`/`ar`), so the open-ended
/// family — oscillators, filters, arithmetic, generators — needs no entry
/// here: a new UGen is audio-or-control-rate for free (and `ar` by default,
/// see [`default_rate`]). Only the two bounded exceptions are listed: the
/// scalar/`ir` and demand/`dr` kinds that *widen* the set, and the block-I/O
/// kinds that *narrow* it to `ar` only (a length-1 wire would drop the block
/// they read or write).
pub fn rate_allowed(kind: UGenKind, rate: Rate) -> bool {
    use Rate::{Ar, Dr, Ir, Kr};
    match kind {
        // Demand source: demand-rate only.
        UGenKind::Dseq => rate == Dr,
        // Init-rate scalars.
        UGenKind::Rand => rate == Ir,
        UGenKind::SampleRate => matches!(rate, Ir | Kr),
        // Buffer-info scalars: frozen (`ir`), per block (`kr`), or broadcast
        // per sample (`ar`, the current default).
        UGenKind::BufSampleRate
        | UGenKind::BufRateScale
        | UGenKind::BufFrames
        | UGenKind::BufChannels
        | UGenKind::BufDur => matches!(rate, Ir | Kr | Ar),
        // Block I/O and stateful streamers: audio-rate only.
        UGenKind::In
        | UGenKind::InCtl
        | UGenKind::Out
        | UGenKind::ReplaceOut
        | UGenKind::PlayBuf
        | UGenKind::BufRd
        | UGenKind::DiskIn
        | UGenKind::DiskOut
        | UGenKind::LocalIn
        | UGenKind::LocalOut
        | UGenKind::EnvGen => rate == Ar,
        // Signal processors (oscillators, math, generators, the Demand
        // driver, and every UGen added later): audio or control rate.
        _ => matches!(rate, Kr | Ar),
    }
}

/// Runs on the network thread (allocates). `config` carries static per-UGen
/// parameters (e.g. `DiskIn`/`DiskOut` file paths); most kinds ignore it.
pub fn build(kind: UGenKind, config: &UGenConfig) -> Box<dyn UGen> {
    match kind {
        UGenKind::SinOsc => Box::new(SinOsc::new()),
        UGenKind::Impulse => Box::new(Impulse::new()),
        UGenKind::WhiteNoise => Box::new(WhiteNoise::new()),
        UGenKind::Add => Box::new(BinaryOp::new(BinOp::Add)),
        UGenKind::Sub => Box::new(BinaryOp::new(BinOp::Sub)),
        UGenKind::Mul => Box::new(BinaryOp::new(BinOp::Mul)),
        UGenKind::Div => Box::new(BinaryOp::new(BinOp::Div)),
        UGenKind::In => Box::new(In),
        UGenKind::InCtl => Box::new(InCtl),
        UGenKind::Out => Box::new(Out),
        UGenKind::ReplaceOut => Box::new(ReplaceOut),
        UGenKind::PlayBuf => Box::new(PlayBuf::new()),
        UGenKind::BufRd => Box::new(BufRd),
        UGenKind::BufSampleRate => Box::new(BufInfo(BufInfoKind::SampleRate)),
        UGenKind::BufRateScale => Box::new(BufInfo(BufInfoKind::RateScale)),
        UGenKind::BufFrames => Box::new(BufInfo(BufInfoKind::Frames)),
        UGenKind::BufChannels => Box::new(BufInfo(BufInfoKind::Channels)),
        UGenKind::BufDur => Box::new(BufInfo(BufInfoKind::Duration)),
        UGenKind::DiskIn => Box::new(DiskIn::open(config)),
        UGenKind::DiskOut => Box::new(DiskOut::open(config)),
        // Intercepted by UGenSynth::process; these are placeholders.
        UGenKind::LocalIn => Box::new(LocalIn),
        UGenKind::LocalOut => Box::new(LocalOut),
        UGenKind::EnvGen => Box::new(EnvGen::new()),
        UGenKind::SampleRate => Box::new(SampleRate),
        UGenKind::Rand => Box::new(Rand::new()),
        // Demand is driven by UGenSynth::process; Dseq is pulled, not run.
        UGenKind::Demand => Box::new(Demand::new()),
        UGenKind::Dseq => Box::new(Dseq::new()),
    }
}
