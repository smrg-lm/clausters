//! The signal presentation's **one** column source and its mesh renderer.
//!
//! Every view that draws a signal against time answers the same two questions
//! per pixel — *what is the min/max over the span this pixel covers* and *what
//! is the sample at this position* — and the catalog used to answer them three
//! times over: the heavy waveform through [`WaveformData::column`], a clip's
//! inline body with its own slice fold, and the static plot with a third one
//! over an interleaved buffer. [`Trace`] is that one answer, with an arm per
//! source shape: raw interleaved samples, or a [`WaveformData`]'s peak pyramid.
//!
//! [`draw_channel`] is **the** renderer of a signal against time — the only
//! one. A navigable waveform, a clip's take, a plot's series and a meter's
//! history all reach the screen through this function, differing in the three
//! coordinate maps they hand it and in nothing else.
//!
//! It was not always one. The navigable view used to build its own vertex
//! buffer through a dedicated `wgpu` pipeline, drawing the same two regimes a
//! second time — and the two drifted exactly where duplicated arithmetic
//! drifts: the pipeline took a neighbouring sample on the left of the window
//! but not on the right (so a deep zoom drew a trace arriving at a sample and
//! stopping dead), marked samples with squares where this one marks discs, and
//! split the regimes on the opposite side of the threshold. The pipeline did
//! nothing for it: its columns were folded on the CPU per pixel, as here, and
//! its vertex count was bounded by the render width, as here. So it is gone,
//! and `crate::waveform` is now the data and the navigation state alone.

use clausters_core::loudness;
use clausters_core::peaks;

use crate::waveform::WaveformData;

use clausters_core::resample;

use crate::host::layout::Rect;
use crate::host::paint::{Color, Mesh};
use crate::host::theme::Theme;

/// At or below this many samples per pixel a trace **with no sample dots** is
/// drawn as a polyline through the raw samples rather than as min/max columns,
/// and a column this narrow measures the line between samples rather than the
/// band around it. A trace with dots crosses where they appear instead
/// ([`samples_are_drawn`]), so its picture changes character exactly once.
pub const LINE_THRESHOLD: f64 = 2.0;

/// **How close the level may come to the envelope before it stops being a
/// second reading of it** — the ratio of the body's amplitude to the peak's,
/// over the span on screen.
///
/// The measure itself is exact at any span; what a short span stops being is
/// *informative*. Root-mean-square and peak converge as a column's window
/// shrinks below a cycle — over a quarter cycle of a bass note the two differ
/// by a factor of `0.9` — so the body ends up drawing the envelope's own
/// outline back over it, and on the way there it reads the wave's **phase**,
/// beating against the period in a lattice nobody can interpret. Watching the
/// level climb onto the peaks is itself the artefact, which is why the rule is
/// stated where the climb starts rather than where it ends.
///
/// It is a fact about **the signal on screen**, not about the zoom, and that is
/// the whole of it: the reader sees the level go exactly when it has stopped
/// saying anything the envelope was not already saying, at whatever
/// magnification that happens. Two things were tried before it and are recorded
/// in `docs/decisions.md`: an opacity ramped linearly in samples-per-pixel,
/// which made the body's *weight* track the zoom, and a duration floor, which
/// is what the quantity is made of but is an order of magnitude coarser than a
/// working zoom — at six seconds across a window a column is under seven
/// milliseconds, so any floor that reads as perceptual removes the body from
/// the place it is looked at.
///
/// `0.8` was chosen by eye, at the picture: a body still a fifth clear of the
/// peaks reads as a level, and closer than that it reads as a second edge
/// inside the first. A number derived from the convergence instead would have
/// to name a waveform to be derived *for* — a sine sits at `0.707` over a long
/// window and a square at `1.0` — which is why this is a threshold on what is
/// on screen and not a formula.
pub const BODY_MERGE_RATIO: f32 = 0.8;

/// **The window the level is averaged over**, in seconds — fixed, and that is
/// the point of it.
///
/// A root-mean-square is an average *over a duration*, so the duration is part
/// of the reading: average whatever a pixel column happens to cover and the
/// body's own values follow the **zoom**, changing as you move the view over
/// samples that did not change. A level is a property of the signal, so the
/// window is the signal's — the same 50 ms whatever the magnification — and the
/// body stops moving when you do.
///
/// `50 ms` is the editors' number: WaveLab's default RMS window, adjustable up
/// to 999 ms. It sits below the ear's own integration — energy integrates over
/// something like 200 ms and a VU meter's window is 300 ms — so it is the floor
/// of what still reads as a level rather than the window a meter would pick.
///
/// What ends the body is then the *other* side of the same picture: the
/// **envelope** narrows as the zoom advances, since a column covers less of the
/// wave, and once it has come down onto the level ([`BODY_MERGE_RATIO`]) there
/// are no longer two readings — so the body goes, which is also what keeps it
/// from ever poking out of the envelope that contains it.
pub const BODY_WINDOW_SECS: f64 = 0.050;

/// A signal's samples, read per pixel column. The two arms are the two shapes
/// a source arrives in: an interleaved buffer held in full (an inline body, a
/// plot's array), or a [`WaveformData`] whose peak pyramid answers a zoomed-out
/// column without touching the samples.
pub enum Trace<'a> {
    /// Raw interleaved samples: frame `f` of channel `ch` is
    /// `samples[f * channels + ch]`.
    Samples { samples: &'a [f32], channels: usize },
    /// A pyramid-backed source — the editor-grade path, where a column costs a
    /// pyramid read rather than the samples it summarizes.
    Data(&'a WaveformData),
    /// **A loudness profile**, read as a signal in LU: the curve of what a
    /// meter would have shown along the take ([`loudness::Profile`]).
    ///
    /// It is an arm of this enum rather than a renderer of its own because it
    /// answers the same two questions every other source does — what did this
    /// column cover, and what is the value here — so the one renderer draws it
    /// with nothing but a different vertical map. `floor` is where a reading of
    /// silence lands, which is the bottom of the scale the layer is drawn on:
    /// a loudness of `-inf` has no place on an axis and every axis has a
    /// bottom.
    Loudness {
        profile: &'a loudness::Profile,
        window: f64,
        floor: f32,
    },
}

impl<'a> Trace<'a> {
    /// An interleaved buffer of `channels` channels.
    pub fn samples(samples: &'a [f32], channels: usize) -> Self {
        Trace::Samples {
            samples,
            channels: channels.max(1),
        }
    }

    /// How many frames (per-channel samples) the source holds.
    pub fn frames(&self) -> usize {
        match self {
            Trace::Samples { samples, channels } => samples.len() / channels,
            Trace::Data(data) => data.total_samples(),
            Trace::Loudness { profile, .. } => profile.frames() as usize,
        }
    }

    /// Whether individual samples can be read at all. A **cache-only** source —
    /// a pyramid mapped without its buffer, the compact bulk path — has none, so
    /// every zoom stays in the column regime: asking it for a sample would read
    /// an empty buffer and collapse the signal to a flat line exactly where the
    /// viewer zoomed in to see it.
    pub fn has_raw(&self) -> bool {
        match self {
            Trace::Samples { samples, .. } => !samples.is_empty(),
            Trace::Data(data) => data.has_raw(),
            // A profile answers for any point of the take, at any zoom: its
            // readings are what it holds, and they are not samples to run out
            // of.
            Trace::Loudness { .. } => true,
        }
    }

    /// How many channels the source holds.
    pub fn channels(&self) -> usize {
        match self {
            Trace::Samples { channels, .. } => *channels,
            Trace::Data(data) => data.num_channels(),
            // A loudness reading is the channels **summed**, with their
            // weights: one curve for the take, not one per lane.
            Trace::Loudness { .. } => 1,
        }
    }

    /// Min/max of channel `ch` over the source span `[s0, s1)` — the span one
    /// pixel column covers, at `samples_per_px`. An empty or out-of-range span
    /// reads as silence rather than as nothing, so a column always draws.
    pub fn column(&self, ch: usize, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
        match self {
            Trace::Samples { samples, channels } => {
                let frames = samples.len() / channels;
                if frames == 0 || ch >= *channels {
                    return (0.0, 0.0);
                }
                // The last column can land exactly on the end: keep the span
                // non-empty and inside the buffer (a `clamp` with min > max
                // panics).
                let a = (s0.floor().max(0.0) as usize).min(frames - 1);
                let b = (s1.ceil() as usize).clamp(a + 1, frames);
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for f in a..b {
                    let v = samples[f * channels + ch];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                (lo, hi)
            }
            Trace::Data(data) => data.column(ch, samples_per_px, s0, s1),
            // The quietest and loudest readings the column covers, so a
            // zoomed-out curve keeps the excursion between two pixels instead
            // of sampling one reading out of the hundred it spans.
            Trace::Loudness {
                profile,
                window,
                floor,
            } => {
                let (lo, hi) = profile
                    .column(*window, s0.max(0.0) as u64, s1.max(0.0) as u64)
                    .unwrap_or((*floor, *floor));
                (lo.max(*floor), hi.max(*floor))
            }
        }
    }

    /// The **mean square** of channel `ch` over the source span `[s0, s1)` —
    /// what a column measures when it measures level rather than extent.
    ///
    /// `None` is a source that cannot answer, never a zero: a pyramid cached
    /// before the statistic existed has an envelope and no energy, and a body
    /// drawn from zeros would claim silence over samples that is not silent.
    /// Raw samples always answer, since the measure is a loop over them.
    pub fn column_ms(&self, ch: usize, samples_per_px: f64, s0: f64, s1: f64) -> Option<f32> {
        match self {
            Trace::Samples { samples, channels } => {
                let frames = samples.len() / channels;
                if frames == 0 || ch >= *channels {
                    return None;
                }
                let a = (s0.floor().max(0.0) as usize).min(frames - 1);
                let b = (s1.ceil() as usize).clamp(a + 1, frames);
                let mut sum = 0.0f64;
                for f in a..b {
                    let v = samples[f * channels + ch] as f64;
                    sum += v * v;
                }
                Some((sum / (b - a) as f64) as f32)
            }
            Trace::Data(data) => data.column_ms(ch, samples_per_px, s0, s1),
            // A level of a loudness curve is not a measurement of anything:
            // the curve is already a level.
            Trace::Loudness { .. } => None,
        }
    }

    /// One sample of channel `ch` at frame position `s`, clamped to the source.
    pub fn at(&self, ch: usize, s: f64) -> f32 {
        match self {
            Trace::Samples { samples, channels } => {
                let frames = samples.len() / channels;
                if frames == 0 || ch >= *channels {
                    return 0.0;
                }
                let f = (s.round().max(0.0) as usize).min(frames - 1);
                samples[f * channels + ch]
            }
            Trace::Data(data) => data.samples_at(ch, s.round().max(0.0) as usize),
            Trace::Loudness {
                profile,
                window,
                floor,
            } => (profile.window_at(*window, s.max(0.0) as u64) as f32).max(*floor),
        }
    }
}

/// **What a column measures.** These are pictures of one span, not of
/// different sources: `Peak` is the extent the signal reached (the min/max
/// envelope every editor draws), `Rms` the level it held there — the body an
/// editor draws *inside* that envelope — and `Signal` what the waveform did
/// **between** the samples. One function draws any of them, placed once per
/// measure, which is what lets a view show several without a second renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Measure {
    /// The min/max envelope: what the signal reached.
    #[default]
    Peak,
    /// The symmetric body about zero at `sqrt(mean square)`: what it held.
    Rms,
    /// **The reconstruction**: the band-limited signal the samples are of,
    /// drawn between them once they are separate points, with the peaks that
    /// leave full scale marked.
    ///
    /// It is a **layer over** the sample picture and never a replacement for
    /// it, which is the whole of why it is a measure: the samples' own
    /// envelope and polyline are continuous across every zoom, and this is
    /// drawn on top of them in its own ink, so nothing about the picture
    /// *changes* when the reconstruction becomes available — something is
    /// added. iZotope RX, the one editor that draws this at all, states the
    /// same shape: the analog waveform is plotted "in red under digital sample
    /// values (blue)". Replacing the one with the other is what makes a
    /// waveform's amplitude appear to jump at a zoom threshold, since the two
    /// differ by a fixed amount that is the signal's own inter-sample content.
    Signal,
    /// **The momentary loudness**: what a meter would have read at each point
    /// of the take, over the 400 ms up to it (ITU-R BS.1770 and EBU R 128).
    ///
    /// The one measure drawn on an axis of its own. Peak, level and
    /// reconstruction all measure the signal's *amplitude* and share the
    /// picture's vertical; loudness is in LU, so this layer brings its own
    /// scale and its own ruler and is read against the target line rather than
    /// against the samples it crosses.
    Momentary,
    /// **The short-term loudness**: the same reading over the 3 s up to each
    /// point, which is the curve a loudness track plots.
    Short,
}

impl Measure {
    /// The wire name, or `None` for one this build does not know — which reads
    /// as "the prop was not set" rather than as an error, the protocol's own
    /// posture for an unknown value.
    pub fn parse(name: &str) -> Option<Measure> {
        match name {
            "peak" => Some(Measure::Peak),
            "rms" => Some(Measure::Rms),
            "signal" => Some(Measure::Signal),
            "momentary" => Some(Measure::Momentary),
            "short" => Some(Measure::Short),
            _ => None,
        }
    }

    /// The name this measure answers `/gui_query` with.
    pub fn name(self) -> &'static str {
        match self {
            Measure::Peak => "peak",
            Measure::Rms => "rms",
            Measure::Signal => "signal",
            Measure::Momentary => "momentary",
            Measure::Short => "short",
        }
    }

    /// The loudness window this measure reads, in seconds — `None` for the
    /// measures of amplitude, which is also the test for *which axis a layer is
    /// drawn on*: a measure with a window is in LU and brings its own scale.
    pub fn loudness_window(self) -> Option<f64> {
        match self {
            Measure::Momentary => Some(loudness::MOMENTARY_SECONDS),
            Measure::Short => Some(loudness::SHORT_TERM_SECONDS),
            _ => None,
        }
    }

    /// Whether this measure is read in LU rather than in the picture's own
    /// vertical.
    pub fn is_loudness(self) -> bool {
        self.loudness_window().is_some()
    }
}

/// **What a view measures**, which may be more than one thing: the classic
/// editor picture is the envelope with the level body drawn *inside* it.
///
/// It is a set on the element rather than a stack of elements, and that is the
/// correction the first attempt earned: every signal picture paints its own
/// field before it draws (a heavy view's `view_field`, a plot's `track`), so two
/// pictures on one rectangle are not layers — the second one's field hides the
/// first. Layering happens *inside* one body or not at all, which is also what
/// the picture wants: one field, one axis, one ruler, one selection, one
/// playhead, one upload.
///
/// The order is the type's own, and there is nothing to choose: the envelope is
/// the outer shape and the level lives inside it, so `Peak` draws first
/// whatever order the wire named them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measures(u8);

impl Default for Measures {
    /// **The envelope, and the reconstruction over it.** The reconstruction
    /// only draws where the samples are separate points, so at every other zoom
    /// the default is the envelope every editor draws; where they are, it is
    /// the dots and the signal through them rather than straight segments --
    /// the default iZotope RX ships, and the picture a reader zooming in to the
    /// samples is asking for. A view that wants the bare samples says
    /// `measure: "peak"`.
    fn default() -> Self {
        Measures(Measures::of(Measure::Peak).0 | Measures::of(Measure::Signal).0)
    }
}

impl Measures {
    /// Every measure there is, in drawing order — back to front.
    pub const ALL: [Measure; 5] = [
        Measure::Peak,
        Measure::Rms,
        Measure::Signal,
        Measure::Momentary,
        Measure::Short,
    ];

    /// The set holding exactly one measure.
    pub fn of(m: Measure) -> Self {
        Measures(1 << m as u8)
    }

    /// The empty set — what a stack with no measure in it folds from.
    pub fn none() -> Self {
        Measures(0)
    }

    /// This set, plus `m`.
    pub fn with(self, m: Measure) -> Self {
        Measures(self.0 | Measures::of(m).0)
    }

    /// The wire form: measure names separated by spaces (`"peak"`,
    /// `"peak rms"`). `None` when a name is one this build does not know or the
    /// list is empty — which reads as "the prop was not set" rather than as an
    /// error, the protocol's own posture, and keeps a view that names nothing
    /// drawing something.
    pub fn parse(names: &str) -> Option<Self> {
        let mut set = Measures(0);
        for name in names.split_whitespace() {
            set.0 |= Measures::of(Measure::parse(name)?).0;
        }
        (set.0 != 0).then_some(set)
    }

    /// Whether `m` is drawn.
    pub fn has(self, m: Measure) -> bool {
        self.0 & Measures::of(m).0 != 0
    }

    /// The measures drawn, in drawing order.
    pub fn iter(self) -> impl Iterator<Item = Measure> {
        Measures::ALL.into_iter().filter(move |m| self.has(*m))
    }

    /// What `/gui_query` answers — the wire form this parsed from.
    pub fn name(self) -> String {
        self.iter().map(Measure::name).collect::<Vec<_>>().join(" ")
    }
}

/// The color a measure is drawn in: the channel's own series color for an
/// envelope, the **body** role for a level.
///
/// It is one function because the choice is the same wherever a signal is
/// drawn — a navigable view, a clip's take, a plot — and a body that took the
/// series color would be invisible against the envelope it sits inside, which
/// is the one thing a layer must not be.
pub fn measure_color(theme: &Theme, measure: Measure, series: Color) -> Color {
    match measure {
        Measure::Peak => series,
        Measure::Rms => theme.trace_body,
        // The reconstruction is read *against* the samples it is drawn over,
        // so it takes an ink of its own rather than the series colour -- the
        // same reason a level body does, and the same shape RX states (the
        // analog waveform "in red under digital sample values (blue)").
        Measure::Signal => theme.trace_signal,
        // The loudness curves are read against their own scale and against the
        // target line, not against the samples they are drawn over, so they
        // take an ink of their own -- and one ink for both, because a view
        // drawing the two draws one curve inside the other, the short-term
        // reading being the momentary one integrated longer.
        Measure::Momentary | Measure::Short => theme.trace_loudness,
    }
}

/// How a trace is inked: the color of its columns and polyline, and the
/// trace's **weight** — the width the polyline is stroked with, and the least a
/// column is ever inked, so a signal keeps one optical weight across the regime
/// boundary (a column is as wide as the pixel column it fills, which is what
/// makes the columns tile).
#[derive(Debug, Clone, Copy)]
pub struct TraceStyle {
    pub color: Color,
    pub width: f32,
    /// The radius a **sample dot** is drawn at once the samples are far enough
    /// apart to carry one ([`dots_fit`]); `0` draws none.
    ///
    /// It is `point_radius` — the role a break-point is drawn at — and that is
    /// the point rather than a coincidence: a dot says *this is a sample, and
    /// it is a thing you could take hold of*, which is what sample-level
    /// editing will grab. Sizing it as a curve's break-point means the two
    /// affordances read as the same kind of target the day the second one
    /// becomes draggable.
    pub dot_radius: f32,
    /// What each column measures — the envelope, the level inside it, or the
    /// reconstruction over both.
    pub measure: Measure,
    /// **Every measure this view draws**, not only this one. A layer needs it
    /// for the one case where layers are not independent: the straight line
    /// between two samples exists because nothing better is available, so when
    /// the reconstruction is drawn over it the sample layer keeps its dots and
    /// drops the line.
    pub layers: Measures,
    /// **The window a level is averaged over**, in samples
    /// ([`BODY_WINDOW_SECS`] at the source's rate) — resolved by the caller,
    /// because the trace works in samples and only the caller knows the rate.
    ///
    /// `0` is an unknown rate, and then a column averages its own span: a live
    /// window already measured in milliseconds, or a harness with no source
    /// rate, has nothing better to offer and nothing that moves under a zoom
    /// it does not have.
    pub body_window: f64,
    /// **The ink an inter-sample over is marked in** — the meter's clip lamp,
    /// because it is the same statement about the same signal. Transparent
    /// (the default) marks none, which is what a view with no amplitude
    /// meaning wants.
    pub over: Color,
    /// The glyph scale the over figure is written at — the placement's caption
    /// scale, handed over like every other size here, so a drawing never reads
    /// a size table of its own. `0` writes no figure.
    pub over_text: f32,
    /// **The ground a figure is written on**, as `(colour, corner radius)` —
    /// set where this layer draws over a picture of its own (a spectrogram
    /// under it), where a figure written straight onto the texture is
    /// unreadable wherever the analysis is bright. `None` writes the text
    /// bare, which is what a layer over a plain field does.
    pub plate: Option<(Color, f32)>,
    /// **How much of the samples exists**, in frames — `None` for the
    /// ordinary case, where all of it does.
    ///
    /// It is set for a take that is being **written into as it is drawn**: past
    /// the write frontier a recording's buffer holds its own zeros, which are
    /// not silence in the samples but the absence of samples, and the
    /// minimum-ink rule would otherwise draw a flat line across a stretch
    /// nothing has happened in yet. So the picture stops, and the axis past it
    /// stays empty until the writer gets there.
    pub written: Option<f64>,
}

/// Whether sample dots are drawn at `spacing` pixels apart: they need to read
/// as separate points, so a dot is drawn only once its neighbour is three radii
/// away — a full diameter of air between them. Below that the line is the
/// picture and a row of touching dots would just thicken it.
pub fn dots_fit(spacing: f32, radius: f32) -> bool {
    radius > 0.0 && spacing >= 3.0 * radius
}

/// The same question in the **axis' own unit**: whether a picture at
/// `samples_per_px` is drawing its samples one by one.
///
/// It exists so a *gesture* can ask it. Writing one sample is only meaningful
/// where the reader can see which sample they are writing, and the drawing
/// already decides exactly that with [`dots_fit`] — so the pencil asks the
/// picture rather than carrying a threshold of its own. Two numbers for one
/// rule is how a stroke came to be allowed over a picture with no dots in it,
/// writing hundreds of samples the hand could not aim at.
pub fn samples_are_drawn(samples_per_px: f64, radius: f32) -> bool {
    if !samples_per_px.is_finite() || samples_per_px <= 0.0 {
        // No span to divide by: whatever is drawn, one pixel is not more than
        // one sample, so the dot is the only question left.
        return radius > 0.0;
    }
    dots_fit((1.0 / samples_per_px) as f32, radius)
}

/// How many samples a pixel may cover before the samples stop being drawn one
/// by one — the ceiling [`samples_are_drawn`] tests against, for a refusal that
/// can say what it is waiting for.
pub fn drawable_per_px(radius: f32) -> f64 {
    if radius <= 0.0 {
        return 0.0;
    }
    1.0 / (3.0 * radius) as f64
}

impl TraceStyle {
    /// A trace inked at `width`, with no sample dots.
    pub fn new(color: Color, width: f32) -> Self {
        TraceStyle {
            color,
            width,
            dot_radius: 0.0,
            over: [0.0, 0.0, 0.0, 0.0],
            over_text: 0.0,
            measure: Measure::Peak,
            layers: Measures::of(Measure::Peak),
            body_window: 0.0,
            plate: None,
            written: None,
        }
    }

    /// The same trace, writing its figures on a plate — what a layer drawn
    /// over a texture does with text.
    pub fn with_plate(mut self, color: Color, radius: f32) -> Self {
        self.plate = Some((color, radius));
        self
    }

    /// The same trace, marking each sample once they are far enough apart.
    pub fn with_dots(mut self, radius: f32) -> Self {
        self.dot_radius = radius;
        self
    }

    /// The same trace, marking the peaks **between** its samples that leave
    /// full scale — see [`draw_channel`].
    pub fn with_overs(mut self, over: Color, text: f32) -> Self {
        self.over = over;
        self.over_text = text;
        self
    }

    /// The same trace, told **every** measure the view draws — so a layer can
    /// answer for what is under it (see [`TraceStyle::layers`]).
    pub fn with_layers(mut self, layers: Measures) -> Self {
        self.layers = layers;
        self
    }

    /// The same trace, measuring `measure` instead of the envelope.
    pub fn with_measure(mut self, measure: Measure) -> Self {
        self.measure = measure;
        self
    }

    /// The same trace over samples that only exists as far as `frames` — a
    /// take being recorded. `None` (the default) is all of it.
    pub fn with_written(mut self, frames: Option<u64>) -> Self {
        self.written = frames.map(|f| f as f64);
        self
    }

    /// The same trace, told the source's sample rate — which is how the level's
    /// fixed window ([`BODY_WINDOW_SECS`]) becomes a number of samples to
    /// average. A rate of zero (unknown) leaves each column averaging its own
    /// span.
    pub fn with_rate(mut self, sample_rate: f64) -> Self {
        self.body_window = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate * BODY_WINDOW_SECS
        } else {
            0.0
        };
        self
    }
}

/// Draws one channel of `trace` into `rect`, resolved to the rect's own pixel
/// width and never finer — the project's one graphics rule.
///
/// The two coordinate maps are the caller's, because they are what differs
/// between the views that share this renderer: `src` takes an x pixel to the
/// source frame it falls on (through a clip's placement and the navigation
/// window, or straight down the whole buffer), `x_of` is its inverse, and
/// `y_at` maps a sample value to a y pixel inside the lane. Above
/// [`LINE_THRESHOLD`] samples per pixel — or whenever the source cannot answer
/// for one sample ([`Trace::has_raw`]) — it draws one min/max column per pixel,
/// each **joined to the one before it** ([`peaks::join`], the core's rule and
/// not this renderer's, because a page drawing its own columns from the same
/// pyramid takes the same one) so the picture stays the one
/// continuous curve the other regime draws; at or below it, the polyline
/// through every visible sample **plus the one beyond each edge**, so the
/// segments that cross the edges are drawn and the trace enters and leaves the
/// rect instead of stopping at the last sample it can see.
// The rect, the source, the channel, three coordinate maps and a style: all
// distinct inputs to one drawing pass, clearer flat than bundled.
#[allow(clippy::too_many_arguments)]
pub fn draw_channel(
    mesh: &mut Mesh,
    rect: Rect,
    trace: &Trace,
    ch: usize,
    src: impl Fn(f32) -> f64,
    x_of: impl Fn(f64) -> f32,
    y_at: impl Fn(f32) -> f32,
    style: TraceStyle,
) {
    let frames = trace.frames();
    if frames < 2 || rect.w < 1.0 || rect.h <= 0.0 {
        return;
    }
    // **A measure is drawn by the source that can answer it, and by no other.**
    // A loudness layer is the profile's curve and an amplitude layer is the
    // samples', so the two never cross: a view whose measures name a loudness
    // one draws it from a `Loudness` trace (with that trace's own vertical map)
    // and draws nothing for it from the samples, which is what lets every
    // caller loop over the measures without knowing which sources it holds.
    if style.measure.is_loudness() != matches!(trace, Trace::Loudness { .. }) {
        return;
    }
    // **The trace bounds itself to its lane.** It reads a *span* per pixel and
    // deliberately reaches past the pixels it fills — the sample before the
    // left edge and the one after the right, or the line would start and end
    // inside the box — and it reaches past the top and bottom too whenever a
    // value falls outside the vertical window. Those overshoots are the picture
    // where they cross an edge and litter where they land: a pair of discs on
    // the ruler beside the view, a stroke over the labels under it. A container
    // that is a coordinate system masks its contents anyway (a clip does), but
    // a free-standing view is nobody's content, so the drawing that knows it
    // overshoots is the one that answers for it — every destination at once,
    // rather than one mask per placement. Narrowed, never widened: whatever
    // mask was already in force still holds, and it is put back on the way out.
    let outer = mesh.clip();
    mesh.set_clip(Some(outer.map_or(rect, |c| c.intersect(rect))));
    let cols = rect.w.max(1.0) as usize;
    let cw = rect.w / cols as f32;
    let per_px = (src(rect.x + cw) - src(rect.x)).max(0.0);
    // **The regime, decided once**, because the level body's own answer starts
    // here: a body is a reading *of* an envelope, so it is drawn where the
    // envelope is and nowhere else — past the crossing the trace is the
    // polyline through the samples themselves and there is nothing left for a
    // level to be a reading of.
    // **One moment, and it is the dots'.** The picture of the samples changes
    // exactly once as a zoom comes in: when the samples become separate points.
    // Before that it is the envelope, and past it it is the dots, the segments
    // between them and the reconstruction over them -- all at once, so a reader
    // never sees the waveform change character and only then see why. A style
    // with no dots has no such moment and keeps the old crossing.
    let sample_zoom = if style.dot_radius > 0.0 {
        samples_are_drawn(per_px, style.dot_radius)
    } else {
        per_px <= LINE_THRESHOLD
    };
    let columns = !sample_zoom || !trace.has_raw();
    // The reconstruction is a layer over **resolvable** samples. Zoomed out, a
    // picture of it would be the envelope of the reconstructed signal, which
    // needs that envelope stored beside the samples' own (the peak pyramid)
    // -- not a reconstruction of every sample on every frame. Until then it
    // draws nothing here, and the sample envelope under it is unchanged: a
    // layer that is absent is not a picture that jumped.
    if style.measure == Measure::Signal && columns {
        mesh.set_clip(outer);
        return;
    }
    if style.measure == Measure::Rms {
        if columns
            && per_px > LINE_THRESHOLD
            && !body_merges(trace, ch, &src, rect, cols, cw, per_px, style.body_window)
        {
            draw_body(mesh, rect, trace, ch, &src, &y_at, style, cols, cw, per_px);
        }
        mesh.set_clip(outer);
        return;
    }
    if columns {
        // What the column before this one reached — see [`peaks::join`].
        let mut prev: Option<(f32, f32)> = None;
        for c in 0..cols {
            let x = rect.x + c as f32 * cw;
            // Past the written frontier there is no samples yet, so there is
            // no column: the axis is drawn and left empty rather than inked
            // over the buffer's own zeros.
            if style.written.is_some_and(|w| src(x) >= w) {
                break;
            }
            let (lo, hi) = if per_px <= LINE_THRESHOLD && trace.has_raw() {
                line_column(trace, ch, src(x), src(x + cw), frames)
            } else {
                trace.column(ch, per_px, src(x), src(x + cw))
            };
            if lo > hi {
                prev = None;
                continue;
            }
            // **Joined to the column before it**, which is what keeps the
            // trace one curve: see [`peaks::join`]. The walk is the renderer's
            // — a column that held nothing starts the run again above.
            let (vlo, vhi) = peaks::join(lo, hi, prev.take());
            prev = Some((lo, hi));
            // A column is a quad, **never inked thinner than the trace's
            // weight in either direction**: at least the pixel column it fills,
            // so columns tile into a solid band on a dense signal, and at least
            // `style.width`, so a signal keeps one optical weight across the
            // regime boundary. The centred stroke this replaces was thinner on
            // both counts: capped to the column width it came out below the
            // weight the polyline uses a pixel away, and where the signal
            // barely moves inside one column it was a zero-length line, which
            // draws *nothing* — so the flat stretch of an envelope disappeared
            // exactly where it is most readable. Overlapping neighbours is the
            // price of the second floor, and it is what a stroke does anyway.
            let (top, bottom) = (y_at(vhi), y_at(vlo));
            let (w, h) = (cw.max(style.width), (bottom - top).max(style.width));
            let (cx, cy) = (x + cw * 0.5, (top + bottom) * 0.5);
            mesh.rect(Rect::new(cx - w * 0.5, cy - h * 0.5, w, h), style.color);
        }
    } else {
        // Few enough samples per pixel that individual ones matter: step by
        // sample, not by pixel, so nothing visible is skipped.
        let first = src(rect.x).floor().max(0.0) as usize;
        let end = style
            .written
            .map_or(frames, |w| (w.max(0.0) as usize).min(frames));
        if end == 0 {
            mesh.set_clip(outer);
            return;
        }
        let last = (src(rect.x + rect.w).ceil().max(0.0) as usize).min(end - 1);
        // Where the samples land decides whether each one is marked: the line
        // is an interpolation the drawing invents, and a dot is what says which
        // points of it are data.
        let spacing = (x_of(1.0) - x_of(0.0)).abs();
        let dots = dots_fit(spacing, style.dot_radius);
        // **Never finer than the screen**: the reconstruction is drawn where
        // there are pixels for a curve and nowhere else.
        let step = curve_step(spacing);
        if style.measure == Measure::Signal {
            // Where the sub-samples are closer than the screen shows, the stride
            // is the whole interval: the curve is then its two endpoints, the
            // same pixels, and the layer still appears at the same moment as
            // the dots.
            let step = step.unwrap_or(resample::FINE.factor());
            draw_signal(
                mesh, rect, trace, ch, first, last, end, step, &x_of, &y_at, style,
            );
            mesh.set_clip(outer);
            return;
        }
        // **The sample layer keeps its line only where nothing better is
        // drawn.** The straight segment between two samples exists because no
        // reconstruction is available; where the `Signal` layer draws one over
        // it, the segment is a second, wrong line between the same two dots,
        // so it goes and the dots stay -- the shape iZotope RX draws, samples
        // as points and the analog waveform as the curve through them.
        let line = !style.layers.has(Measure::Signal);
        // **A dot marks a sample of this view, and the view is half-open.** The
        // line reaches the samples just past each edge so it crosses them, but
        // a sample lying exactly on the right edge is the first one *after* the
        // view -- in a box cut in two, the other half's first sample -- and a
        // dot there drew it in both halves. A tolerance far under a sample
        // absorbs the pixel map's rounding at the edges.
        const ON_EDGE: f64 = 1e-6;
        let (from, to) = (src(rect.x), src(rect.x + rect.w));
        let mut prev: Option<[f32; 2]> = None;
        for f in first..=last.max(first) {
            let p = [x_of(f as f64), y_at(trace.at(ch, f as f64))];
            if let (true, Some(q)) = (line, prev) {
                mesh.line(q, p, style.width, style.color);
            }
            let inside = f as f64 >= from - ON_EDGE && (f as f64) < to - ON_EDGE;
            if dots && inside {
                mesh.disc(p[0], p[1], style.dot_radius, style.color);
            }
            prev = Some(p);
        }
    }
    mesh.set_clip(outer);
}

/// **A column narrower than two samples, measured as the line between them.**
///
/// The ordinary column is the min/max of the samples it touches, and under two
/// samples a pixel wide that is the two samples *around* it -- so every pixel
/// between them draws the whole vertical span, a band where a line belongs, and
/// the picture changed character before the dots appeared. Reading the straight
/// line between the samples at the column's two edges (and any sample strictly
/// inside) rasterizes exactly the segment the sample regime draws, so the
/// envelope and the line are one picture until the dots say the samples are
/// separate points.
fn line_column(trace: &Trace, ch: usize, s0: f64, s1: f64, frames: usize) -> (f32, f32) {
    let last = frames.saturating_sub(1) as f64;
    let at = |s: f64| {
        let s = s.clamp(0.0, last);
        let a = s.floor();
        let t = (s - a) as f32;
        let va = trace.at(ch, a);
        let vb = trace.at(ch, (a + 1.0).min(last));
        va + (vb - va) * t
    };
    let (mut lo, mut hi) = {
        let (a, b) = (at(s0), at(s1));
        (a.min(b), a.max(b))
    };
    let mut f = s0.floor() + 1.0;
    while f < s1 && f <= last {
        let v = trace.at(ch, f);
        lo = lo.min(v);
        hi = hi.max(v);
        f += 1.0;
    }
    (lo, hi)
}

/// **The `Signal` layer**: the band-limited reconstruction through the
/// visible samples, with the peaks between them that leave full scale marked,
/// and how far past full scale the loudest went written once.
///
/// It draws **no dots** -- they are the sample layer's, under it -- and its
/// ink is its own ([`measure_color`]), so the curve is read *against* the
/// samples rather than instead of them.
// The rect, the source, the channel, the visible span, the stride, two
// coordinate maps and a style: distinct inputs to one pass, clearer flat.
#[allow(clippy::too_many_arguments)]
fn draw_signal(
    mesh: &mut Mesh,
    rect: Rect,
    trace: &Trace,
    ch: usize,
    first: usize,
    last: usize,
    end: usize,
    step: usize,
    x_of: &impl Fn(f64) -> f32,
    y_at: &impl Fn(f32) -> f32,
    style: TraceStyle,
) {
    // The loudest inter-sample over on screen, for the one figure.
    let mut worst = 0.0f32;
    let mut sub = [0.0f32; resample::MAX_FACTOR];
    for f in first.max(1)..=last.max(first) {
        let q = [x_of(f as f64 - 1.0), y_at(trace.at(ch, f as f64 - 1.0))];
        let p = [x_of(f as f64), y_at(trace.at(ch, f as f64))];
        let count = reconstruct(trace, ch, f - 1, end, &mut sub);
        let mut at = q;
        let mut k = step;
        while k < count {
            let x = x_of(f as f64 - 1.0 + k as f64 / count as f64);
            let to = [x, y_at(sub[k])];
            mesh.line(at, to, style.width, style.color);
            // **An over between two samples, marked where it is.** Neither
            // sample need have left full scale, so nothing reading samples has
            // anything to report -- the one thing a sample picture structurally
            // cannot show.
            if sub[k].abs() > 1.0 && style.over[3] > 0.0 {
                let h = style.width * 3.0;
                let top = if sub[k] > 0.0 {
                    rect.y
                } else {
                    rect.y + rect.h - h
                };
                mesh.rect(
                    Rect::new(x - style.width, top, style.width * 2.0, h),
                    style.over,
                );
                worst = worst.max(sub[k].abs());
            }
            at = to;
            k += step;
        }
        mesh.line(at, p, style.width, style.color);
    }
    // **How far past full scale the loudest went**, in dBTP, once for the
    // view -- the question a lit clip lamp raises, and the same answer.
    if worst > 1.0 && style.over_text > 0.0 {
        let text = format!("{:+.1}", 20.0 * worst.log10());
        let w = crate::host::font::width(&text, style.over_text);
        if w < rect.w {
            if let Some((plate, radius)) = style.plate {
                let h = crate::host::font::height(style.over_text);
                let inset = style.width;
                mesh.round_rect(
                    Rect::new(
                        rect.x + rect.w - w - 2.0 - inset,
                        rect.y + 2.0 - inset,
                        w + 2.0 * inset,
                        h + 2.0 * inset,
                    ),
                    radius,
                    plate,
                );
            }
            crate::host::font::text(
                mesh,
                &text,
                rect.x + rect.w - w - 2.0,
                rect.y + 2.0,
                style.over_text,
                style.over,
            );
        }
    }
}

/// **Whether the curve between two samples is worth drawing, and how finely.**
///
/// `None` while the samples are close enough that a straight segment and the
/// reconstruction land on the same pixels -- which is most zoom levels, and
/// where drawing a curve would be arithmetic nobody can see. Otherwise the
/// stride through [`resample::FINE`]'s sub-samples: every one where the samples
/// are far apart, every second or fourth where they are nearer, so the curve is
/// never resolved finer than the screen can show it.
fn curve_step(spacing: f32) -> Option<usize> {
    let factor = resample::FINE.factor();
    // Two device pixels per drawn segment is the floor: below that the segments
    // are shorter than the stroke is wide.
    let segments = (spacing / 2.0).floor() as usize;
    if segments < 2 {
        return None;
    }
    Some((factor / segments.min(factor)).max(1))
}

/// The reconstruction of the interval **starting at sample `f`**, into `out`.
///
/// Returns how many sub-samples were written (`FINE`'s factor, or `0` where
/// there is nothing to reconstruct). The window is read through the trace
/// itself, with the samples outside the take taken as silence -- the edge
/// policy `clausters_core::resample` documents, so the curve at the very start
/// of a take is the same curve the measurement reads there.
fn reconstruct(trace: &Trace, ch: usize, f: usize, frames: usize, out: &mut [f32]) -> usize {
    let filter = resample::FINE;
    let factor = filter.factor().min(out.len());
    if factor == 0 {
        return 0;
    }
    let mut window = [0.0f32; resample::TAPS];
    for (k, slot) in window.iter_mut().enumerate() {
        // The same centred window the core reads: `GUARD - 1` samples before
        // this one, itself, and `GUARD` after.
        let at = f as isize + 1 + k as isize - resample::GUARD as isize;
        *slot = if at < 0 || at as usize >= frames {
            0.0
        } else {
            trace.at(ch, at as f64)
        };
    }
    for (p, slot) in out.iter_mut().enumerate().take(factor) {
        *slot = filter.phase_at(&window, p);
    }
    factor
}

/// **The span a column's level is averaged over**: its own, or the fixed window
/// centred on it where that is wider — the one function both the drawing and
/// the merge test read through, so the level they compare and the level they
/// draw are the same number.
fn level_span(s0: f64, s1: f64, window: f64) -> (f64, f64) {
    if window > s1 - s0 {
        let (c, half) = ((s0 + s1) * 0.5, window * 0.5);
        (c - half, c + half)
    } else {
        (s0, s1)
    }
}

/// **Whether the level has met the envelope**, over the columns on screen: the
/// body's amplitude against the peak's, weighted by the peak so the loud part
/// of the picture decides and silence contributes nothing to either side.
///
/// It is asked of the **whole visible span** rather than per column, because
/// the answer is what the layer *is*, not what one pixel of it looks like:
/// columns near a zero crossing converge on their own at any zoom, and a body
/// blinking out column by column would read as a picture with holes in it
/// rather than as a level that has stopped meaning something.
///
/// A source with no energy answers `false` — nothing merged, there was never a
/// body — and [`draw_body`] skips those columns one at a time, which is the
/// distinction between *not measured* and *measured and redundant*.
// The trace, the channel, the rect and its column geometry, and the two spans a
// level is read over: distinct inputs to one question, clearer flat than bundled
// — the same call `draw_channel` above it takes.
#[allow(clippy::too_many_arguments)]
fn body_merges(
    trace: &Trace,
    ch: usize,
    src: impl Fn(f32) -> f64,
    rect: Rect,
    cols: usize,
    cw: f32,
    per_px: f64,
    window: f64,
) -> bool {
    let (mut level, mut peak) = (0.0f32, 0.0f32);
    for c in 0..cols {
        let x = rect.x + c as f32 * cw;
        let (s0, s1) = (src(x), src(x + cw));
        let (a, b) = level_span(s0, s1, window);
        let Some(ms) = trace.column_ms(ch, per_px, a, b) else {
            continue;
        };
        let (lo, hi) = trace.column(ch, per_px, s0, s1);
        if lo > hi {
            continue;
        }
        level += ms.max(0.0).sqrt();
        peak += lo.abs().max(hi.abs());
    }
    peak > 0.0 && level / peak >= BODY_MERGE_RATIO
}

/// The measured body: one column per pixel at `±sqrt(mean square)` about zero,
/// which is what makes it a *body* rather than a second envelope — level has no
/// sign, so the picture is symmetric by construction and sits inside the
/// envelope of the same span wherever both are drawn.
///
/// **Measured over the column's own samples**, exactly as the envelope above it
/// is — the same group of samples answering two questions, which is what makes
/// the body a reading *of* the envelope rather than a second signal. Every
/// editor draws it this way, and none of them slides a window of its own: a
/// fixed averaging time would smear the level across a transient and, worse,
/// push the body outside the envelope that is supposed to contain it.
///
/// **It goes when it has met the envelope** ([`BODY_MERGE_RATIO`]), which is
/// the other half of the same convention — and it goes at **one weight**, the
/// weight it is drawn at everywhere else, so what a body's weight says is the
/// signal and never the magnification.
#[allow(clippy::too_many_arguments)]
fn draw_body(
    mesh: &mut Mesh,
    rect: Rect,
    trace: &Trace,
    ch: usize,
    src: impl Fn(f32) -> f64,
    y_at: impl Fn(f32) -> f32,
    style: TraceStyle,
    cols: usize,
    cw: f32,
    per_px: f64,
) {
    let color = style.color;
    for c in 0..cols {
        let x = rect.x + c as f32 * cw;
        if style.written.is_some_and(|w| src(x) >= w) {
            break;
        }
        // A source with no measure draws nothing at all — the column is
        // skipped rather than inked at zero, so an old cache shows the
        // envelope it does have and no body it never measured.
        let (a, b) = level_span(src(x), src(x + cw), style.body_window);
        let Some(ms) = trace.column_ms(ch, per_px, a, b) else {
            continue;
        };
        let r = ms.max(0.0).sqrt();
        let (top, bottom) = (y_at(r), y_at(-r));
        // The same two floors the envelope's columns take: the pixel column it
        // fills, so the bodies tile, and the trace's weight, so a stretch the
        // level barely moves in is still drawn.
        let (w, h) = (cw.max(style.width), (bottom - top).abs().max(style.width));
        let (cx, cy) = (x + cw * 0.5, (top + bottom) * 0.5);
        mesh.rect(Rect::new(cx - w * 0.5, cy - h * 0.5, w, h), color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interleaved_column_reads_its_own_channel() {
        // Two channels: channel 0 rises, channel 1 is its negative.
        let samples: Vec<f32> = (0..8).flat_map(|i| [i as f32, -(i as f32)]).collect();
        let trace = Trace::samples(&samples, 2);
        assert_eq!(trace.frames(), 8);
        assert_eq!(trace.channels(), 2);
        assert_eq!(trace.column(0, 4.0, 0.0, 4.0), (0.0, 3.0));
        assert_eq!(trace.column(1, 4.0, 0.0, 4.0), (-3.0, 0.0));
        assert_eq!(trace.at(0, 5.0), 5.0);
        assert_eq!(trace.at(1, 5.0), -5.0);
    }

    #[test]
    fn a_column_landing_on_the_end_is_still_one_sample_wide() {
        let samples = [1.0f32, 2.0, 3.0];
        let trace = Trace::samples(&samples, 1);
        // s0 exactly at the last frame, s1 past it: the span clamps inside the
        // buffer instead of panicking or reading nothing.
        assert_eq!(trace.column(0, 1.0, 3.0, 4.0), (3.0, 3.0));
        assert_eq!(trace.at(0, 99.0), 3.0);
    }

    /// The pyramid arm answers the same envelope as the raw one for a source
    /// that has both — the property that lets a take and an inline body be one
    /// renderer.
    #[test]
    fn the_pyramid_arm_agrees_with_the_raw_arm_on_the_same_signal() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin()).collect();
        let data = WaveformData::new(samples.clone().into(), 256);
        let raw = Trace::samples(&samples, 1);
        let pyr = Trace::Data(&data);
        for c in 0..16 {
            let (s0, s1) = (c as f64 * 256.0, (c + 1) as f64 * 256.0);
            // Below the base bucket both read the raw samples, so they agree
            // exactly; what a wider column reads is the pyramid's own business.
            let a = raw.column(0, 128.0, s0, s0 + 128.0);
            let b = pyr.column(0, 128.0, s0, s0 + 128.0);
            assert_eq!(a, b, "column {c} over [{s0}, {s1})");
        }
    }

    /// **The curve between two samples is the signal's, not the renderer's.**
    /// Zoomed in far enough that the samples are separate points, the drawing
    /// reconstructs the band-limited signal between them -- so a peak that
    /// falls between two samples is *drawn*, where the straight segment this
    /// replaced flattened it. The same fact a true-peak reading reports as a
    /// number.
    #[test]
    fn a_zoomed_in_trace_draws_the_signal_between_its_samples() {
        // The classic inter-sample peak: a sine at a quarter of the sample
        // rate, sampled at 45 degrees. Every sample is at +-1 and the signal
        // between them reaches sqrt(2).
        let samples: Vec<f32> = (0..64)
            .map(|i| if (i / 2) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let mut sub = [0.0f32; resample::MAX_FACTOR];
        let trace = Trace::samples(&samples, 1);
        let n = reconstruct(&trace, 0, 20, samples.len(), &mut sub);
        assert_eq!(n, resample::FINE.factor());
        assert_eq!(sub[0], samples[20], "the first sub-sample is the sample");
        let between = sub.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(
            between > 1.3,
            "the signal between the samples overshoots them: {between}"
        );
    }

    /// **An over between two samples is marked where it is.** Neither sample
    /// left full scale, so a picture of the samples has nothing to show and a
    /// meter watching them has nothing to report -- which is the whole claim a
    /// true-peak reading makes, drawn.
    #[test]
    fn a_peak_between_two_samples_is_marked() {
        // The classic case: every sample at +-1, the signal between them at
        // sqrt(2). Drawn wide enough that the curve regime is entered.
        let samples: Vec<f32> = (0..32)
            .map(|i| if (i / 2) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 640.0, 100.0);
        let n = samples.len() as f64;
        let draw = |over: Color| {
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &trace,
                0,
                |x| (x - rect.x) as f64 / rect.w as f64 * n,
                |s| rect.x + (s / n) as f32 * rect.w,
                |v| rect.y + rect.h * 0.5 * (1.0 - v.clamp(-1.0, 1.0)),
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0)
                    .with_measure(Measure::Signal)
                    .with_overs(over, 1.0),
            );
            mesh.vertex_count()
        };
        let marked = draw([1.0, 0.0, 0.0, 1.0]);
        let plain = draw([0.0, 0.0, 0.0, 0.0]);
        assert!(
            marked > plain,
            "the overs and their figure are drawn: {marked} against {plain}"
        );
    }

    /// **The reconstruction is a layer, never a replacement** -- which is what
    /// keeps a waveform's amplitude from jumping at a zoom threshold. Zoomed
    /// out the `Signal` layer draws nothing and the envelope under it is the
    /// same picture it always was; zoomed in it draws the curve, and the
    /// sample layer keeps its dots and drops only the straight segment the
    /// curve now stands in for.
    #[test]
    fn the_reconstruction_is_a_layer_over_the_samples() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.9).sin()).collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 400.0, 100.0);
        let draw = |frames: f64, measure: Measure, layers: Measures| {
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &trace,
                0,
                |x| (x - rect.x) as f64 / rect.w as f64 * frames,
                |s| rect.x + (s / frames) as f32 * rect.w,
                |v| rect.y + rect.h * 0.5 * (1.0 - v),
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0)
                    .with_dots(2.0)
                    .with_measure(measure)
                    .with_layers(layers),
            );
            mesh.vertex_count()
        };
        let both = Measures::parse("peak signal").unwrap();
        let bare = Measures::of(Measure::Peak);

        // Zoomed out: the envelope is the envelope, whatever is layered on it.
        let wide = 4096.0;
        assert_eq!(
            draw(wide, Measure::Signal, both),
            0,
            "no reconstruction zoomed out"
        );
        assert_eq!(
            draw(wide, Measure::Peak, both),
            draw(wide, Measure::Peak, bare),
            "and the sample picture under it is unchanged"
        );

        // Zoomed in to separate points: the curve is drawn, and the sample
        // layer gives up its straight segments but not its dots.
        let close = 20.0;
        assert!(draw(close, Measure::Signal, both) > 0, "the curve is drawn");
        let dots_only = draw(close, Measure::Peak, both);
        let with_lines = draw(close, Measure::Peak, bare);
        assert!(
            dots_only > 0 && dots_only < with_lines,
            "the dots stay and the segments go: {dots_only} against {with_lines}"
        );
    }

    /// The layer's wire name, and the default a view gets: the envelope with
    /// the reconstruction over it.
    #[test]
    fn the_signal_measure_parses_and_is_on_by_default() {
        assert_eq!(Measure::parse("signal"), Some(Measure::Signal));
        assert_eq!(Measure::Signal.name(), "signal");
        assert_eq!(Measures::default().name(), "peak signal");
        assert!(!Measures::parse("peak").unwrap().has(Measure::Signal));
    }

    /// **The waveform changes character once, where the dots appear.** Above
    /// that zoom a column under two samples wide measures the line between the
    /// samples rather than the band around them, so the picture is one thin
    /// line on both sides of the old crossing; at that zoom the sample regime --
    /// dots, segments, the reconstruction -- begins, all together.
    #[test]
    fn the_picture_changes_where_the_dots_appear_and_nowhere_else() {
        let radius = 3.0f32;
        let at_dots = drawable_per_px(radius);
        assert!(
            samples_are_drawn(at_dots, radius),
            "the dots' own zoom draws them"
        );
        assert!(
            !samples_are_drawn(at_dots * 1.5, radius),
            "and a little further out does not"
        );
        assert!(
            at_dots < LINE_THRESHOLD,
            "the crossing moved in, past the old one"
        );

        // Between the two, a column that straddles a sample is a sliver of the
        // ramp -- half a step either side of it -- and not the two whole steps
        // the ordinary column spans by reading both samples around it.
        let samples: Vec<f32> = (0..400).map(|i| i as f32 / 400.0).collect();
        let trace = Trace::samples(&samples, 1);
        let step = 1.0 / 400.0;
        let (lo, hi) = line_column(&trace, 0, 10.75, 11.25, samples.len());
        assert!(
            (hi - lo - step * 0.5).abs() < step * 0.01,
            "half a sample of the ramp spans half a step: {}",
            hi - lo
        );
        let (lo, hi) = trace.column(0, 0.5, 10.75, 11.25);
        assert!(
            hi - lo >= step * 0.99,
            "where the ordinary column spans the whole step: {}",
            hi - lo
        );
    }

    /// **Never finer than the screen.** The curve is drawn where it can be
    /// seen and the straight segment is kept where it would be the same
    /// picture, which is what keeps a zoomed-out view from paying for
    /// arithmetic nobody can look at.
    #[test]
    fn the_curve_is_drawn_only_where_there_are_pixels_for_it() {
        assert_eq!(curve_step(1.0), None, "samples a pixel apart: a line");
        assert_eq!(curve_step(3.0), None, "one segment is not a curve");
        assert!(curve_step(20.0).is_some(), "far apart: the curve");
        // And the stride thins with the room: wide apart, every sub-sample.
        assert_eq!(curve_step(200.0), Some(1));
        let mid = curve_step(8.0).unwrap();
        assert!(mid > 1, "nearer, every second or fourth: {mid}");
    }

    /// Zoomed out, the trace costs the rect's pixels — not the source's
    /// samples. This is the rule the three implementations each restated.
    #[test]
    fn a_long_source_costs_the_rect_width_not_its_samples() {
        let samples: Vec<f32> = (0..200_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 300.0, 100.0);
        let n = samples.len() as f64;
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * n,
            |s| rect.x + (s / n) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        // Two triangles (six vertices) per column, at most one column per pixel.
        assert!(mesh.vertex_count() <= (rect.w as u32 + 2) * 6);
        assert!(!mesh.is_empty());
    }

    /// The heights of the quads drawn, in order — one per column, six
    /// vertices each.
    fn quad_heights(mesh: &Mesh) -> Vec<f32> {
        let ys: Vec<f32> = mesh.positions().map(|(_, y)| y).collect();
        ys.as_chunks::<6>()
            .0
            .iter()
            .map(|q| {
                let hi = q.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let lo = q.iter().cloned().fold(f32::INFINITY, f32::min);
                hi - lo
            })
            .collect()
    }

    /// Draws `samples` across `rect` at exactly `per_px` samples per pixel,
    /// full scale mapped to the rect — the geometry every view hands the
    /// renderer, with the two maps written out.
    fn draw_at(samples: &[f32], rect: Rect, per_px: f64) -> Mesh {
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &Trace::samples(samples, 1),
            0,
            |x| (x - rect.x) as f64 * per_px,
            |s| rect.x + (s / per_px) as f32,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        mesh
    }

    /// **A one-sample jump is drawn wherever it falls**, including exactly on
    /// a column boundary — the square wave's vertical edge, which is the whole
    /// of the picture at that zoom.
    ///
    /// The columns partition the samples and the curve does not, so before the
    /// join a jump landing between two columns was drawn by neither: this
    /// signal came out as a row of dashes along the top and another along the
    /// bottom, with nothing joining them. Chosen so every transition lands on
    /// a boundary (period 8 at 4 samples per pixel), which is exactly what a
    /// zoom or a scroll arrives at on its own.
    #[test]
    fn a_jump_on_a_column_boundary_is_still_drawn() {
        let samples: Vec<f32> = (0..800)
            .map(|i| if (i % 8) < 4 { 1.0 } else { -1.0 })
            .collect();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mesh = draw_at(&samples, rect, 4.0);
        let heights = quad_heights(&mesh);
        assert_eq!(heights.len(), 100, "one column per pixel");
        // Every column but the first spans the full swing: it holds one level
        // and reaches the other, which is the edge between them.
        let full = heights.iter().filter(|h| **h > rect.h * 0.9).count();
        assert_eq!(full, 99, "the edges are drawn: {heights:?}");
    }

    /// **The lane that raised it, measured again.** A minute of a square wave
    /// across 1250 columns at 2304 samples a column: eleven transitions, of
    /// which one falls exactly on a column boundary, and 1239 columns holding a
    /// constant run.
    ///
    /// Before the join, ten transitions drew as a tall column — a column that
    /// holds both levels *is* the vertical — and the eleventh drew nothing at
    /// all, because each side of it was a separate quad. All eleven draw now,
    /// and the picture no longer changes with where the zoom happens to put a
    /// boundary.
    ///
    /// The flat columns are the other half of what that lane showed, and they
    /// are **not** a defect: a constant run has no height, so it is inked at
    /// the trace's own weight, which is what keeps a flat stretch visible at
    /// all. What the join adds there is that a run of them is *connected* to
    /// what comes before it; what it does not add is slope, because a picture
    /// never resolves finer than the screen and a value that drifts by less
    /// than a pixel across a column has none to draw.
    #[test]
    fn every_transition_of_a_minute_long_square_wave_is_drawn() {
        let (columns, per_px) = (1250usize, 2304.0);
        let n = (columns as f64 * per_px) as usize;
        let period = n / 12; // eleven transitions inside the take
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                if (i / period).is_multiple_of(2) {
                    0.8
                } else {
                    -0.8
                }
            })
            .collect();
        let rect = Rect::new(0.0, 0.0, columns as f32, 100.0);
        let heights = quad_heights(&draw_at(&samples, rect, per_px));
        assert_eq!(heights.len(), columns, "one column per pixel");
        let tall = heights.iter().filter(|h| **h > rect.h * 0.5).count();
        assert_eq!(tall, 11, "every transition draws a vertical: {tall}");
        // And the rest are the constant runs, inked at the trace's weight and
        // no more — the picture is still a measurement of its column.
        let flat = heights.iter().filter(|h| **h <= 1.0 + 1e-3).count();
        assert_eq!(flat, columns - 11, "a constant run has no height to draw");
    }

    /// **A line that drifts by less than a pixel a column is connected**, not
    /// a row of separated bars — the other thing that lane showed.
    ///
    /// A column of a constant run has no height, so it is inked at the trace's
    /// weight and sits where its value is. Two neighbours a fifth of a pixel
    /// apart used to be two such bars with a gap between them wherever the
    /// rounding fell on either side of a pixel; joined, each reaches the one
    /// before it, so the run is one line whatever the drift. What it still does
    /// not have is *slope*: below a pixel a column there is none to draw, and a
    /// picture never resolves finer than the screen.
    #[test]
    fn a_drift_of_less_than_a_pixel_a_column_still_draws_one_line() {
        let (columns, per_px) = (200usize, 64.0);
        let n = (columns as f64 * per_px) as usize;
        let rect = Rect::new(0.0, 0.0, columns as f32, 100.0);
        // Full scale is rect.h / 2 pixels of value, so this drifts a fifth of a
        // pixel per column: two columns are never a pixel apart.
        let step = 0.2 / (rect.h as f64 * 0.5) / per_px;
        let samples: Vec<f32> = (0..n).map(|i| (i as f64 * step) as f32).collect();
        let mesh = draw_at(&samples, rect, per_px);
        let ys: Vec<f32> = mesh.positions().map(|(_, y)| y).collect();
        let spans: Vec<(f32, f32)> = ys
            .as_chunks::<6>()
            .0
            .iter()
            .map(|q| {
                (
                    q.iter().cloned().fold(f32::INFINITY, f32::min),
                    q.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                )
            })
            .collect();
        assert_eq!(spans.len(), columns);
        for (c, pair) in spans.windows(2).enumerate() {
            let (a, b) = (pair[0], pair[1]);
            assert!(
                b.0 <= a.1 + 1e-3 && b.1 >= a.0 - 1e-3,
                "column {c} and {} do not touch: {a:?} then {b:?}",
                c + 1
            );
        }
    }

    /// ...and the join inks **only** the crossing: a column whose neighbour
    /// already overlaps it is drawn exactly as it was measured, which is every
    /// column of ordinary audio.
    #[test]
    fn joining_never_widens_a_column_that_already_overlaps() {
        // Several cycles per column: consecutive columns share nearly the
        // whole swing, so there is no crossing left to draw.
        let samples: Vec<f32> = (0..4000).map(|i| (i as f32 * 0.4).sin()).collect();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let inked = quad_heights(&draw_at(&samples, rect, 40.0));
        let trace = Trace::samples(&samples, 1);
        for (c, h) in inked.iter().enumerate() {
            let (lo, hi) = trace.column(0, 40.0, c as f64 * 40.0, (c + 1) as f64 * 40.0);
            let measured = ((hi - lo) * rect.h * 0.5).max(1.0);
            assert!(
                (h - measured).abs() < 1e-3,
                "column {c} inked {h}, measured {measured}"
            );
        }
    }

    /// The trace over a signal that only ever moves one way: **connected at
    /// every boundary, and never wider than the two columns it joins.**
    ///
    /// A slow ramp is the case the join does real work in at every column —
    /// consecutive ones are disjoint by construction, so each reaches back the
    /// one segment the polyline would draw. What it may never do is
    /// accumulate: the extension is drawn and not remembered, so a run of
    /// columns cannot walk the trace outwards.
    #[test]
    fn a_monotone_signal_is_connected_and_no_wider_than_its_columns() {
        let n = 4000;
        let samples: Vec<f32> = (0..n).map(|i| i as f32 / n as f32 * 2.0 - 1.0).collect();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let per_px = n as f64 / rect.w as f64;
        let mesh = draw_at(&samples, rect, per_px);
        let ys: Vec<f32> = mesh.positions().map(|(_, y)| y).collect();
        let spans: Vec<(f32, f32)> = ys
            .as_chunks::<6>()
            .0
            .iter()
            .map(|q| {
                let hi = q.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                (q.iter().cloned().fold(f32::INFINITY, f32::min), hi)
            })
            .collect();
        let trace = Trace::samples(&samples, 1);
        let measured = |c: usize| {
            let (lo, hi) = trace.column(0, per_px, c as f64 * per_px, (c + 1) as f64 * per_px);
            // Value space to y, top first — the map `draw_at` hands over.
            (rect.h * 0.5 * (1.0 - hi), rect.h * 0.5 * (1.0 - lo))
        };
        for c in 1..spans.len() {
            let (top, bottom) = spans[c];
            let (ptop, pbottom) = spans[c - 1];
            assert!(
                top <= pbottom + 1e-3 && bottom + 1e-3 >= ptop,
                "column {c} left a gap: {:?} after {:?}",
                spans[c],
                spans[c - 1]
            );
            // Inside the two measurements it joins, the trace weight included.
            let (mtop, mbottom) = measured(c);
            let (ptop_m, pbottom_m) = measured(c - 1);
            assert!(
                top >= mtop.min(ptop_m) - 1.0 && bottom <= mbottom.max(pbottom_m) + 1.0,
                "column {c} inked {:?} outside [{}, {}]",
                spans[c],
                mtop.min(ptop_m),
                mbottom.max(pbottom_m)
            );
        }
    }

    /// A column the signal barely moves in is still inked, at the trace's own
    /// weight in **both** directions. It used to be a zero-length line — which
    /// draws nothing at all — so a slow curve faded out exactly where it
    /// flattened: the sustain of an envelope, the tail of a decay. And a column
    /// narrower than the weight read thinner than the polyline the same
    /// function draws a pixel the other side of the threshold. The regime
    /// decides how a signal is resolved, never how heavily it is inked.
    #[test]
    fn a_flat_column_still_inks_the_traces_own_weight() {
        // A constant signal, long enough to be well inside the column regime.
        let samples = vec![0.5f32; 40_000];
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let n = samples.len() as f64;
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * n,
            |s| rect.x + (s / n) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.5),
        );
        // Every column drew: six vertices each, none collapsed away.
        assert_eq!(mesh.vertex_count(), rect.w as u32 * 6);
        // ...and each one is a quad at least the trace's weight both ways: a
        // flat signal over a 200 px rect inks a band 1.5 px thick, not a
        // hairline and not nothing.
        let inked = mesh.extent().expect("the flat signal drew");
        assert!(
            (inked.h - 1.5).abs() < 1e-3,
            "a flat column inks the trace weight vertically, got {}",
            inked.h
        );
        // ...spanning the rect and no more: the outer columns are widened to
        // the weight like every other, and the lane bounds what that widening
        // would have hung over the edge.
        assert!(
            (inked.w - rect.w).abs() < 1e-3,
            "columns span the rect and stop at it, got {}",
            inked.w
        );
    }

    /// **A column is its own envelope, never a fill to the baseline** — the
    /// divergence this closed, and it closed the other way from the first
    /// attempt. The GPU pipeline used to clamp every column to zero; clamping
    /// everywhere would have inked a band the signal was never in.
    ///
    /// The two cases are the whole argument. A signal sitting at +0.8 draws a
    /// thin band at +0.8 whatever its domain says, because that is where it
    /// was; and a signal that swings across zero draws the solid body every
    /// editor draws, because *the data fills it* — no rule, no threshold, and
    /// nothing that has to know the zoom.
    #[test]
    fn a_column_is_its_own_envelope_and_the_data_is_what_fills_it() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let draw = |samples: &[f32], min: f32, max: f32| {
            let trace = Trace::samples(samples, 1);
            let n = samples.len() as f64;
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &trace,
                0,
                |x| (x - rect.x) as f64 / rect.w as f64 * n,
                |s| rect.x + (s / n) as f32 * rect.w,
                move |v| {
                    rect.y + rect.h * (1.0 - crate::host::graphics::meters::fraction(v, min, max))
                },
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
            );
            mesh.extent().expect("the signal drew").h
        };
        // A signal that never comes near zero is a band at its own level, in a
        // bipolar domain exactly as in a unipolar one.
        let offset = vec![0.8f32; 4_000];
        assert!(
            draw(&offset, -1.0, 1.0) < rect.h * 0.05,
            "an offset signal is not filled from the baseline"
        );
        assert!(
            draw(&offset, 0.0, 1.0) < rect.h * 0.05,
            "...and its domain does not change that"
        );
        // A signal that does cross zero fills, and the data is what fills it:
        // every column's own min/max spans the lane.
        let swinging: Vec<f32> = (0..4_000)
            .map(|i| if i % 2 == 0 { 0.9 } else { -0.9 })
            .collect();
        assert!(
            draw(&swinging, -1.0, 1.0) > rect.h * 0.8,
            "audio at overview zoom is the solid body it always was"
        );
    }

    /// A **subsonic** signal is the case that proves the zoom could not have
    /// been the criterion: a cycle a second has far more samples than the
    /// screen has pixels — deep in the column regime — and is a curve, not a
    /// body. Every column is a thin band, and the bands trace the wave.
    #[test]
    fn a_subsonic_signal_draws_a_curve_not_a_body() {
        // One cycle of a 1 Hz sine at 48 kHz: 48000 samples over 100 px.
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 / 48_000.0 * std::f32::consts::TAU).sin())
            .collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let n = samples.len() as f64;
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * n,
            |s| rect.x + (s / n) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        // Well past the polyline threshold — this is the column regime.
        assert!(n / rect.w as f64 > LINE_THRESHOLD * 100.0);
        // The column at the peak barely moves: a thin band near the top, not a
        // slab reaching down to the zero line.
        let peak_col = (rect.w * 0.25) as usize;
        let x = rect.x + peak_col as f32;
        let per_px = n / rect.w as f64;
        let (lo, hi) = trace.column(0, per_px, x as f64 * per_px, (x as f64 + 1.0) * per_px);
        assert!(lo > 0.9 && hi <= 1.0, "the peak column is [{lo}, {hi}]");
        assert!(
            (hi - lo) < 0.05,
            "a slow cycle hardly moves inside one column"
        );
    }

    /// **Sample dots**: once the samples stand far enough apart to read as
    /// separate points, each one is marked. The line between them is an
    /// interpolation the drawing invents — the dot is what says which points of
    /// it are data, and what sample-level editing will take hold of, which is
    /// why it is sized as a curve's break-point.
    #[test]
    fn samples_are_marked_once_they_stand_apart() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let draw = |n: usize, radius: f32| {
            let samples: Vec<f32> = (0..n).map(|i| (i as f32 * 0.3).sin()).collect();
            let trace = Trace::samples(&samples, 1);
            let span = (n - 1) as f64;
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &trace,
                0,
                |x| (x - rect.x) as f64 / rect.w as f64 * span,
                |s| rect.x + (s / span) as f32 * rect.w,
                |v| rect.y + rect.h * 0.5 * (1.0 - v),
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_dots(radius),
            );
            mesh.vertex_count()
        };
        // 20 samples over 200 px: 10 px apart, past three radii — marked.
        let marked = draw(20, 3.0);
        let bare = draw(20, 0.0);
        assert!(
            marked > bare,
            "deep zoom marks each sample: {marked} vs {bare}"
        );
        // 100 samples over the same width: 2 px apart, so a row of dots would
        // just thicken the line. None are drawn -- and the zoom where they do
        // not fit is not yet the sample picture either, so a radius that cannot
        // fit changes nothing at all: two such radii draw the same geometry.
        assert_eq!(
            draw(100, 3.0),
            draw(100, 2.0),
            "dots that would touch are not drawn"
        );
    }

    /// **A sample on a view's right edge is not the view's.** A view is
    /// half-open, so a box cut in two does not dot the second half's first
    /// sample at the end of the first: a view ending exactly on a sample marks
    /// what a view ending just before it marks, and one past it marks one more.
    #[test]
    fn the_sample_on_the_right_edge_is_the_next_views() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let samples: Vec<f32> = (0..16).map(|i| (i as f32 * 0.3).sin()).collect();
        let trace = Trace::samples(&samples, 1);
        let dots = |to: f64| {
            let draw = |radius: f32| {
                let mut mesh = Mesh::new();
                draw_channel(
                    &mut mesh,
                    rect,
                    &trace,
                    0,
                    |x| (x - rect.x) as f64 / rect.w as f64 * to,
                    |s| rect.x + (s / to) as f32 * rect.w,
                    |v| rect.y + rect.h * 0.5 * (1.0 - v),
                    TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_dots(radius),
                );
                mesh.vertex_count()
            };
            draw(3.0) - draw(0.0)
        };
        assert_eq!(
            dots(4.0),
            dots(3.5),
            "samples 0 to 3 either way, and not the one on the edge"
        );
        assert!(
            dots(4.5) > dots(4.0),
            "one sample past the edge adds its dot"
        );
    }

    /// A helper drawing one signal twice, once per measure, over the same maps.
    fn draw_measures(samples: &[f32], rect: Rect, window: (f64, f64)) -> (Mesh, Mesh) {
        draw_measures_at(samples, rect, window, 0.0)
    }

    /// The same, at a source rate — which is what gives the level its fixed
    /// averaging window ([`BODY_WINDOW_SECS`]); `0.0` leaves each column
    /// averaging its own span.
    fn draw_measures_at(
        samples: &[f32],
        rect: Rect,
        window: (f64, f64),
        rate: f64,
    ) -> (Mesh, Mesh) {
        let (start, len) = window;
        let one = |measure: Measure| {
            let trace = Trace::samples(samples, 1);
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &trace,
                0,
                |x| start + (x - rect.x) as f64 / rect.w as f64 * len,
                |s| rect.x + ((s - start) / len) as f32 * rect.w,
                |v| rect.y + rect.h * 0.5 * (1.0 - v),
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0)
                    .with_measure(measure)
                    .with_rate(rate),
            );
            mesh
        };
        (one(Measure::Peak), one(Measure::Rms))
    }

    /// **The body is symmetric about zero, and inside the envelope column by
    /// column.** Both are the measure's own definition rather than a drawing
    /// choice: a level has no sign, so `±sqrt(mean square)` is symmetric by
    /// construction, and the root-mean-square of a span can never exceed the
    /// largest magnitude in it.
    ///
    /// **Column by column is the whole of the claim, and the DC case is why it
    /// is stated that way.** A signal offset from zero has an envelope offset
    /// with it, while the body stays centred on zero — so the body reaches
    /// *below* the lowest sample of an all-positive signal, and the picture is
    /// right: RMS is measured about zero and a level includes the offset. What
    /// is never true is a column whose body leaves its own column's envelope.
    #[test]
    fn the_body_is_symmetric_about_zero_and_inside_each_column() {
        // Asymmetric on purpose: a signal riding a DC offset.
        let samples: Vec<f32> = (0..20_000)
            .map(|i| 0.3 + 0.6 * (i as f32 * 0.05).sin())
            .collect();
        let trace = Trace::samples(&samples, 1);
        let spp = 50.0;
        for c in 0..100 {
            let (s0, s1) = (c as f64 * spp, (c + 1) as f64 * spp);
            let (lo, hi) = trace.column(0, spp, s0, s1);
            let r = trace
                .column_ms(0, spp, s0, s1)
                .expect("samples measure")
                .sqrt();
            assert!(
                r <= lo.abs().max(hi.abs()) + 1e-5,
                "column {c}: rms {r} outside [{lo}, {hi}]"
            );
        }
        // And the drawn body straddles zero, which is what makes it a body.
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let (_, rms) = draw_measures(&samples, rect, (0.0, samples.len() as f64));
        let e = rms.extent().expect("the body drew");
        let centre = e.y + e.h * 0.5;
        assert!(
            (centre - (rect.y + rect.h * 0.5)).abs() < 0.5,
            "the body is centred on zero, at {centre}"
        );
    }

    /// **The body fades out where a column stops holding a meaningful
    /// the envelope has come down onto it.** Two rules, and they are one
    /// picture seen from either side.
    ///
    /// The **window is fixed** ([`BODY_WINDOW_SECS`], 50 ms of the *source*):
    /// a level is an average over a duration, so averaging whatever a pixel
    /// column happens to cover would make the body's own values follow the
    /// zoom, changing over samples that did not change. The values are the
    /// signal's, and they stand still while the view moves.
    ///
    /// What ends it is the **envelope**, which does narrow with the zoom: once
    /// it has come down to within [`BODY_MERGE_RATIO`] of the level there are
    /// no longer two readings, so the body goes — before it can poke out of the
    /// shape that is supposed to contain it. One weight throughout, and a cut,
    /// which is the editors' own answer: Audacity's RMS "will disappear" as you
    /// zoom in.
    #[test]
    fn a_level_is_averaged_over_a_fixed_window_and_goes_when_the_envelope_meets_it() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let rate = 48_000.0;
        // A sine that is loud for its first half and quiet for its second, so
        // the level has something to say the envelope does not.
        let sine: Vec<f32> = (0..2_000_000)
            .map(|i| {
                let a = if i < 1_000_000 { 0.9 } else { 0.2 };
                a * (i as f32 * 0.05).sin()
            })
            .collect();
        let body_top = |len: f64| {
            let (_, rms) = draw_measures_at(&sine, rect, (0.0, len), rate);
            rms.positions()
                .map(|(_, y)| y)
                .fold(f32::INFINITY, f32::min)
        };
        // **The values do not follow the zoom.** Two views of the same stretch,
        // one four times closer: the body's own top is the same pixel, because
        // both averaged the same 50 ms of signal.
        let wide = body_top(400_000.0);
        let close = body_top(100_000.0);
        assert!(
            (wide - close).abs() < 0.5,
            "a level that moves under a zoom is a level of the view: {wide} vs {close}"
        );

        // **And it goes where the envelope meets it.** Zoomed into a fraction
        // of a cycle the column's peak has fallen onto the level, and the body
        // is not drawn at all rather than drawn outside the envelope.
        let (_, near) = draw_measures_at(&sine, rect, (0.0, 400.0), rate);
        assert!(
            near.is_empty(),
            "the level went when it stopped being a second reading"
        );

        // A source with no rate has no fixed window and no zoom of its own to
        // be wrong about: each column averages its own span, as it always did.
        let (_, plain) = draw_measures(&sine, rect, (0.0, 400_000.0));
        assert!(!plain.is_empty());
    }

    /// **A take being written is drawn up to its frontier and no further.**
    /// Past it a recording's buffer holds its own zeros, which are not silence
    /// in the samples but the absence of samples — and the minimum-ink rule
    /// would draw a flat line across them, which is a picture of a stretch that
    /// has not happened yet. Both regimes stop: the columns break at the
    /// frontier, and the polyline's last sample is the last one written.
    #[test]
    fn a_written_frontier_cuts_the_picture_and_leaves_the_axis_empty() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Ten thousand frames of signal in a buffer of forty thousand: the
        // first quarter is the take, the rest is what has not been recorded.
        let mut samples = vec![0.0f32; 40_000];
        for (i, v) in samples.iter_mut().enumerate().take(10_000) {
            *v = (i as f32 * 0.05).sin() * 0.8;
        }
        let right = |written: Option<u64>| {
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &Trace::samples(&samples, 1),
                0,
                |x| (x - rect.x) as f64 / rect.w as f64 * 40_000.0,
                |s| rect.x + (s / 40_000.0) as f32 * rect.w,
                |v| rect.y + rect.h * 0.5 * (1.0 - v),
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_written(written),
            );
            mesh.positions().fold(0.0f32, |a, (x, _)| a.max(x))
        };
        // Uncut, the flat tail is inked to the right edge by the minimum-ink
        // rule -- which is the picture this prop exists to stop drawing.
        assert!(right(None) > 90.0, "{}", right(None));
        // Cut, nothing is drawn past the quarter the take reaches.
        let cut = right(Some(10_000));
        assert!(
            (24.0..=27.0).contains(&cut),
            "the picture stops at the frontier: {cut}"
        );
        // Nothing written yet is nothing drawn, not a line across the buffer.
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &Trace::samples(&samples, 1),
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * 40_000.0,
            |s| rect.x + (s / 40_000.0) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_written(Some(0)),
        );
        assert!(mesh.is_empty());
    }

    /// **A source that cannot measure draws no body at all.** The column is
    /// skipped rather than inked at zero, because zero is silence and a flat
    /// line across samples that is not silent is the one picture worse than no
    /// picture. (The cache that has an envelope and no energy is the real case;
    /// it is asserted where the format lives, in `clausters_core::peaks`. Here
    /// the same `None` arrives from a channel the source does not have.)
    #[test]
    fn a_source_that_cannot_measure_draws_no_body() {
        let samples: Vec<f32> = (0..4_000).map(|i| (i as f32 * 0.1).sin()).collect();
        let trace = Trace::samples(&samples, 1);
        assert_eq!(trace.column_ms(1, 40.0, 0.0, 40.0), None);
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            1,
            |x| (x - rect.x) as f64 / rect.w as f64 * 4_000.0,
            |s| rect.x + (s / 4_000.0) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_measure(Measure::Rms),
        );
        assert!(mesh.is_empty(), "no measure, no body");
    }

    /// The measure a wire name resolves to, and the one it does not: an unknown
    /// value is `None`, which the parse reads as a prop that was not set.
    #[test]
    fn the_measure_names_round_trip() {
        assert_eq!(Measure::parse("peak"), Some(Measure::Peak));
        assert_eq!(Measure::parse("rms"), Some(Measure::Rms));
        assert_eq!(Measure::parse("loudness"), None);
        assert_eq!(Measure::default().name(), "peak");
        assert_eq!(Measure::Rms.name(), "rms");
    }

    /// The rule itself, stated where both renderers read it: three radii of
    /// separation, so a dot has a full diameter of air around it.
    #[test]
    fn dots_need_a_diameter_of_air() {
        assert!(!dots_fit(5.0, 0.0), "no radius, no dots");
        assert!(!dots_fit(8.0, 3.0));
        assert!(dots_fit(9.0, 3.0));
        assert!(dots_fit(40.0, 4.0));
    }

    /// Zoomed in past the threshold, every sample in range is a polyline
    /// vertex — the regime where a pixel-stepped loop would drop samples.
    #[test]
    fn zoomed_in_the_polyline_visits_every_visible_sample() {
        let samples: Vec<f32> = (0..64)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let trace = Trace::samples(&samples, 1);
        // 64 samples over 200 px: well under the threshold. The maps leave a
        // margin on both axes, so every stroke lands inside the lane and this
        // counts segments rather than what the lane's own bound does to the
        // ones on its edge.
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let n = samples.len() as f64;
        let (x0, w) = (rect.x + 2.0, rect.w - 4.0);
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - x0) as f64 / w as f64 * n,
            |s| x0 + (s / n) as f32 * w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v * 0.8),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        // 64 samples -> 63 segments, six vertices each.
        assert_eq!(mesh.vertex_count(), 63 * 6);
    }

    /// **At the deepest zoom the trace still crosses the rect — and stops at
    /// its edges.** A window narrower than one sample sees a single data point:
    /// a renderer that draws only what is inside draws a line arriving at that
    /// point from the left and nothing leaving it (which is what the deleted
    /// pipeline did), and one that draws the neighbours unbounded puts ink on
    /// the ruler beside the view. Both halves are the same assertion here: ink
    /// on both edges, none past them.
    #[test]
    fn the_deepest_zoom_still_enters_and_leaves_the_rect() {
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.3).sin()).collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(10.0, 0.0, 200.0, 100.0);
        // One sample's worth of window, landing between two samples.
        let (start, len) = (500.5f64, 1.0f64);
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| start + (x - rect.x) as f64 / rect.w as f64 * len,
            |s| rect.x + ((s - start) / len) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
        for (x, _) in mesh.positions() {
            left = left.min(x);
            right = right.max(x);
        }
        assert!(
            (left - rect.x).abs() < 0.01,
            "the trace enters at the left edge and no further: {left}"
        );
        assert!(
            (right - (rect.x + rect.w)).abs() < 0.01,
            "the trace leaves at the right edge and no further: {right}"
        );
    }

    /// **The trace bounds itself, on both axes.** A signal that leaves the
    /// vertical window and a window narrower than the samples around it both
    /// reach past the lane, and the drawing is what stops them: a free-standing
    /// view is nobody's content, so no container mask is in force.
    #[test]
    fn the_trace_never_draws_outside_its_lane() {
        // A signal well outside the [-1, 1] the y map below folds into the
        // lane, so the polyline runs off the top and the bottom too.
        let samples: Vec<f32> = (0..64)
            .map(|i| if i % 2 == 0 { 4.0 } else { -4.0 })
            .collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(10.0, 20.0, 200.0, 100.0);
        let (start, len) = (20.0f64, 8.0f64);
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| start + (x - rect.x) as f64 / rect.w as f64 * len,
            |s| rect.x + ((s - start) / len) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_dots(3.0),
        );
        assert!(!mesh.is_empty(), "the trace draws");
        let out = mesh
            .positions()
            .filter(|(x, y)| {
                *x < rect.x - 0.01
                    || *x > rect.x + rect.w + 0.01
                    || *y < rect.y - 0.01
                    || *y > rect.y + rect.h + 0.01
            })
            .count();
        assert_eq!(out, 0, "{out} vertices are drawn outside the lane");
    }

    /// A **cache-only** source — a mapped pyramid with no samples — stays in the
    /// column regime however deep the zoom goes. Asking it for one sample would
    /// read an empty buffer, which is the wave vanishing exactly where the
    /// viewer zoomed in to look at it.
    #[test]
    fn a_source_without_samples_never_enters_the_line_regime() {
        let signal: Vec<f32> = (0..4096)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let data = WaveformData::with_pyramid(
            std::sync::Arc::from([] as [f32; 0]),
            crate::peaks::Pyramid::build(&signal, 256),
        );
        let trace = Trace::Data(&data);
        assert!(!trace.has_raw());
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        // Half a sample per pixel: the line regime, for a source that had one.
        let (start, len) = (100.0f64, 100.0f64);
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| start + (x - rect.x) as f64 / rect.w as f64 * len,
            |s| rect.x + ((s - start) / len) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        // One quad per pixel column, and each one carries the envelope the
        // pyramid still knows about.
        assert_eq!(mesh.vertex_count(), 200 * 6);
    }
}
