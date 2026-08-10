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
//! The wire says the **point** of the product — `view` (the presentation),
//! the source props (a `bus` is forward-only, anything else addressable) and
//! `navigable` — and [`point`] is where each combination's defaults live. The
//! six names the catalog grew (`waveform`, `scope`, `spectrum`, …) were six
//! points of it, and they are the two clients' builder names now, nothing the
//! host knows about.

pub mod trace;

use std::path::PathBuf;
use std::sync::Arc;

use crate::spectrogram::FreqScale;
use crate::waveform::WaveformData;

use super::widget::{EditorProps, Rate, Ruler, RulerY};

mod body;
mod bulk;
mod chrome;
mod freq;
mod live;
mod slot;
pub use live::LiveState;

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
    /// The `view` prop as a presentation; `None` when the wire names one this
    /// host does not have.
    pub fn parse(name: &str) -> Option<Presentation> {
        Some(match name {
            "trace" => Presentation::Signal,
            "spectrum" => Presentation::Spectrum,
            "spectrogram" => Presentation::TimeFrequency,
            "phase" => Presentation::Phase,
            _ => return None,
        })
    }

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
    /// **Bulk**: resolve this source as a peak pyramid rather than as an array
    /// of samples. It is a property of the source's *size*, not of the drawing
    /// — a take is minutes of audio and reaches the host as a pyramid (a
    /// mapped file, a peaks cache, a fetched buffer), a plotted sequence is a
    /// few thousand values and reaches it whole. Both draw through the same
    /// [`trace::Trace`], which is why this is one flag and not two code paths.
    pub bulk: bool,
    /// The resolved pyramid of a [`bulk`](Self::bulk) source, once a loader has
    /// mapped or fetched it. `None` until then — and always, for a source that
    /// is not bulk, which keeps its samples inline instead.
    pub body: Option<Arc<WaveformData>>,
}

impl Data {
    /// The one column source this data draws through: its resolved pyramid
    /// when it has one, else its inline samples. Every signal drawing — the
    /// GPU waveform, a plot, a clip's take — reads its columns from here.
    pub fn trace(&self) -> trace::Trace<'_> {
        match &self.body {
            Some(d) => trace::Trace::Data(d),
            None => trace::Trace::samples(&self.samples, self.channels),
        }
    }

    /// Whether the source has nothing to draw yet — no pyramid and no samples.
    pub fn is_empty(&self) -> bool {
        self.body.is_none() && self.samples.is_empty()
    }
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
    /// **The retention policy on this source's time axis**, in seconds of
    /// history (0 = none, the default).
    ///
    /// A forward-only source has no addressable past, which is what stops it
    /// being navigable: there is nothing behind the newest window to zoom out
    /// to. Retention is what supplies one — the host keeps this many seconds of
    /// the bus and the view reads *that*, so a span the axis declares is a span
    /// the axis can be navigated over. It is a policy of the **axis**, not of
    /// the drawing: the same seconds mean the same seconds whatever the frame
    /// rate, the FFT size or the hop, and a `/gui_set` of it resizes the
    /// history live.
    pub retention: f32,
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

    /// The range with each auto side filled in from `(lo, hi)` — for a drawing
    /// that has no data pass of its own to auto-fit against.
    pub fn resolved(&self, lo: f32, hi: f32) -> (f32, f32) {
        (self.min.unwrap_or(lo), self.max.unwrap_or(hi))
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
    /// What a **live** presentation has accumulated from its forward-only
    /// source: the rolling history, the triggered window, the analysis states,
    /// the rolling transform. Advanced once per tick ([`Self::tick`]) and only
    /// drawn afterwards, so a repaint never advances anything.
    pub live: LiveState,
    /// Whether the element's **claimed GPU slot** has content it has not handed
    /// the frame yet ([`Self::fill`]). True on a fresh element and at every
    /// mutation point that changes what the slot would be built from; cleared
    /// when the fill is taken, which is what keeps a still picture at zero
    /// uploads.
    pub slot_dirty: bool,
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
                    retention: 0.0,
                })
            } else {
                Source::Data(Data {
                    samples: Arc::from([] as [f32; 0]),
                    channels: 1,
                    buffer: None,
                    path: None,
                    cache: None,
                    base_bucket: DEFAULT_BASE_BUCKET,
                    bulk: p.bulk,
                    body: None,
                })
            },
            caps: p.caps,
            value: p.value,
            spectral: p.spectral,
            display: Display::default(),
            editor,
            analysis: None,
            live: LiveState::default(),
            slot_dirty: true,
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
    /// Everything else — a plot, a live curve, a clip's take — draws into the
    /// window's shared mesh.
    pub fn is_gpu_view(&self) -> bool {
        self.caps.navigable && self.presentation.is_heavy()
    }

    /// Whether the element's picture *is* a texture — the time-frequency
    /// presentation, one sample of an uploaded STFT per pixel. The trace has
    /// two drawings of one signal (the GPU pipeline when it navigates, the mesh
    /// when it is a clip's take); this one has a single drawing, so a clip
    /// carrying it needs the slot a navigable view would have.
    pub fn is_texture_view(&self) -> bool {
        self.presentation == Presentation::TimeFrequency
    }

    /// Whether the element needs a GPU slot of its own, under whatever id
    /// addresses it (a clip's body is addressed by the clip).
    pub fn needs_gpu_slot(&self) -> bool {
        self.is_gpu_view() || (self.is_texture_view() && !self.is_live())
    }

    /// Whether the element reads its samples live (a forward-only source).
    pub fn is_live(&self) -> bool {
        matches!(self.source, Source::Bus(_))
    }

    /// How many lanes this element stacks out of the `uploaded` channels the
    /// front found in its slot: one when the channels are **overlaid**, however
    /// many there are, else one per channel.
    pub fn lanes(&self, uploaded: usize) -> usize {
        if self.display.overlay {
            1
        } else {
            uploaded.max(1)
        }
    }

    /// Whether a y zoom over this element anchors at the centre of a lane: a
    /// trace's axis measures **amplitude**, which is centred on zero in every
    /// lane. A spectrogram's measures frequency, which is a value under the
    /// pointer.
    pub fn centres_y_zoom(&self) -> bool {
        self.presentation == Presentation::Signal
    }

    /// Whether the element navigates a **frequency** x axis of its own: a
    /// navigable spectrum, the one presentation whose horizontal domain is not
    /// the window's time.
    ///
    /// Such an axis is addressable with no retention at all — every bin is
    /// there every frame — so navigating it needs no history and no navigation
    /// group: it is one normalized window the element carries alone
    /// ([`EditorProps::x_view`]), exactly as the vertical window of a
    /// spectrogram's frequency axis already is. Nothing else in a window
    /// measures in hertz along x, so there is no axis to share.
    pub fn navigates_freq(&self) -> bool {
        self.caps.navigable && self.presentation == Presentation::Spectrum
    }

    /// Whether the element navigates the window's shared **time** axis — the
    /// capability that joins a navigation group, which is every navigable
    /// presentation but the spectrum.
    pub fn navigates_time(&self) -> bool {
        self.caps.navigable && !self.navigates_freq()
    }

    /// The rate this element's frequency axis is placed by: its own when it
    /// names one — a stored analysis carries the rate of what it analyzed —
    /// else the server's, which is what a live tap is running at.
    pub fn freq_rate(&self, server_rate: f64) -> f64 {
        if self.editor.sample_rate > 0.0 {
            self.editor.sample_rate
        } else {
            server_rate
        }
    }

    /// **The frequency window this element can actually show**: the window that
    /// was asked for ([`EditorProps::x_view`]), opened up wherever it is finer
    /// than the analysis behind it resolves.
    ///
    /// The two are deliberately kept apart. What is *stored* is the request —
    /// the reader's last zoom, or a script's `/gui_set` — and the floor is a
    /// function of where the window sits: on a log axis a window narrow enough
    /// at 12 kHz cannot exist at 100 Hz, where four bins already span a quarter
    /// of the axis. Were the opening written back, a pan down the axis would
    /// spend the zoom on its way and the pan back up would arrive somewhere the
    /// reader never asked to be. Kept apart, the axis opens where it must and
    /// closes again as soon as there is room, so a gesture undoes itself.
    ///
    /// This is the window the frame draws, the gesture anchors in, and the
    /// `"view_x"` event reports — everything but what is written down.
    pub fn freq_window(&self, server_rate: f64) -> (f64, f64) {
        let (start, len) = self.editor.x_view();
        self.freq_window_of(server_rate, start, len)
    }

    /// [`Self::freq_window`] of a request that has not been written yet: what
    /// the element would be showing had it been. A gesture asks this before
    /// writing, so a request that changes nothing on the screen can be left
    /// unwritten rather than overwriting the one the reader is still using.
    pub fn freq_window_of(&self, server_rate: f64, start: f64, len: f64) -> (f64, f64) {
        // Only over a frequency axis: the same pair on a stored waveform is a
        // window over time, which no FFT bin has anything to say about.
        if self.presentation != Presentation::Spectrum {
            return (start, len);
        }
        let (nyquist, f_lo_norm) = super::spectrum::axis_geometry(self.freq_rate(server_rate));
        let floor = super::spectrum::min_display_span(
            self.spectral.fft_size,
            nyquist * 2.0,
            self.spectral.freq_scale,
            f_lo_norm,
            start,
        );
        // Opened at the left edge, which is where the floor was measured from;
        // only a window that then runs off the top is pulled back onto the axis.
        let len = len.max(floor).min(1.0);
        (start.min(1.0 - len).max(0.0), len)
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

/// One point of the element's product — a presentation over a source kind,
/// with the capabilities and the defaults that combination carries.
///
/// The wire names the point directly (`view`, the source props, `navigable`),
/// so this is the table of what each point *defaults* to, not a table of
/// names: it is where a triggered live trace gets its 20 ms window and a
/// spectrogram its 1024-sample analysis.
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
    /// Whether a random-access source of this preset is [`Data::bulk`] — the
    /// navigable heavy views, whose sources are takes rather than sequences.
    pub bulk: bool,
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

/// A spectrum's capabilities: navigating its frequency axis, and nothing else.
/// Selecting on it would be a span of hertz with no source to slice, and
/// editing a magnitude is not a thing this model expresses.
fn freq_nav(navigable: bool) -> Caps {
    Caps { navigable, ..WATCH }
}

/// The point of the product a wire node describes: its `view` (the
/// presentation), whether its source is forward-only, and whether it
/// navigates.
///
/// `navigable` separates the two addressable traces, and it separates more
/// than a capability: a navigating view resolves its source as a **take** —
/// through the peak pyramid, never as an array of samples — and pins its
/// value axis, while a still one holds the sequence itself and auto-fits an
/// axis nobody named. That is why it is an argument here rather than a flag
/// applied afterwards.
pub fn point(view: Presentation, live: bool, navigable: bool) -> Preset {
    let base = Preset {
        presentation: view,
        live,
        caps: WATCH,
        value: ValueRange::new(-1.0, 1.0),
        spectral: Spectral::default(),
        ruler: Ruler::Time,
        ruler_y: RulerY::Norm,
        window_ms: 20.0,
        size_prop: "fft_size",
        bulk: false,
    };
    match (view, live) {
        // The time-frequency texture and the goniometer read the same over
        // either source kind: neither has a still variant to tell apart.
        (Presentation::TimeFrequency, _) => Preset {
            caps: if navigable { NAV } else { WATCH },
            bulk: !live,
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
        (Presentation::Phase, _) => Preset {
            ruler: Ruler::Off,
            ruler_y: RulerY::Off,
            window_ms: 30.0,
            ..base
        },
        // The spectrum navigates **frequency**, so `navigable` costs it no
        // history and no group — but not a selection either: a span of hertz is
        // not something the model can hand back to a source that has no such
        // axis to slice.
        (Presentation::Spectrum, true) => Preset {
            caps: freq_nav(navigable),
            ruler_y: RulerY::Db,
            ..base
        },
        // A stored spectrum measures an arbitrary sequence, so its value axis
        // auto-fits like any still view's.
        (Presentation::Spectrum, false) => Preset {
            caps: freq_nav(navigable),
            value: ValueRange::auto(),
            ..base
        },
        // A live trace is the triggered window; a still one is the whole
        // sequence over an auto-fitted axis; a navigating one is the take.
        (Presentation::Signal, true) => base,
        (Presentation::Signal, false) if navigable => Preset {
            caps: NAV,
            bulk: true,
            ..base
        },
        (Presentation::Signal, false) => Preset {
            value: ValueRange::auto(),
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six views the catalog named separately are six **distinct** points
    /// of one product — which is why the wire says the point and no table of
    /// names is left to keep in step with it.
    #[test]
    fn the_six_views_are_distinct_points_of_the_product() {
        let points = [
            point(Presentation::Signal, false, true), // the heavy waveform
            point(Presentation::Signal, false, false), // the static plot
            point(Presentation::Signal, true, false), // the oscilloscope
            point(Presentation::Spectrum, true, false), // the spectroscope
            point(Presentation::TimeFrequency, false, true), // the spectrogram
            point(Presentation::Phase, true, false),  // the goniometer
        ];
        let shape = |p: &Preset| (p.presentation, p.live, p.caps, p.bulk, p.value);
        for (i, a) in points.iter().enumerate() {
            for b in &points[i + 1..] {
                assert_ne!(shape(a), shape(b), "two points configure one element");
            }
        }
    }

    /// Only a navigable heavy view costs a GPU slot: a plot, a live curve and
    /// a clip body all draw into the window's mesh.
    #[test]
    fn only_a_navigable_heavy_view_owns_a_slot() {
        let el = |v, live, nav| SignalElement::from_preset(&point(v, live, nav));
        assert!(el(Presentation::Signal, false, true).is_gpu_view());
        assert!(el(Presentation::TimeFrequency, false, true).is_gpu_view());
        assert!(!el(Presentation::Signal, false, false).is_gpu_view());
        assert!(!el(Presentation::Signal, true, false).is_gpu_view());
        assert!(!el(Presentation::Spectrum, true, false).is_gpu_view());
        assert!(!el(Presentation::Phase, true, false).is_gpu_view());
    }

    /// The two source kinds are the arrangement layer's two states, and the
    /// wire's `bus` is what puts an element on one of them.
    #[test]
    fn a_live_point_reads_a_bus_and_a_stored_one_reads_samples() {
        for view in [
            Presentation::Signal,
            Presentation::Spectrum,
            Presentation::Phase,
        ] {
            let el = SignalElement::from_preset(&point(view, true, false));
            assert!(el.is_live(), "{view:?} over a bus reads forward-only");
            assert!(el.source.data().is_none());
            assert!(el.source.bus().is_some());
        }
        for view in [Presentation::Signal, Presentation::TimeFrequency] {
            let el = SignalElement::from_preset(&point(view, false, true));
            assert!(!el.is_live(), "{view:?} over samples is addressable");
            assert!(el.source.data().is_some());
        }
    }

    /// A still trace is not a navigable one with a flag off: it holds the
    /// sequence itself rather than a take, and auto-fits an axis nobody named.
    #[test]
    fn a_still_trace_is_a_construction_of_its_own() {
        let moving = point(Presentation::Signal, false, true);
        let still = point(Presentation::Signal, false, false);
        assert!(moving.bulk && !still.bulk);
        assert_eq!(still.value, ValueRange::auto());
        assert_eq!(moving.value, ValueRange::new(-1.0, 1.0));
    }
}
