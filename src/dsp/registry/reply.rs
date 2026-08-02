//! The side-effect UGens: reply and observe, no `Out` required.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- side-effect UGens: reply/observe, no `Out` required. Control or
    //     audio rate; their output is silence (SendTrig/SendReply) or the
    //     polled signal passed through (Poll). ---
    desc(
        "SendTrig",
        Fixed(3),
        &[inp("trig", 0.0), inp("id", 0.0), inp("value", 0.0)],
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
        &[inp("trig", 0.0), inp("signal", 0.0), inp("trig_id", -1.0)],
        Kr,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(Poll::new(c)),
    ),
];
