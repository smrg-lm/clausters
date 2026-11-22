//! The delay core: one circular line behind nine names.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- the delay core: one circular line, parameterized by interpolation,
    //     by what it feeds back, and by where its samples live. The nine names
    //     below hold private memory sized at build from `max_delay` and the
    //     sample rate; the nine `Buf*` ones after them use a channel of a pool
    //     buffer instead, so the line's contents are addressable. Same
    //     arithmetic either way. See `dsp::delay`. ---
    desc(
        "DelayN",
        Fixed(2),
        I_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::None,
                Feedback::None,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "CombN",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::None,
                Feedback::Comb,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "AllpassN",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::None,
                Feedback::Allpass,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "DelayL",
        Fixed(2),
        I_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Linear,
                Feedback::None,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "CombL",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Linear,
                Feedback::Comb,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "AllpassL",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Linear,
                Feedback::Allpass,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "DelayC",
        Fixed(2),
        I_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Cubic,
                Feedback::None,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "CombC",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Cubic,
                Feedback::Comb,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "AllpassC",
        Fixed(3),
        I_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            Box::new(Delay::new(
                Interp::Cubic,
                Feedback::Allpass,
                c.max_delay.unwrap_or(DEFAULT_MAX_DELAY),
                b.sample_rate,
            ))
        },
    ),
    desc(
        "BufDelayN",
        Fixed(4),
        I_BUF_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::None, Feedback::None)),
    ),
    desc(
        "BufCombN",
        Fixed(5),
        I_BUF_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::None, Feedback::Comb)),
    ),
    desc(
        "BufAllpassN",
        Fixed(5),
        I_BUF_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::None, Feedback::Allpass)),
    ),
    desc(
        "BufDelayL",
        Fixed(4),
        I_BUF_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::Linear, Feedback::None)),
    ),
    desc(
        "BufCombL",
        Fixed(5),
        I_BUF_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::Linear, Feedback::Comb)),
    ),
    desc(
        "BufAllpassL",
        Fixed(5),
        I_BUF_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::Linear, Feedback::Allpass)),
    ),
    desc(
        "BufDelayC",
        Fixed(4),
        I_BUF_DELAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::Cubic, Feedback::None)),
    ),
    desc(
        "BufCombC",
        Fixed(5),
        I_BUF_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::Cubic, Feedback::Comb)),
    ),
    desc(
        "BufAllpassC",
        Fixed(5),
        I_BUF_DELAY_DECAY,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Delay::over_buffer(Interp::Cubic, Feedback::Allpass)),
    ),
    // --- the delay lines beyond the nine (`dsp::lines`): a plucked string,
    //     a granular pitch shifter, and a tap pair over a buffer. Each
    //     private line is sized at build from a constant input. ---
    plain(
        "Pluck",
        &[
            inp("signal", 0.0),
            inp("trig", 1.0),
            inp("maxdelaytime", 0.2),
            inp("delaytime", 0.2),
            inp("decaytime", 1.0),
            inp("coef", 0.5),
        ],
        |c, ctx| Box::new(Pluck::new(c.constant(2, 0.2), ctx.sample_rate)),
    ),
    desc_draws(desc(
        "PitchShift",
        Fixed(5),
        &[
            inp("signal", 0.0),
            inp("window_size", 0.2),
            inp("pitch_ratio", 1.0),
            inp("pitch_dispersion", 0.0),
            inp("time_dispersion", 0.0),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |c, ctx| {
            Box::new(PitchShift::new(
                ctx.rand(),
                c.constant(1, 0.2),
                ctx.sample_rate,
            ))
        },
    )),
    plain(
        "DelTapWr",
        &[inp("bufnum", 0.0), inp("signal", 0.0)],
        |_, _| Box::new(DelTapWr::default()),
    ),
    plain(
        "DelTapRd",
        &[
            inp("bufnum", 0.0),
            inp("phase", 0.0),
            inp("deltime", 0.0),
            inp_opt("interp", 1.0),
        ],
        |_, _| Box::new(DelTapRd::default()),
    ),
];
