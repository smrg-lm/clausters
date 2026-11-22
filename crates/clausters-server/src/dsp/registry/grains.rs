//! Granular synthesis.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- grains of a source (`dsp::grains`): one row per output channel,
    //     the channel count and the index last; `max_grains` sizes the rows ---
    plain(
        "GrainIn",
        &[
            inp("trigger", 0.0),
            inp("dur", 1.0),
            inp("signal", 0.0),
            inp("pan", 0.0),
            inp("envbufnum", -1.0),
            inp("max_grains", 512.0),
            inp("numchans", 1.0),
            inp_opt("chan", 0.0),
        ],
        |c, _| Box::new(Granular::new(GrainKind::In, c.constant(5, 512.0))),
    ),
    plain(
        "GrainSin",
        &[
            inp("trigger", 0.0),
            inp("dur", 1.0),
            inp("freq", 440.0),
            inp("pan", 0.0),
            inp("envbufnum", -1.0),
            inp("max_grains", 512.0),
            inp("numchans", 1.0),
            inp_opt("chan", 0.0),
        ],
        |c, _| Box::new(Granular::new(GrainKind::Sin, c.constant(5, 512.0))),
    ),
    plain(
        "GrainFM",
        &[
            inp("trigger", 0.0),
            inp("dur", 1.0),
            inp("carfreq", 440.0),
            inp("modfreq", 200.0),
            inp("index", 1.0),
            inp("pan", 0.0),
            inp("envbufnum", -1.0),
            inp("max_grains", 512.0),
            inp("numchans", 1.0),
            inp_opt("chan", 0.0),
        ],
        |c, _| Box::new(Granular::new(GrainKind::Fm, c.constant(7, 512.0))),
    ),
    plain(
        "GrainBuf",
        &[
            inp("trigger", 0.0),
            inp("dur", 1.0),
            inp("sndbuf", 0.0),
            inp("rate", 1.0),
            inp("pos", 0.0),
            inp("interp", 2.0),
            inp("pan", 0.0),
            inp("envbufnum", -1.0),
            inp("max_grains", 512.0),
            inp("numchans", 1.0),
            inp_opt("chan", 0.0),
        ],
        |c, _| Box::new(Granular::new(GrainKind::Buf, c.constant(8, 512.0))),
    ),
    plain(
        "TGrains",
        &[
            inp("trigger", 0.0),
            inp("bufnum", 0.0),
            inp("rate", 1.0),
            inp("center_pos", 0.0),
            inp("dur", 0.1),
            inp("pan", 0.0),
            inp("amp", 0.1),
            inp("interp", 4.0),
            inp("numchans", 2.0),
            inp_opt("chan", 0.0),
        ],
        |_, _| Box::new(TGrains::default()),
    ),
    // One row per channel of the buffer.
    desc_draws(plain(
        "Warp1",
        &[
            inp("bufnum", 0.0),
            inp("pointer", 0.0),
            inp("freq_scale", 1.0),
            inp("window_size", 0.2),
            inp("envbufnum", -1.0),
            inp("overlaps", 8.0),
            inp("window_rand_ratio", 0.0),
            inp("interp", 1.0),
            inp_opt("chan", 0.0),
        ],
        |_, ctx| Box::new(Warp1::new(ctx.rand())),
    )),
    desc_draws(plain(
        "GrainTap",
        &[
            inp("bufnum", 0.0),
            inp("grain_dur", 0.2),
            inp("pch_ratio", 1.0),
            inp("pch_dispersion", 0.0),
            inp("time_dispersion", 0.0),
            inp("overlap", 2.0),
        ],
        |_, ctx| Box::new(GrainTap::new(ctx.rand())),
    )),
];
