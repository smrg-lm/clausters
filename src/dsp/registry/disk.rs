//! Streaming disk I/O and synth-private feedback.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- streaming disk I/O (need a path; see dsp::disk) ---
    desc(
        "DiskIn",
        Fixed(1),
        &[inp("chan", 0.0)],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        true,
        |c, ctx| Box::new(DiskIn::open(c, &ctx.meters)),
    ),
    desc(
        "DiskOut",
        Fixed(1),
        &[inp("signal", 0.0)],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        true,
        |c, ctx| Box::new(DiskOut::open(c, &ctx.meters)),
    ),
    // --- a segment list streamed from disk, following the transport ---
    #[cfg(not(target_arch = "wasm32"))]
    desc(
        "TransportDiskIn",
        Fixed(5),
        &[
            inp("bufnum", 0.0),
            inp("chan", 0.0),
            inp_opt("offset", 0.0),
            inp_opt("start", 0.0),
            inp_opt("loop", 0.0),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, ctx| Box::new(crate::dsp::stream::TransportDiskIn::open(&ctx.meters)),
    ),
    // --- synth-private feedback (synth-coordinated execution) ---
    desc(
        "LocalIn",
        Fixed(1),
        &[inp("channel", 0.0)],
        Ar,
        R_AR,
        ExecLocalIn,
        BusRole::None,
        false,
        |_, _| Box::new(LocalIn),
    ),
    desc(
        "LocalOut",
        Fixed(2),
        &[inp("channel", 0.0), inp("signal", 0.0)],
        Ar,
        R_AR,
        ExecLocalOut,
        BusRole::None,
        false,
        |_, _| Box::new(LocalOut),
    ),
];
