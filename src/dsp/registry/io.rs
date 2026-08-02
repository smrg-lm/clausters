//! Audio-bus I/O.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- audio-bus I/O (audio rate only; carries a bus role) ---
    desc(
        "In",
        Fixed(1),
        I_BUS,
        Ar,
        R_AR,
        Normal,
        Read,
        false,
        |_, _| Box::new(In),
    ),
    desc(
        "InCtl",
        Fixed(1),
        I_BUS,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(InCtl),
    ),
    desc(
        "OutCtl",
        Fixed(2),
        I_BUS_SIGNAL,
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OutCtl),
    ),
    desc(
        "Out",
        Fixed(2),
        I_BUS_SIGNAL,
        Ar,
        R_AR,
        Normal,
        Write,
        false,
        |_, _| Box::new(Out),
    ),
    desc(
        "ReplaceOut",
        Fixed(2),
        I_BUS_SIGNAL,
        Ar,
        R_AR,
        Normal,
        ReadWrite,
        false,
        |_, _| Box::new(ReplaceOut),
    ),
];
