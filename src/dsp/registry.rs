//! Registry of available UGen kinds: name parsing, input arity, construction.

use crate::dsp::UGen;
use crate::dsp::binop::{BinOp, BinaryOp};
use crate::dsp::noise::WhiteNoise;
use crate::dsp::sinosc::SinOsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UGenKind {
    SinOsc,
    WhiteNoise,
    Add,
    Sub,
    Mul,
    Div,
}

pub fn parse_kind(name: &str) -> Option<UGenKind> {
    match name {
        "SinOsc" => Some(UGenKind::SinOsc),
        "WhiteNoise" => Some(UGenKind::WhiteNoise),
        "Add" => Some(UGenKind::Add),
        "Sub" => Some(UGenKind::Sub),
        "Mul" => Some(UGenKind::Mul),
        "Div" => Some(UGenKind::Div),
        _ => None,
    }
}

pub fn arity(kind: UGenKind) -> usize {
    match kind {
        UGenKind::SinOsc => 1, // freq
        UGenKind::WhiteNoise => 0,
        UGenKind::Add | UGenKind::Sub | UGenKind::Mul | UGenKind::Div => 2,
    }
}

/// Runs on the network thread (allocates).
pub fn build(kind: UGenKind) -> Box<dyn UGen> {
    match kind {
        UGenKind::SinOsc => Box::new(SinOsc::new()),
        UGenKind::WhiteNoise => Box::new(WhiteNoise::new()),
        UGenKind::Add => Box::new(BinaryOp::new(BinOp::Add)),
        UGenKind::Sub => Box::new(BinaryOp::new(BinOp::Sub)),
        UGenKind::Mul => Box::new(BinaryOp::new(BinOp::Mul)),
        UGenKind::Div => Box::new(BinaryOp::new(BinOp::Div)),
    }
}
