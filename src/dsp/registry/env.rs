//! The envelope generator.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- envelope ---
    // Variadic: the five named slots are the fixed head; the envelope's own
    // breakpoint array follows and is unnamed by nature.
    desc_done(desc(
        "EnvGen",
        Variadic,
        &[
            inp("gate", 1.0),
            inp("level_scale", 1.0),
            inp("level_bias", 0.0),
            inp("time_scale", 1.0),
            inp("done_action", 0.0),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(EnvGen::new()),
    )),
    // The one-segment envelopes: the same engine with its header filled
    // in, so they inherit the shape arithmetic and the whole done-action set.
    // Unlike `EnvGen` they run at either rate — a ramp is the archetypal `kr`
    // UGen, and a `kr` one costs a block's worth of work per block.
    desc_done(desc(
        "Line",
        Fixed(4),
        I_LINE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Line::new(LineShape::Linear)),
    )),
    desc_done(desc(
        "XLine",
        Fixed(4),
        I_XLINE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Line::new(LineShape::Exponential)),
    )),
];
