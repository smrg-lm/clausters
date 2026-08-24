//! Arithmetic: the generic op UGens and the fused forms.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- arithmetic: the generic op UGens, selected by a core opcode
    //     index; every math need is one more `clausters_core::builtins` entry,
    //     not a new kind. `Add`/`Sub`/`Mul`/`Div` stay as thin aliases below
    //     for back-compat with existing defs. ---
    desc_op("BinaryOpUGen", Fixed(2), I_AB, OpFamily::Binary, |c, _| {
        Box::new(BinaryOp::from_index(c.op.unwrap_or(0)))
    }),
    desc_op("UnaryOpUGen", Fixed(1), I_A, OpFamily::Unary, |c, _| {
        Box::new(UnaryOp::from_index(c.op.unwrap_or(0)))
    }),
    // The range maps (`clausters_core::warp`), one generic kind carrying the
    // map by name -- the same argument `BinaryOpUGen` makes, and the same
    // function a client computes a *value* with, so a mapped signal and a
    // mapped fader position cannot drift. `clip` rides as static config beside
    // `op`; the bounds are ordinary inputs, so a range may be modulated.
    desc_op("RangeMapUGen", Fixed(6), I_MAP, OpFamily::Map, |c, _| {
        Box::new(RangeMap::from_index(c.op.unwrap_or(0), c.clip.unwrap_or(0)))
    }),
    // Fused forms scsynth optimizes (fixed kinds, not op-table entries).
    desc(
        "MulAdd",
        Fixed(3),
        I_ABC,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(MulAdd),
    ),
    desc(
        "Sum3",
        Fixed(3),
        I_ABC,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Sum3),
    ),
    desc(
        "Sum4",
        Fixed(4),
        &[inp("a", 0.0), inp("b", 0.0), inp("c", 0.0), inp("d", 0.0)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Sum4),
    ),
    // Aliases for the four operator kinds (back-compat).
    desc(
        "Add",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Add)),
    ),
    desc(
        "Sub",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Sub)),
    ),
    desc(
        "Mul",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Mul)),
    ),
    desc(
        "Div",
        Fixed(2),
        I_AB,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(BinaryOp::new(BinOp::Div)),
    ),
];
