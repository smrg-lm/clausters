//! The delay core: one circular line behind nine names.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- the delay core: one circular line, parameterized by
    //     interpolation and by what it feeds back. The line is synth-private
    //     memory sized at build from `max_delay` and the sample rate - a pool
    //     buffer is immutable, and this one is written every sample. See
    //     `dsp::delay`. ---
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
];
