//! Reverberators and physical models.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- Freeverb and GVerb (`dsp::reverb`), their lines sized at build;
    //     a stereo row is one output channel, the index last ---
    plain(
        "FreeVerb",
        &[
            inp("signal", 0.0),
            inp_opt("mix", 0.33),
            inp_opt("room", 0.5),
            inp_opt("damp", 0.5),
        ],
        |_, ctx| Box::new(FreeVerb::new(ctx.sample_rate)),
    ),
    plain(
        "FreeVerb2",
        &[
            inp("signal", 0.0),
            inp("signal2", 0.0),
            inp_opt("mix", 0.33),
            inp_opt("room", 0.5),
            inp_opt("damp", 0.5),
            inp_opt("chan", 0.0),
        ],
        |c, ctx| Box::new(FreeVerb2::new(ctx.sample_rate, c.constant(5, 0.0))),
    ),
    plain(
        "GVerb",
        &[
            inp("signal", 0.0),
            inp("roomsize", 10.0),
            inp("revtime", 3.0),
            inp("damping", 0.5),
            inp("inputbw", 0.5),
            inp("spread", 15.0),
            inp("drylevel", 1.0),
            inp("earlyreflevel", 0.7),
            inp("taillevel", 0.5),
            inp("maxroomsize", 300.0),
            inp_opt("chan", 0.0),
        ],
        |c, ctx| {
            Box::new(GVerb::new(
                ctx.sample_rate,
                c.constant(9, 300.0),
                c.constant(10, 0.0),
            ))
        },
    ),
    // --- a spring and a bouncing ball (`dsp::physical`) ---
    plain(
        "Spring",
        &[inp("signal", 0.0), inp("spring", 1.0), inp("damp", 0.0)],
        |_, _| Box::new(Spring::default()),
    ),
    desc_draws(plain(
        "Ball",
        &[
            inp("signal", 0.0),
            inp("g", 1.0),
            inp("damp", 0.0),
            inp("friction", 0.01),
        ],
        |_, ctx| Box::new(Ball::new(BallKind::Height, ctx.rand())),
    )),
    desc_draws(plain(
        "TBall",
        &[
            inp("signal", 0.0),
            inp("g", 10.0),
            inp("damp", 0.0),
            inp("friction", 0.01),
        ],
        |_, ctx| Box::new(Ball::new(BallKind::Impacts, ctx.rand())),
    )),
];
