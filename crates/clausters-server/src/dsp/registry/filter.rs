//! The filter core: one state-variable topology behind every name.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- the filter core: one topology-preserving state-variable
    //     implementation behind every two-pole name, plus the one-pole family.
    //     `BPF` and `Resonz` are the same row twice on purpose - scsynth ships
    //     two historically distinct resonators with the same parameterization
    //     and the same unity peak gain. See `dsp::filter`. ---
    desc(
        "LPF",
        Fixed(2),
        I_FILT,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Lp)),
    ),
    desc(
        "HPF",
        Fixed(2),
        I_FILT,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Hp)),
    ),
    desc(
        "RLPF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::RLp)),
    ),
    desc(
        "RHPF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::RHp)),
    ),
    desc(
        "BPF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Bp)),
    ),
    desc(
        "Resonz",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Resonz)),
    ),
    desc(
        "BRF",
        Fixed(3),
        I_FILT_RQ,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Notch)),
    ),
    desc(
        "Svf",
        Fixed(6),
        &[
            inp("signal", 0.0),
            inp("freq", 440.0),
            inp("rq", 1.0),
            inp("low", 1.0),
            inp_opt("band", 0.0),
            inp_opt("high", 0.0),
        ],
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(Svf::new(SvfMode::Mix)),
    ),
    desc(
        "OnePole",
        Fixed(2),
        I_ONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::OnePole)),
    ),
    desc(
        "OneZero",
        Fixed(2),
        I_ONE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::OneZero)),
    ),
    desc(
        "LeakDC",
        Fixed(2),
        I_LEAK,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::LeakDc)),
    ),
    desc(
        "Integrator",
        Fixed(2),
        I_INTEGRATE,
        Ar,
        R_KR_AR,
        Normal,
        BusRole::None,
        false,
        |_, _| Box::new(OneFilter::new(OneKind::Integrator)),
    ),
    // --- scsynth's small sections: the fixed FIRs, the coefficient-input
    //     sections, and the shapers of a signal's movement ---
    plain(
        "Delay1",
        &[inp("signal", 0.0), inp_opt("x1", 0.0)],
        |_, _| Box::new(SampleDelay::new(false)),
    ),
    plain(
        "Delay2",
        &[inp("signal", 0.0), inp_opt("x1", 0.0), inp_opt("x2", 0.0)],
        |_, _| Box::new(SampleDelay::new(true)),
    ),
    plain("LPZ1", I_SIGNAL, |_, _| Box::new(Fir::new(FirKind::Lpz1))),
    plain("HPZ1", I_SIGNAL, |_, _| Box::new(Fir::new(FirKind::Hpz1))),
    plain("LPZ2", I_SIGNAL, |_, _| Box::new(Fir::new(FirKind::Lpz2))),
    plain("HPZ2", I_SIGNAL, |_, _| Box::new(Fir::new(FirKind::Hpz2))),
    plain("BPZ2", I_SIGNAL, |_, _| Box::new(Fir::new(FirKind::Bpz2))),
    plain("BRZ2", I_SIGNAL, |_, _| Box::new(Fir::new(FirKind::Brz2))),
    plain("Slope", I_SIGNAL, |_, _| Box::new(Fir::new(FirKind::Slope))),
    plain("Flip", I_SIGNAL, |_, _| Box::new(Flip::default())),
    plain("TwoZero", I_RADIUS, |_, _| Box::new(TwoZero::default())),
    plain(
        "FOS",
        &[
            inp("signal", 0.0),
            inp("a0", 0.0),
            inp("a1", 0.0),
            inp("b1", 0.0),
        ],
        |_, _| Box::new(Fos::default()),
    ),
    plain(
        "SOS",
        &[
            inp("signal", 0.0),
            inp("a0", 0.0),
            inp("a1", 0.0),
            inp("a2", 0.0),
            inp("b1", 0.0),
            inp("b2", 0.0),
        ],
        |_, _| Box::new(Sos::default()),
    ),
    plain("Ramp", &[inp("signal", 0.0), inp("time", 0.1)], |_, _| {
        Box::new(LineRamp::default())
    }),
    plain(
        "Slew",
        &[inp("signal", 0.0), inp("up", 1.0), inp("down", 1.0)],
        |_, _| Box::new(Slew::default()),
    ),
    // --- scsynth's two-pole sections with named parameters, realized as the
    //     rows above are ---
    plain("TwoPole", I_RADIUS, |_, _| {
        Box::new(Svf::new(SvfMode::TwoPole))
    }),
    plain("APF", I_RADIUS, |_, _| Box::new(Svf::new(SvfMode::Allpass))),
    plain(
        "Ringz",
        &[
            inp("signal", 0.0),
            inp("freq", 440.0),
            inp("decaytime", 1.0),
        ],
        |_, _| Box::new(Svf::new(SvfMode::Ringz)),
    ),
    plain(
        "Formlet",
        &[
            inp("signal", 0.0),
            inp("freq", 440.0),
            inp("attacktime", 1.0),
            inp("decaytime", 1.0),
        ],
        |_, _| Box::new(Formlet::default()),
    ),
    plain(
        "MidEQ",
        &[
            inp("signal", 0.0),
            inp("freq", 440.0),
            inp("rq", 1.0),
            inp("db", 0.0),
        ],
        |_, _| Box::new(Svf::new(SvfMode::MidEq)),
    ),
    plain("BLowPass", I_B_RQ, |_, _| {
        Box::new(Svf::new(SvfMode::BLowPass))
    }),
    plain("BHiPass", I_B_RQ, |_, _| {
        Box::new(Svf::new(SvfMode::BHiPass))
    }),
    plain("BAllPass", I_B_RQ, |_, _| {
        Box::new(Svf::new(SvfMode::BAllPass))
    }),
    plain("BBandPass", I_B_BW, |_, _| {
        Box::new(Svf::new(SvfMode::BBandPass))
    }),
    plain("BBandStop", I_B_BW, |_, _| {
        Box::new(Svf::new(SvfMode::BBandStop))
    }),
    plain(
        "BPeakEQ",
        &[
            inp("signal", 0.0),
            inp("freq", 1200.0),
            inp("rq", 1.0),
            inp("db", 0.0),
        ],
        |_, _| Box::new(Svf::new(SvfMode::BPeakEq)),
    ),
    plain("BLowShelf", I_B_SHELF, |_, _| {
        Box::new(Svf::new(SvfMode::BLowShelf))
    }),
    plain("BHiShelf", I_B_SHELF, |_, _| {
        Box::new(Svf::new(SvfMode::BHiShelf))
    }),
    // A bank of fixed resonators: the input, the scales, then (freq, amp,
    // ring time) per partial.
    desc(
        "Klank",
        Variadic,
        &[
            inp("signal", 0.0),
            inp("freqscale", 1.0),
            inp("freqoffset", 0.0),
            inp("decayscale", 1.0),
        ],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(Klank::new(c.inputs)),
    ),
];

const I_RADIUS: &[UGenInput] = &[inp("signal", 0.0), inp("freq", 440.0), inp("radius", 0.8)];
const I_B_RQ: &[UGenInput] = &[inp("signal", 0.0), inp("freq", 1200.0), inp("rq", 1.0)];
const I_B_BW: &[UGenInput] = &[inp("signal", 0.0), inp("freq", 1200.0), inp("bw", 1.0)];
const I_B_SHELF: &[UGenInput] = &[
    inp("signal", 0.0),
    inp("freq", 1200.0),
    inp("rs", 1.0),
    inp("db", 0.0),
];
