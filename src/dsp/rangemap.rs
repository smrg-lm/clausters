//! The range maps as a UGen: `RangeMapUGen`, one generic kind carrying the map
//! by name, the way `BinaryOpUGen` carries an operator.
//!
//! **There is one implementation of these curves and it is not here.**
//! `clausters_core::warp` holds the eight maps a client already computes
//! values with (`linlin`, `linexp`, `explin`, `expexp`, `lincurve`,
//! `curvelin`, `range`, `exprange`); this file is the plumbing that lets a
//! *signal* through the same function, so a def that maps an LFO onto a
//! frequency and a script that maps a fader position onto one agree by
//! construction rather than by care.
//!
//! **What makes it cheap is that the bounds usually hold still.** A map's
//! ranges decide a good deal of the arithmetic before any value arrives — the
//! zero-straddling test, `ln(hi/lo)`, `exp(curve)`, the bend's two terms — and
//! two of those are transcendentals. So when every bound input is a constant,
//! which is the ordinary def, the block builds one [`Map`] and every sample
//! pays only [`Map::at`]. A bound that is a *signal* is legal and costs the
//! whole preparation per sample; that is the price of modulating a range, and
//! it is charged only to the def that asks for it.

use clausters_core::warp::{Clip, Map, MapOp};

use crate::dsp::{ProcessCtx, UGen, at};

/// `RangeMapUGen(in, in_lo, in_hi, out_lo, out_hi, curve)` with the map and the
/// clip as static fields.
pub struct RangeMap {
    op: MapOp,
    clip: Clip,
}

impl RangeMap {
    /// The map named by the compiler-resolved opcode index, with the clip the
    /// def asked for. An index outside the table cannot reach here — the
    /// compiler resolves the name — so an unknown one falls back to the first
    /// map rather than growing a failure path the wire cannot produce.
    pub fn from_index(op: u32, clip: u32) -> Self {
        Self {
            op: MapOp::from_u32(op).unwrap_or(MapOp::Linlin),
            clip: Clip::from_u32(clip).unwrap_or_default(),
        }
    }
}

impl UGen for RangeMap {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let bounds = &inputs[1..6];
        if bounds.iter().all(|b| b.len() == 1) {
            let map = Map::new(
                self.op,
                bounds[0][0],
                bounds[1][0],
                bounds[2][0],
                bounds[3][0],
                bounds[4][0],
                self.clip,
            );
            for (i, out) in output.iter_mut().enumerate() {
                *out = map.at(at(inputs[0], i));
            }
        } else {
            for (i, out) in output.iter_mut().enumerate() {
                let map = Map::new(
                    self.op,
                    at(bounds[0], i),
                    at(bounds[1], i),
                    at(bounds[2], i),
                    at(bounds[3], i),
                    at(bounds[4], i),
                    self.clip,
                );
                *out = map.at(at(inputs[0], i));
            }
        }
    }
}
