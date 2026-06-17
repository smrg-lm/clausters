//! C0: the server's native operators must agree with `clausters_core` — the
//! shared crate is the single source of truth, so a client computing with the
//! core matches the server by construction. These tests drive the real
//! `UGen::process` path and compare it to direct core calls.

use std::sync::Arc;

use clausters::dsp::binop::{BinOp, BinaryOp};
use clausters::dsp::buffer::Buffer;
use clausters::dsp::noise::WhiteNoise;
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx, UGen};
use clausters_core::builtins::{self, BinaryOp as CoreBinaryOp};

/// A throwaway context for UGens that only need to run (binop/noise ignore the
/// buses and buffers).
fn ctx<'a>(buses: &'a Buses, buffers: &'a [Option<Arc<Buffer>>]) -> ProcessCtx<'a> {
    ProcessCtx {
        sample_rate: 48_000.0,
        buses,
        buffers,
        offset: 0,
        frames: BLOCK_SIZE,
    }
}

#[test]
fn binary_ops_match_the_core_bit_for_bit() {
    let buses = Buses::new(ControlBuses::new());
    let buffers: Vec<Option<Arc<Buffer>>> = vec![];
    let mut ctx = ctx(&buses, &buffers);

    // A varied block and a broadcast constant exercise both `at` branches.
    let a: Vec<f32> = (0..BLOCK_SIZE).map(|i| i as f32 * 0.5 - 7.0).collect();
    let b: Vec<f32> = (0..BLOCK_SIZE).map(|i| (i as f32).sin() * 3.0).collect();
    let konst = [0.25f32];

    for (op, core) in [
        (BinOp::Add, CoreBinaryOp::Add),
        (BinOp::Sub, CoreBinaryOp::Sub),
        (BinOp::Mul, CoreBinaryOp::Mul),
        (BinOp::Div, CoreBinaryOp::Div),
    ] {
        for (x, y) in [(a.as_slice(), b.as_slice()), (a.as_slice(), &konst[..])] {
            let mut got = vec![0.0f32; BLOCK_SIZE];
            BinaryOp::new(op).process(&mut ctx, &[x, y], &mut got);

            let mut expect = vec![0.0f32; BLOCK_SIZE];
            builtins::binary_slice(core, x, y, &mut expect);

            assert_eq!(got, expect, "op {op:?} diverged from the core");
        }
    }
}

/// The server's `WhiteNoise` now delegates wholesale to
/// `clausters_core::rng::WhiteNoise` (exactness covered by that crate's unit
/// tests). Here we just confirm the delegation actually drives the UGen path:
/// in range and not a constant.
#[test]
fn white_noise_delegation_runs() {
    let buses = Buses::new(ControlBuses::new());
    let buffers: Vec<Option<Arc<Buffer>>> = vec![];
    let mut ctx = ctx(&buses, &buffers);

    let mut out = vec![0.0f32; BLOCK_SIZE];
    WhiteNoise::new().process(&mut ctx, &[], &mut out);

    assert!(out.iter().all(|s| (-1.0..1.0).contains(s)), "out of range");
    assert!(
        out.windows(2).any(|w| w[0] != w[1]),
        "noise should not be constant"
    );
}
