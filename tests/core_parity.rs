//! C0: the server's native operators must agree with `clausters_core` — the
//! shared crate is the single source of truth, so a client computing with the
//! core matches the server by construction. These tests drive the real
//! `UGen::process` path and compare it to direct core calls.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::dsp::binop::{BinOp, BinaryOp};
use clausters::dsp::buffer::Buffer;
use clausters::dsp::fused::{MulAdd, Sum3, Sum4};
use clausters::dsp::noise::WhiteNoise;
use clausters::dsp::unop::UnaryOp;
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx, UGen};
use clausters_core::builtins::{self, BinaryOp as CoreBinaryOp, UnaryOp as CoreUnaryOp};

/// A throwaway context for UGens that only need to run (binop/noise ignore the
/// buses and buffers).
fn ctx<'a>(buses: &'a Buses, buffers: &'a [Option<Arc<Buffer>>]) -> ProcessCtx<'a> {
    ProcessCtx {
        sample_rate: 48_000.0,
        full_sample_rate: 48_000.0,
        buses,
        buffers,
        offset: 0,
        frames: BLOCK_SIZE,
    }
}

/// Bit-pattern view of a block, so "bit-identical" comparisons treat two equal
/// NaNs (e.g. `asin` out of domain) as equal — `NaN != NaN` would otherwise
/// spuriously fail an exact-match test.
fn bits(xs: &[f32]) -> Vec<u32> {
    xs.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn binary_ops_match_the_core_bit_for_bit() {
    let buses = Buses::new(ControlBuses::new(1024), 128);
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

/// S3: the generic `BinaryOpUGen` (built from an opcode index) must match the
/// core over the *whole* opcode table, driven through the real UGen path.
#[test]
fn binary_op_ugen_matches_the_core_over_the_table() {
    let buses = Buses::new(ControlBuses::new(1024), 128);
    let buffers: Vec<Option<Arc<Buffer>>> = vec![];
    let mut ctx = ctx(&buses, &buffers);

    // Values chosen positive-ish so pitch/log ops stay finite where they apply;
    // both a full block and a broadcast constant exercise the `at` branches.
    let a: Vec<f32> = (0..BLOCK_SIZE).map(|i| i as f32 * 0.1 + 0.5).collect();
    let b: Vec<f32> = (0..BLOCK_SIZE).map(|i| (i as f32).cos() + 1.5).collect();
    let konst = [0.75f32];

    let mut op = 0u32;
    while let Some(core) = CoreBinaryOp::from_u32(op) {
        for (x, y) in [(a.as_slice(), b.as_slice()), (a.as_slice(), &konst[..])] {
            let mut got = vec![0.0f32; BLOCK_SIZE];
            BinaryOp::from_index(op).process(&mut ctx, &[x, y], &mut got);
            let mut expect = vec![0.0f32; BLOCK_SIZE];
            builtins::binary_slice(core, x, y, &mut expect);
            assert_eq!(bits(&got), bits(&expect), "binary op {core:?} diverged");
        }
        op += 1;
    }
    assert!(op >= 35, "expected the full binary table, stopped at {op}");
}

/// S3: `UnaryOpUGen` over the whole unary opcode table, through the UGen path.
#[test]
fn unary_op_ugen_matches_the_core_over_the_table() {
    let buses = Buses::new(ControlBuses::new(1024), 128);
    let buffers: Vec<Option<Arc<Buffer>>> = vec![];
    let mut ctx = ctx(&buses, &buffers);

    let x: Vec<f32> = (0..BLOCK_SIZE).map(|i| i as f32 * 0.05 + 0.5).collect();

    let mut op = 0u32;
    while let Some(core) = CoreUnaryOp::from_u32(op) {
        let mut got = vec![0.0f32; BLOCK_SIZE];
        UnaryOp::from_index(op).process(&mut ctx, &[&x], &mut got);
        let mut expect = vec![0.0f32; BLOCK_SIZE];
        builtins::unary_slice(core, &x, &mut expect);
        assert_eq!(bits(&got), bits(&expect), "unary op {core:?} diverged");
        op += 1;
    }
    assert!(op >= 37, "expected the full unary table, stopped at {op}");
}

/// S3: the fused UGens equal the same expression composed from core operators.
#[test]
fn fused_ugens_match_the_core_composition() {
    let buses = Buses::new(ControlBuses::new(1024), 128);
    let buffers: Vec<Option<Arc<Buffer>>> = vec![];
    let mut ctx = ctx(&buses, &buffers);

    let a: Vec<f32> = (0..BLOCK_SIZE).map(|i| i as f32 * 0.5 - 7.0).collect();
    let b: Vec<f32> = (0..BLOCK_SIZE).map(|i| (i as f32).sin() * 3.0).collect();
    let c: Vec<f32> = (0..BLOCK_SIZE).map(|i| i as f32 * -0.25 + 1.0).collect();
    let d = [2.0f32];

    let mul_add = |x: f32, y: f32, z: f32| {
        builtins::apply_binary(
            CoreBinaryOp::Add,
            builtins::apply_binary(CoreBinaryOp::Mul, x, y),
            z,
        )
    };

    let mut got = vec![0.0f32; BLOCK_SIZE];
    MulAdd.process(&mut ctx, &[&a, &b, &c], &mut got);
    for (i, g) in got.iter().enumerate() {
        assert_eq!(*g, mul_add(a[i], b[i], c[i]), "MulAdd frame {i}");
    }

    let mut got3 = vec![0.0f32; BLOCK_SIZE];
    Sum3.process(&mut ctx, &[&a, &b, &c], &mut got3);
    for (i, g) in got3.iter().enumerate() {
        let s = builtins::apply_binary(
            CoreBinaryOp::Add,
            builtins::apply_binary(CoreBinaryOp::Add, a[i], b[i]),
            c[i],
        );
        assert_eq!(*g, s, "Sum3 frame {i}");
    }

    let mut got4 = vec![0.0f32; BLOCK_SIZE];
    Sum4.process(&mut ctx, &[&a, &b, &c, &d], &mut got4);
    for (i, g) in got4.iter().enumerate() {
        let ab = builtins::apply_binary(CoreBinaryOp::Add, a[i], b[i]);
        let abc = builtins::apply_binary(CoreBinaryOp::Add, ab, c[i]);
        let s = builtins::apply_binary(CoreBinaryOp::Add, abc, d[0]);
        assert_eq!(*g, s, "Sum4 frame {i}");
    }
}

/// The server's `WhiteNoise` now delegates wholesale to
/// `clausters_core::rng::WhiteNoise` (exactness covered by that crate's unit
/// tests). Here we just confirm the delegation actually drives the UGen path:
/// in range and not a constant.
#[test]
fn white_noise_delegation_runs() {
    let buses = Buses::new(ControlBuses::new(1024), 128);
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
