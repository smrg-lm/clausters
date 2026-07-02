//! Registry of available UGen kinds: name parsing, input arity, construction.

use crate::dsp::UGen;
use crate::dsp::binop::{BinOp, BinaryOp};
use crate::dsp::buf::{BufInfo, BufInfoKind, BufRd, PlayBuf};
use crate::dsp::disk::{DiskIn, DiskOut};
use crate::dsp::envgen::EnvGen;
use crate::dsp::impulse::Impulse;
use crate::dsp::io::{In, InCtl, Out, ReplaceOut};
use crate::dsp::local::{LocalIn, LocalOut};
use crate::dsp::noise::WhiteNoise;
use crate::dsp::sinosc::SinOsc;

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
        _ => None,
    }
}

pub fn arity(kind: UGenKind) -> usize {
    match kind {
        UGenKind::WhiteNoise => 0,
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
        | UGenKind::LocalOut => 2,
        // (bufnum, chan, rate, loop) and (bufnum, chan, phase, loop).
        UGenKind::PlayBuf | UGenKind::BufRd => 4,
        UGenKind::EnvGen => usize::MAX,
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
    }
}
