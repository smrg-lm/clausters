//! The one-pole smoothers, also inserted by lagged controls.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- one-pole smoothers (also inserted by lagged controls) ---
    desc(
        "Lag",
        Fixed(2),
        &[inp("signal", 0.0), inp("time", 0.1)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Lag::new()),
    ),
    desc(
        "VarLag",
        Fixed(3),
        &[inp("signal", 0.0), inp("up", 0.1), inp("down", 0.1)],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(VarLag::new()),
    ),
];
