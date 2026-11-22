//! Noise, by spectrum and by density.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- noise ---
    desc_draws(desc(
        "PinkNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(PinkNoise::new(ctx.rand())),
    )),
    desc_draws(desc(
        "BrownNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(BrownNoise::new(ctx.rand())),
    )),
    desc_draws(desc(
        "GrayNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(GrayNoise::new(ctx.rand())),
    )),
    desc_draws(desc(
        "ClipNoise",
        Fixed(0),
        I_NONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(ClipNoise::new(ctx.rand())),
    )),
    desc_draws(desc(
        "LFNoise0",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::new(LfNoiseShape::Step, ctx.rand())),
    )),
    desc_draws(desc(
        "LFNoise1",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::new(LfNoiseShape::Linear, ctx.rand())),
    )),
    desc_draws(desc(
        "LFNoise2",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::new(LfNoiseShape::Quadratic, ctx.rand())),
    )),
    desc_draws(desc(
        "LFClipNoise",
        Fixed(1),
        I_LF_NOISE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(LfNoise::new(LfNoiseShape::Clip, ctx.rand())),
    )),
    desc_draws(desc(
        "Dust",
        Fixed(1),
        I_DENSITY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dust::new(DustMode::Unipolar, ctx.rand())),
    )),
    desc_draws(desc(
        "Dust2",
        Fixed(1),
        I_DENSITY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Dust::new(DustMode::Bipolar, ctx.rand())),
    )),
    desc_draws(desc(
        "Crackle",
        Fixed(1),
        I_CHAOS,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(Crackle::new(ctx.rand())),
    )),
    // --- the dynamic noises and the logistic map ---
    desc_draws(plain("LFDNoise0", I_FREQ500, |_, ctx| {
        Box::new(LfdNoise::new(LfdShape::Step, ctx.rand()))
    })),
    desc_draws(plain("LFDNoise1", I_FREQ500, |_, ctx| {
        Box::new(LfdNoise::new(LfdShape::Linear, ctx.rand()))
    })),
    desc_draws(plain("LFDNoise3", I_FREQ500, |_, ctx| {
        Box::new(LfdNoise::new(LfdShape::Cubic, ctx.rand()))
    })),
    desc_draws(plain("LFDClipNoise", I_FREQ500, |_, ctx| {
        Box::new(LfdNoise::new(LfdShape::Clip, ctx.rand()))
    })),
    plain(
        "Logistic",
        &[
            inp("chaos_param", 3.0),
            inp("freq", 1000.0),
            inp("init", 0.5),
        ],
        |_, _| Box::new(Logistic::new()),
    ),
];

const I_FREQ500: &[UGenInput] = &[inp("freq", 500.0)];
