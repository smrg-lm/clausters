//! Envelope followers and gain control: `Amplitude`, `Compander`, and the
//! look-ahead `Normalizer` and `Limiter`.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    plain(
        "Amplitude",
        &[
            inp("signal", 0.0),
            inp("attacktime", 0.01),
            inp("releasetime", 0.01),
        ],
        |_, _| Box::new(Amplitude::default()),
    ),
    desc(
        "Compander",
        Fixed(7),
        &[
            inp("signal", 0.0),
            inp("control", 0.0),
            inp("thresh", 0.5),
            inp("slopebelow", 1.0),
            inp("slopeabove", 1.0),
            inp("clamptime", 0.01),
            inp("relaxtime", 0.1),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Compander::default()),
    ),
    // `dur` sizes the windows when the synth is built.
    desc(
        "Normalizer",
        Fixed(3),
        I_LOOKAHEAD,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |c, ctx| {
            Box::new(Lookahead::new(
                LookaheadKind::Normalize,
                c.constant(2, 0.01),
                ctx.sample_rate,
            ))
        },
    ),
    desc(
        "Limiter",
        Fixed(3),
        I_LOOKAHEAD,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |c, ctx| {
            Box::new(Lookahead::new(
                LookaheadKind::Limit,
                c.constant(2, 0.01),
                ctx.sample_rate,
            ))
        },
    ),
];

const I_LOOKAHEAD: &[UGenInput] = &[inp("signal", 0.0), inp("level", 1.0), inp("dur", 0.01)];
