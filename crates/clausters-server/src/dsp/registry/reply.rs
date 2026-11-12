//! The side-effect UGens: reply and observe, no `Out` required.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- the meter: a level with the ballistics a person can read
    //     (`dsp::measure`); one number a block, which is what a control bus
    //     carries. ---
    desc(
        "Meter",
        Fixed(3),
        &[
            inp("signal", 0.0),
            inp_opt("decay", clausters_core::measure::METER_FALL_DB),
            inp_opt("hold", 0.0),
        ],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(crate::dsp::measure::Meter::new()),
    ),
    // --- the true peak: a meter's level over the reconstructed signal rather
    //     than over its samples (`dsp::measure`), in dBTP. ---
    desc(
        "TruePeak",
        Fixed(3),
        &[
            inp("signal", 0.0),
            inp_opt("decay", clausters_core::measure::METER_FALL_DB),
            inp_opt("hold", 0.0),
        ],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(crate::dsp::measure::TruePeak::new()),
    ),
    // --- the clip count: how many times the signal was flattened, which is
    //     a run of samples at full scale and not a peak (`dsp::measure`). ---
    desc(
        "ClipCount",
        Fixed(3),
        &[
            inp("signal", 0.0),
            inp_opt("ceiling", clausters_core::measure::CLIP_CEILING),
            inp_opt("run", clausters_core::measure::CLIP_RUN as f32),
        ],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(crate::dsp::measure::ClipCount::new()),
    ),
    // --- side-effect UGens: reply/observe, no `Out` required. Control or
    //     audio rate; their output is silence (SendTrig/SendReply) or the
    //     polled signal passed through (Poll). ---
    desc(
        "SendTrig",
        Fixed(3),
        &[inp("trig", 0.0), inp_opt("id", 0.0), inp_opt("value", 0.0)],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(SendTrig::new()),
    ),
    // Variadic: `trig`/`reply_id` are the head, the reported values follow.
    desc(
        "SendReply",
        Variadic,
        &[inp("trig", 0.0), inp("reply_id", -1.0)],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(SendReply::new(c)),
    ),
    desc(
        "Poll",
        Fixed(3),
        &[
            inp("trig", 0.0),
            inp("signal", 0.0),
            inp_opt("trig_id", -1.0),
        ],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(Poll::new(c)),
    ),
];
