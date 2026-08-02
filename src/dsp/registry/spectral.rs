//! The frequency-domain chain and partitioned convolution.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- frequency-domain (`fr`) chain: FFT opens a synth-private
    //     spectral chain, PV_* transform it in place, IFFT resynthesises audio.
    //     FFT/PV carry the chain at control rate (a per-block ready marker);
    //     IFFT produces audio. See `dsp::spectral`. ---
    desc_spectral(
        "FFT",
        Fixed(2),
        &[inp("source", 0.0), inp("active", 1.0)],
        Kr,
        R_KR,
        SpectralRole::Source,
        |c, _| Box::new(Fft::new(c)),
    ),
    desc_spectral(
        "IFFT",
        Fixed(1),
        I_CHAIN,
        Ar,
        R_AR,
        SpectralRole::Sink,
        |c, _| Box::new(Ifft::new(c)),
    ),
    desc_spectral(
        "PV_MagAbove",
        Fixed(2),
        I_CHAIN_THRESHOLD,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvMag::new(MagMode::Above)),
    ),
    desc_spectral(
        "PV_MagBelow",
        Fixed(2),
        I_CHAIN_THRESHOLD,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvMag::new(MagMode::Below)),
    ),
    desc_spectral(
        "PV_BrickWall",
        Fixed(2),
        &[inp("chain", 0.0), inp("wipe", 0.0)],
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvBrickWall),
    ),
    // the curated PV set — parameterized implementations under the
    // scsynth-compatible names, deliberately not a one-UGen-per-op catalog
    // (see docs/decisions.md). PvMag gains a clip mode; PvCombine is ONE
    // binary implementation behind six names; the stateful pair and the bin
    // remap allocate their scratch at build (network thread).
    desc_spectral(
        "PV_MagClip",
        Fixed(2),
        I_CHAIN_THRESHOLD,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |_, _| Box::new(PvMag::new(MagMode::Clip)),
    ),
    desc_spectral(
        "PV_Add",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Add)),
    ),
    desc_spectral(
        "PV_Mul",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Mul)),
    ),
    desc_spectral(
        "PV_Min",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Min)),
    ),
    desc_spectral(
        "PV_Max",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::Max)),
    ),
    desc_spectral(
        "PV_MagMul",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::MagMul)),
    ),
    desc_spectral(
        "PV_CopyPhase",
        Fixed(2),
        I_CHAIN_AB,
        Kr,
        R_KR,
        SpectralRole::Filter2,
        |_, _| Box::new(PvCombine::new(CombineOp::CopyPhase)),
    ),
    desc_spectral(
        "PV_MagFreeze",
        Fixed(2),
        &[inp("chain", 0.0), inp("freeze", 0.0)],
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvMagFreeze::new(c)),
    ),
    desc_spectral(
        "PV_MagSmear",
        Fixed(2),
        &[inp("chain", 0.0), inp("bins", 0.0)],
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvMagSmear::new(c)),
    ),
    desc_spectral(
        "PV_BinShift",
        Fixed(3),
        I_CHAIN_SHIFT,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvBinShift::new(c, false)),
    ),
    desc_spectral(
        "PV_MagShift",
        Fixed(3),
        I_CHAIN_SHIFT,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvBinShift::new(c, true)),
    ),
    // the general per-frame mechanism — one UGen interpreting a
    // compile-validated bin-expression program (`mag_expr`/`phase_expr`) over
    // every bin of each fresh frame. Inputs: `[chain, p0, p1, …]` (variadic
    // parameters, sampled at the hop). An op outside the curated set is a
    // program here, never a new registry row (see docs/decisions.md).
    // Variadic: only `chain` is fixed; `p0…` are the program's parameters.
    desc_spectral(
        "PV_Kernel",
        Variadic,
        I_CHAIN,
        Kr,
        R_KR,
        SpectralRole::Filter,
        |c, _| Box::new(PvKernel::new(c)),
    ),
    // --- partitioned convolution: one UGen, kernel spectra prepared
    //     off the RT thread by `/buffer_gen prepare_partconv`, MACs spread across
    //     the hop's blocks for flat load. Not a PV_*: fast convolution's
    //     zero-padded rectangular segments are incompatible with the windowed
    //     COLA chain. See `dsp::conv`. ---
    desc(
        "Conv",
        Fixed(2),
        &[inp("source", 0.0), inp("kernel", 0.0)],
        Ar,
        R_AR,
        Normal,
        BusRole::None,
        false,
        |c, _| Box::new(Conv::new(c)),
    ),
];
