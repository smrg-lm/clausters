//! The **signal element**: one element for every view of a signal, whatever
//! shape the signal arrives in and whatever the view lets you do with it.
//!
//! The catalog grew these one at a time — `waveform`, `plot`, `scope`,
//! `spectrum`, `spectrogram`, `phasescope` — and ended up spelling one idea six
//! ways. They differ along exactly three axes, and nothing else:
//!
//! - **Presentation** ([`Presentation`]) — what is drawn: the signal against
//!   time, its magnitude spectrum against frequency, its time-frequency
//!   distribution, or the phase relationship of a stereo pair.
//! - **Source** ([`Source`]) — where the samples come from, and in which of the
//!   arrangement layer's two states: **random-access** (a buffer, a file, an
//!   inline array — addressable, so it can be navigated and sliced) or
//!   **forward-only** (a bus at a rate — the newest window, and no addressable
//!   past).
//! - **Capabilities** ([`Caps`]) — what the view lets you do with it: navigate
//!   it, select in it, edit it.
//!
//! Each of the six is a point in that product, and the points nobody built are
//! reachable without new machinery: a navigable live spectrum is a forward-only
//! source whose *frequency* axis is addressable already, and a retained
//! waterfall is a forward-only source with a retention policy on its time axis.
//!
//! **Presentation picks the renderer, and a renderer belongs to the window, not
//! to the element.** The columns and the time-frequency texture go on the
//! window's GPU pipelines; the curves go into its triangle mesh ([`trace`]).
//! Which one draws a signal is therefore invisible to composition — an element
//! costs a GPU slot only when it is a *navigable* heavy view
//! ([`SignalElement::is_gpu_view`]), so a multitrack of clip bodies costs none.
//!
//! The wire still names the six (`type: "waveform"`, `type: "scope"`, …), each
//! a **preset** of this element — [`preset`] is that whole compatibility layer.
//! Moving the wire onto the model itself is a separate step, because it is the
//! breaking half.

pub mod trace;

use std::path::PathBuf;
use std::sync::Arc;

use crate::spectrogram::FreqScale;

use super::widget::{EditorProps, Rate, Ruler, RulerY};

/// The default peak-pyramid bucket of an element whose props name none: one
/// level-0 summary per 256 source samples.
pub const DEFAULT_BASE_BUCKET: usize = 256;

/// What a signal element draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presentation {
    /// Value against time — the trace, whether navigable (the heavy waveform),
    /// static (a plot) or a triggered live window (a scope).
    #[default]
    Signal,
    /// Magnitude against frequency, in dB — one analysis of a stored signal, or
    /// one per frame of a live one.
    Spectrum,
    /// Magnitude against time *and* frequency — the STFT, drawn as a texture.
    TimeFrequency,
    /// The phase relationship of a stereo pair (the rotated Lissajous figure).
    Phase,
}

impl Presentation {
    /// Whether this presentation has a GPU renderer at all — the two that
    /// resolve a whole buffer against a navigable time axis. Whether a given
    /// element *takes* it is [`SignalElement::is_gpu_view`].
    pub fn is_heavy(self) -> bool {
        matches!(self, Presentation::Signal | Presentation::TimeFrequency)
    }
}

/// A **random-access** source: samples the host can address, so an element over
/// it can navigate, slice and (later) edit. The four ways they arrive are tried
/// in precedence order — `cache`, `path`, `buffer`, then whatever came inline —
/// and the first three fill `samples` (or a pyramid) when the resource is
/// mapped or fetched.
#[derive(Debug, Clone)]
pub struct Data {
    pub samples: Arc<[f32]>,
    pub channels: usize,
    pub buffer: Option<i32>,
    pub path: Option<PathBuf>,
    pub cache: Option<PathBuf>,
    /// The peak pyramid's level-0 bucket, in samples.
    pub base_bucket: usize,
}

/// A **forward-only** source: `channels` adjacent buses read live at `rate`.
/// There is no addressable past — the ring holds the newest window — so what
/// the element sees is a policy: how long a window (`window_ms`), where it is
/// aligned (`trigger`, a rising crossing in the first channel), and whether it
/// is frozen (`hold`).
#[derive(Debug, Clone)]
pub struct Bus {
    pub bus: i32,
    pub rate: Rate,
    pub channels: usize,
    pub window_ms: f32,
    pub trigger: f32,
    pub hold: bool,
}

/// Where a signal element's samples come from — the arrangement layer's own
/// generated/generator split seen from the view: a rendered thing that can be
/// read backwards and sliced, or a running one that can only be watched.
#[derive(Debug, Clone)]
pub enum Source {
    Data(Data),
    Bus(Bus),
}

impl Source {
    /// The channel count the source presents, whichever kind it is.
    pub fn channels(&self) -> usize {
        match self {
            Source::Data(d) => d.channels.max(1),
            Source::Bus(b) => b.channels.max(1),
        }
    }

    /// The random-access half, if this is one.
    pub fn data(&self) -> Option<&Data> {
        match self {
            Source::Data(d) => Some(d),
            Source::Bus(_) => None,
        }
    }

    /// The random-access half, mutably — a bulk load lands its samples here.
    pub fn data_mut(&mut self) -> Option<&mut Data> {
        match self {
            Source::Data(d) => Some(d),
            Source::Bus(_) => None,
        }
    }

    /// The forward-only half, if this is one.
    pub fn bus(&self) -> Option<&Bus> {
        match self {
            Source::Bus(b) => Some(b),
            Source::Data(_) => None,
        }
    }

    /// The forward-only half, mutably (a `/gui_set bus` writes through here).
    pub fn bus_mut(&mut self) -> Option<&mut Bus> {
        match self {
            Source::Bus(b) => Some(b),
            Source::Data(_) => None,
        }
    }
}

/// What the view lets the user do. Capabilities are a property of the element,
/// not of its source: a plot *is* a signal element over random-access data with
/// all three off, which is what it always was.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caps {
    /// Zooms and pans its axes, and joins the window's navigation group.
    pub navigable: bool,
    /// Carries a selection (and a playhead over it).
    pub selectable: bool,
    /// Edits its data back to whoever owns it.
    pub editable: bool,
}

/// The value axis of the presentations that measure one. Either side omitted
/// auto-fits to the data — the arbitrary-range sequence case; the live views
/// name both, so nothing moves under them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueRange {
    pub min: Option<f32>,
    pub max: Option<f32>,
}

impl ValueRange {
    /// The range with both sides given.
    pub fn new(min: f32, max: f32) -> Self {
        ValueRange {
            min: Some(min),
            max: Some(max),
        }
    }

    /// Both sides auto-fitted to the data.
    pub fn auto() -> Self {
        ValueRange {
            min: None,
            max: None,
        }
    }
}

/// The spectral parameters, shared by every presentation that runs an FFT — the
/// live curve, the stored one and the time-frequency texture. Only `fft_size`
/// and `hop` are analysis inputs; the rest are display controls, which is why
/// the texture can change all of them for the cost of a uniform write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spectral {
    /// The analysis window, in samples (a supported power of two). The
    /// time-frequency presentation names it `window_size` on the wire.
    pub fft_size: usize,
    /// The time-frequency hop, in samples.
    pub hop: usize,
    pub db_floor: f32,
    pub db_ceil: f32,
    pub freq_scale: FreqScale,
    /// Exponential per-bin smoothing of a live curve, in `[0, 0.99]`.
    pub averaging: f32,
    /// A slowly decaying peak trace per channel, over a live curve.
    pub peak_hold: bool,
    /// The time-frequency colormap index.
    pub colormap: i32,
}

impl Default for Spectral {
    fn default() -> Self {
        Spectral {
            fft_size: 2048,
            hop: 1024,
            db_floor: -100.0,
            db_ceil: 0.0,
            freq_scale: FreqScale::Log,
            averaging: 0.5,
            peak_hold: false,
            colormap: 0,
        }
    }
}

/// The chrome that no presentation owns: how the channels are laid out, and the
/// element's name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Display {
    /// Channels share one field as color-coded traces instead of stacking into
    /// lanes.
    pub overlay: bool,
    pub label: Option<String>,
}

/// One view of one signal: a presentation of a source, what the user may do
/// with it, and the parameters that presentation reads.
///
/// The axis chrome — both ruler strips, the sample rate, the selection and the
/// playhead — lives in [`EditorProps`] for *every* presentation, not only the
/// navigable ones: the strips a live scope shows are the same two strips, and
/// keeping one place for them is what lets one element be swapped for another
/// without the chrome moving.
#[derive(Debug, Clone)]
pub struct SignalElement {
    pub presentation: Presentation,
    pub source: Source,
    pub caps: Caps,
    pub value: ValueRange,
    pub spectral: Spectral,
    pub display: Display,
    pub editor: EditorProps,
    /// The cached spectral analysis of a **stored** [`Presentation::Spectrum`]
    /// — recomputed at the element's mutation points (parse, a bulk load
    /// landing samples, a `/gui_set` touching what it reads), never per frame.
    pub analysis: Option<Arc<super::plot::PlotSpectrum>>,
}

impl SignalElement {
    /// A bare element at the preset's configuration — the base the wire's props
    /// are then read over.
    pub fn from_preset(p: &Preset) -> SignalElement {
        let mut editor = EditorProps::parse(&serde_json::Map::new(), p.ruler_y);
        editor.ruler = p.ruler;
        SignalElement {
            presentation: p.presentation,
            source: if p.live {
                Source::Bus(Bus {
                    bus: 0,
                    rate: Rate::Audio,
                    channels: 1,
                    window_ms: p.window_ms,
                    trigger: 0.0,
                    hold: false,
                })
            } else {
                Source::Data(Data {
                    samples: Arc::from([] as [f32; 0]),
                    channels: 1,
                    buffer: None,
                    path: None,
                    cache: None,
                    base_bucket: DEFAULT_BASE_BUCKET,
                })
            },
            caps: p.caps,
            value: p.value,
            spectral: p.spectral,
            display: Display::default(),
            editor,
            analysis: None,
        }
    }

    /// How many channels the element draws.
    pub fn channels(&self) -> usize {
        self.source.channels()
    }

    /// The element's name, when it carries one.
    pub fn label(&self) -> Option<&str> {
        self.display.label.as_deref()
    }

    /// Whether the element owns a GPU slot: a navigable heavy presentation.
    /// Everything else — a plot, a live curve, a clip's body — draws into the
    /// window's shared mesh.
    pub fn is_gpu_view(&self) -> bool {
        self.caps.navigable && self.presentation.is_heavy()
    }

    /// Whether the element reads its samples live (a forward-only source).
    pub fn is_live(&self) -> bool {
        matches!(self.source, Source::Bus(_))
    }

    /// Recomputes [`Self::analysis`] from the current samples and parameters. A
    /// no-op unless this is a stored-signal spectrum with samples to analyze.
    pub fn refresh_analysis(&mut self) {
        let stored_spectrum = self.presentation == Presentation::Spectrum;
        let Some(data) = self.source.data() else {
            self.analysis = None;
            return;
        };
        self.analysis = (stored_spectrum && !data.samples.is_empty()).then(|| {
            Arc::new(super::plot::analyze(
                &data.samples,
                data.channels,
                self.spectral.fft_size,
                self.editor.sample_rate,
            ))
        });
    }
}

/// A wire type name as a point in the element's product — its presentation,
/// source kind and capabilities, plus the defaults that name carries.
///
/// This table is the whole of the compatibility layer: the six names the
/// catalog grew are configurations of one element, so a script that says
/// `type: "scope"` gets the forward-only triggered signal view it always got.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub presentation: Presentation,
    /// Whether the source is forward-only (a bus) rather than random-access.
    pub live: bool,
    pub caps: Caps,
    /// The value axis when the props name neither side.
    pub value: ValueRange,
    pub spectral: Spectral,
    /// The default x strip.
    pub ruler: Ruler,
    /// The default y strip — the unit this presentation measures in.
    pub ruler_y: RulerY,
    /// The default live window, in milliseconds (forward-only sources only).
    pub window_ms: f32,
    /// The wire name of the analysis size: the spectral views say `fft_size`,
    /// the time-frequency one says `window_size`.
    pub size_prop: &'static str,
    /// Whether the name reads a `view` prop to pick its presentation — the
    /// static plot's `signal`/`spectrum` switch.
    pub view_prop: bool,
}

/// Navigable and selectable: the editor-grade views over addressable data.
const NAV: Caps = Caps {
    navigable: true,
    selectable: true,
    editable: false,
};
/// A view that only shows: a plot, and every live one.
const WATCH: Caps = Caps {
    navigable: false,
    selectable: false,
    editable: false,
};

/// The preset a wire type name configures, or `None` when the name is not a
/// signal element.
pub fn preset(kind: &str) -> Option<Preset> {
    let base = Preset {
        presentation: Presentation::Signal,
        live: false,
        caps: WATCH,
        value: ValueRange::new(-1.0, 1.0),
        spectral: Spectral::default(),
        ruler: Ruler::Time,
        ruler_y: RulerY::Norm,
        window_ms: 20.0,
        size_prop: "fft_size",
        view_prop: false,
    };
    Some(match kind {
        "waveform" => Preset { caps: NAV, ..base },
        "spectrogram" => Preset {
            presentation: Presentation::TimeFrequency,
            caps: NAV,
            ruler_y: RulerY::Hz,
            size_prop: "window_size",
            spectral: Spectral {
                fft_size: 1024,
                hop: 512,
                db_floor: -90.0,
                ..Spectral::default()
            },
            ..base
        },
        "plot" => Preset {
            value: ValueRange::auto(),
            view_prop: true,
            ..base
        },
        "scope" => Preset { live: true, ..base },
        "spectrum" => Preset {
            presentation: Presentation::Spectrum,
            live: true,
            ruler_y: RulerY::Db,
            ..base
        },
        "phasescope" => Preset {
            presentation: Presentation::Phase,
            live: true,
            ruler: Ruler::Off,
            ruler_y: RulerY::Off,
            window_ms: 30.0,
            ..base
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six names the catalog grew are six *distinct* points of one product
    /// — which is what makes the table a compatibility layer rather than a list
    /// of aliases.
    #[test]
    fn every_wire_name_is_a_distinct_point_of_the_product() {
        let names = [
            "waveform",
            "spectrogram",
            "plot",
            "scope",
            "spectrum",
            "phasescope",
        ];
        let points: Vec<_> = names
            .iter()
            .map(|n| {
                let p = preset(n).unwrap();
                (p.presentation, p.live, p.caps)
            })
            .collect();
        for (i, a) in points.iter().enumerate() {
            for b in &points[i + 1..] {
                assert_ne!(a, b, "two names configure the same element");
            }
        }
        assert!(preset("slider").is_none());
    }

    /// Only a navigable heavy view costs a GPU slot: a plot, a live curve and a
    /// clip body all draw into the window's mesh.
    #[test]
    fn only_a_navigable_heavy_view_owns_a_slot() {
        let el = |name: &str| SignalElement::from_preset(&preset(name).unwrap());
        assert!(el("waveform").is_gpu_view());
        assert!(el("spectrogram").is_gpu_view());
        assert!(!el("plot").is_gpu_view());
        assert!(!el("scope").is_gpu_view());
        assert!(!el("spectrum").is_gpu_view());
        assert!(!el("phasescope").is_gpu_view());
    }

    /// The two source kinds are the arrangement layer's two states, and the
    /// preset table is where each name lands on one of them.
    #[test]
    fn the_live_names_are_the_forward_only_sources() {
        for name in ["scope", "spectrum", "phasescope"] {
            let el = SignalElement::from_preset(&preset(name).unwrap());
            assert!(el.is_live(), "{name} reads a bus");
            assert!(el.source.data().is_none());
            assert!(el.source.bus().is_some());
        }
        for name in ["waveform", "spectrogram", "plot"] {
            let el = SignalElement::from_preset(&preset(name).unwrap());
            assert!(!el.is_live(), "{name} reads addressable samples");
            assert!(el.source.data().is_some());
        }
    }
}
