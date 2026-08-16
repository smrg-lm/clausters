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
];
