//! Analysis: the measures read off an `FFT` chain, the onset and beat
//! trackers, and the pitch follower.
//!
//! One slice of the catalog, in its place in the table order;
//! `super::FAMILIES` concatenates them all.

use super::*;

/// A sink of the chain at control rate, holding its value between frames.
const fn measure(
    name: &'static str,
    inputs: &'static [UGenInput],
    build: fn(&UGenConfig, &BuildCtx) -> Box<dyn UGen>,
) -> UGenDescriptor {
    desc_spectral(
        name,
        Fixed(inputs.len()),
        inputs,
        Kr,
        R_KR,
        SpectralRole::Sink,
        build,
    )
}

pub(super) static UGENS: &[UGenDescriptor] = &[
    // --- the measures of a frame (`dsp::features`). ---
    measure("SpecCentroid", I_CHAIN, |c, b| {
        Box::new(SpecCentroid::new(c, b.sample_rate))
    }),
    measure("SpecFlatness", I_CHAIN, |c, _| {
        Box::new(SpecFlatness::new(c))
    }),
    measure(
        "SpecPcile",
        &[
            inp("chain", 0.0),
            inp_opt("fraction", 0.5),
            inp_opt("interpolate", 0.0),
            inp_opt("binout", 0.0),
        ],
        |c, b| Box::new(SpecPcile::new(c, b.sample_rate)),
    ),
    measure(
        "Loudness",
        &[
            inp("chain", 0.0),
            inp_opt("smask", 0.25),
            inp_opt("tmask", 1.0),
        ],
        |c, b| Box::new(Loudness::new(c, b.sample_rate)),
    ),
    // One row per coefficient; `numcoeff` and `chan` are read at build.
    measure(
        "MFCC",
        &[
            inp("chain", 0.0),
            inp_opt("numcoeff", 13.0),
            inp_opt("chan", 0.0),
        ],
        |c, b| Box::new(Mfcc::new(c, b.sample_rate)),
    ),
    measure(
        "KeyTrack",
        &[
            inp("chain", 0.0),
            inp_opt("keydecay", 2.0),
            inp_opt("chromaleak", 0.5),
        ],
        |c, b| Box::new(KeyTrack::new(c, b.sample_rate)),
    ),
    // --- onsets (`dsp::onsets`). `odftype` and `medianspan` are read at
    //     build. ---
    measure(
        "Onsets",
        &[
            inp("chain", 0.0),
            inp_opt("threshold", 0.5),
            inp_opt("odftype", 3.0),
            inp_opt("relaxtime", 1.0),
            inp_opt("floor", 0.1),
            inp_opt("mingap", 10.0),
            inp_opt("medianspan", 11.0),
            inp_opt("whtype", 1.0),
            inp_opt("rawodf", 0.0),
        ],
        |c, b| Box::new(Onsets::new(c, b.sample_rate)),
    ),
    desc_spectral(
        "PV_HainsworthFoote",
        Fixed(5),
        &[
            inp("chain", 0.0),
            inp_opt("proph", 0.0),
            inp_opt("propf", 0.0),
            inp_opt("threshold", 1.0),
            inp_opt("waittime", 0.04),
        ],
        Ar,
        R_AR,
        SpectralRole::Sink,
        |c, b| Box::new(PvOnset::new(c, b.sample_rate, OnsetKind::HainsworthFoote)),
    ),
    desc_spectral(
        "PV_JensenAndersen",
        Fixed(7),
        &[
            inp("chain", 0.0),
            inp_opt("propsc", 0.25),
            inp_opt("prophfe", 0.25),
            inp_opt("prophfc", 0.25),
            inp_opt("propsf", 0.25),
            inp_opt("threshold", 1.0),
            inp_opt("waittime", 0.04),
        ],
        Ar,
        R_AR,
        SpectralRole::Sink,
        |c, b| Box::new(PvOnset::new(c, b.sample_rate, OnsetKind::JensenAndersen)),
    ),
    // --- the beat trackers (`dsp::beat`), a row per output. ---
    measure(
        "BeatTrack",
        &[
            inp("chain", 0.0),
            inp_opt("lock", 0.0),
            inp_opt("chan", 0.0),
        ],
        |c, b| Box::new(BeatTrack::new(c, b.sample_rate)),
    ),
    // It reads `numfeatures` control buses from `busindex`.
    desc(
        "BeatTrack2",
        Fixed(7),
        &[
            inp("busindex", 0.0),
            inp("numfeatures", 1.0),
            inp_opt("windowsize", 2.0),
            inp_opt("phaseaccuracy", 0.02),
            inp_opt("lock", 0.0),
            inp_opt("weightingscheme", -2.1),
            inp_opt("chan", 0.0),
        ],
        Kr,
        R_KR,
        Normal,
        BusRole::ReadControls,
        false,
        |c, b| Box::new(BeatTrack2::new(c, b.sample_rate)),
    ),
    // --- the pitch follower (`dsp::pitch`), a row per output; every input
    //     but the signal is read at build. ---
    desc(
        "Pitch",
        Fixed(12),
        &[
            inp("signal", 0.0),
            inp_opt("init_freq", 440.0),
            inp_opt("min_freq", 60.0),
            inp_opt("max_freq", 4000.0),
            inp_opt("exec_freq", 100.0),
            inp_opt("max_bins_per_octave", 16.0),
            inp_opt("median", 1.0),
            inp_opt("amp_threshold", 0.01),
            inp_opt("peak_threshold", 0.5),
            inp_opt("down_sample", 1.0),
            inp_opt("clar", 0.0),
            inp_opt("chan", 0.0),
        ],
        Kr,
        R_KR,
        Normal,
        BusRole::None,
        false,
        |c, b| {
            let audio = c.audio_inputs.first().copied().unwrap_or(false);
            Box::new(Pitch::new(c, b.sample_rate, audio))
        },
    ),
];
