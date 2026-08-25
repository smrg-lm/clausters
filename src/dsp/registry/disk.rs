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
        |c, _| Box::new(DiskIn::open(c)),
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
        |c, _| Box::new(DiskOut::open(c)),
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
