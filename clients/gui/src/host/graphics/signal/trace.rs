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
//! split the regimes on the opposite side of the threshold. The pipeline bought
//! nothing for it: its columns were folded on the CPU per pixel, as here, and
//! its vertex count was bounded by the render width, as here. So it is gone,
//! and `crate::waveform` is now the data and the navigation state alone.

use crate::waveform::WaveformData;

use crate::host::layout::Rect;
use crate::host::paint::{Color, Mesh};
use crate::host::theme::Theme;

/// At or below this many samples per pixel the trace is drawn as a polyline
/// through the raw samples rather than as min/max columns.
pub const LINE_THRESHOLD: f64 = 2.0;

/// **How many samples a pixel column must hold for its level to mean
/// anything** — below this the body fades out rather than being drawn.
///
/// The measure itself is exact at any span; what a short span stops being is
/// *informative*. Root-mean-square and peak converge as the window shrinks
/// below one cycle — over a quarter cycle of a bass note the two differ by a
/// factor of `0.9`, so the body simply retraces the envelope it sits in — and
/// on the way there it reads the wave's **phase**, beating against the period
/// in a lattice nobody can interpret. Editors draw the same per-column measure
/// and answer this the same way: Audacity's light-blue RMS "will disappear" as
/// you zoom in, "because there are not enough samples to provide a meaningful
/// average in the region being displayed".
///
/// A cycle of the lowest musical pitches is a few hundred samples at any
/// ordinary rate, which is also the pyramid's own summarizing bucket
/// ([`crate::host::elements::signal::DEFAULT_BASE_BUCKET`]) — the resolution at
/// which the energy was measured in the first place. So that is the floor, and
/// the body fades over the octave below it rather than switching off, since a
/// picture that pops at a zoom step reads as a bug.
pub const RMS_FLOOR: f64 = 256.0;

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
        }
    }

    /// How many channels the source holds.
    pub fn channels(&self) -> usize {
        match self {
            Trace::Samples { channels, .. } => *channels,
            Trace::Data(data) => data.num_channels(),
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
        }
    }

    /// The **mean square** of channel `ch` over the source span `[s0, s1)` —
    /// what a column measures when it measures level rather than extent.
    ///
    /// `None` is a source that cannot answer, never a zero: a pyramid cached
    /// before the statistic existed has an envelope and no energy, and a body
    /// drawn from zeros would claim silence over material that is not silent.
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
        }
    }
}

/// **What a column measures.** The two are pictures of one span, not two
/// sources: `Peak` is the extent the signal reached (the min/max envelope every
/// editor draws), `Rms` the level it held there — the body an editor draws
/// *inside* that envelope. One function draws either, placed once per measure,
/// which is what lets a view show both without a second renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Measure {
    /// The min/max envelope: what the signal reached.
    #[default]
    Peak,
    /// The symmetric body about zero at `sqrt(mean square)`: what it held.
    Rms,
}

impl Measure {
    /// The wire name, or `None` for one this build does not know — which reads
    /// as "the prop was not set" rather than as an error, the protocol's own
    /// posture for an unknown value.
    pub fn parse(name: &str) -> Option<Measure> {
        match name {
            "peak" => Some(Measure::Peak),
            "rms" => Some(Measure::Rms),
            _ => None,
        }
    }

    /// The name this measure answers `/gui_query` with.
    pub fn name(self) -> &'static str {
        match self {
            Measure::Peak => "peak",
            Measure::Rms => "rms",
        }
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
    fn default() -> Self {
        Measures::of(Measure::Peak)
    }
}

impl Measures {
    /// Every measure there is, in drawing order — back to front.
    pub const ALL: [Measure; 2] = [Measure::Peak, Measure::Rms];

    /// The set holding exactly one measure.
    pub fn of(m: Measure) -> Self {
        Measures(1 << m as u8)
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
    /// What each column measures — the envelope, or the level inside it.
    pub measure: Measure,
}

/// Whether sample dots are drawn at `spacing` pixels apart: they need to read
/// as separate points, so a dot is drawn only once its neighbour is three radii
/// away — a full diameter of air between them. Below that the line is the
/// picture and a row of touching dots would just thicken it.
pub fn dots_fit(spacing: f32, radius: f32) -> bool {
    radius > 0.0 && spacing >= 3.0 * radius
}

impl TraceStyle {
    /// A trace inked at `width`, with no sample dots.
    pub fn new(color: Color, width: f32) -> Self {
        TraceStyle {
            color,
            width,
            dot_radius: 0.0,
            measure: Measure::Peak,
        }
    }

    /// The same trace, marking each sample once they are far enough apart.
    pub fn with_dots(mut self, radius: f32) -> Self {
        self.dot_radius = radius;
        self
    }

    /// The same trace, measuring `measure` instead of the envelope.
    pub fn with_measure(mut self, measure: Measure) -> Self {
        self.measure = measure;
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
/// for one sample ([`Trace::has_raw`]) — it draws one min/max column per pixel;
/// at or below it, the polyline through every visible sample **plus the one
/// beyond each edge**, so the segments that cross the edges are drawn and the
/// trace enters and leaves the rect instead of stopping at the last sample it
/// can see.
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
    if style.measure == Measure::Rms {
        draw_body(mesh, rect, trace, ch, &src, &y_at, style, cols, cw, per_px);
        mesh.set_clip(outer);
        return;
    }
    if per_px > LINE_THRESHOLD || !trace.has_raw() {
        for c in 0..cols {
            let x = rect.x + c as f32 * cw;
            let (lo, hi) = trace.column(ch, per_px, src(x), src(x + cw));
            if lo > hi {
                continue;
            }
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
            let (top, bottom) = (y_at(hi), y_at(lo));
            let (w, h) = (cw.max(style.width), (bottom - top).max(style.width));
            let (cx, cy) = (x + cw * 0.5, (top + bottom) * 0.5);
            mesh.rect(Rect::new(cx - w * 0.5, cy - h * 0.5, w, h), style.color);
        }
    } else {
        // Few enough samples per pixel that individual ones matter: step by
        // sample, not by pixel, so nothing visible is skipped.
        let first = src(rect.x).floor().max(0.0) as usize;
        let last = (src(rect.x + rect.w).ceil().max(0.0) as usize).min(frames - 1);
        // Where the samples land decides whether each one is marked: the line
        // is an interpolation the drawing invents, and a dot is what says which
        // points of it are data.
        let spacing = (x_of(1.0) - x_of(0.0)).abs();
        let dots = dots_fit(spacing, style.dot_radius);
        let mut prev: Option<[f32; 2]> = None;
        for f in first..=last.max(first) {
            let p = [x_of(f as f64), y_at(trace.at(ch, f as f64))];
            if let Some(q) = prev {
                mesh.line(q, p, style.width, style.color);
            }
            if dots {
                mesh.disc(p[0], p[1], style.dot_radius, style.color);
            }
            prev = Some(p);
        }
    }
    mesh.set_clip(outer);
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
/// **It fades out where a column stops holding a meaningful average**
/// ([`RMS_FLOOR`]), which is the other half of the same convention.
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
    let fade = (per_px / RMS_FLOOR).clamp(0.0, 1.0) as f32;
    if fade <= 0.0 {
        return;
    }
    let mut color = style.color;
    color[3] *= fade;
    for c in 0..cols {
        let x = rect.x + c as f32 * cw;
        // A source with no measure draws nothing at all — the column is
        // skipped rather than inked at zero, so an old cache shows the
        // envelope it does have and no body it never measured.
        let Some(ms) = trace.column_ms(ch, per_px, src(x), src(x + cw)) else {
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
        // just thicken the line. None are drawn, and the picture is the line
        // it was.
        assert_eq!(
            draw(100, 3.0),
            draw(100, 0.0),
            "dots that would touch are not drawn"
        );
    }

    /// A helper drawing one signal twice, once per measure, over the same maps.
    fn draw_measures(samples: &[f32], rect: Rect, window: (f64, f64)) -> (Mesh, Mesh) {
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
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_measure(measure),
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
    /// average.** The measure stays exact at any span — what a short span stops
    /// being is informative: below a cycle, root-mean-square and peak converge,
    /// so the body retraces the envelope it sits in, and on the way there it
    /// reads the wave's phase rather than its level. Editors answer this the
    /// same way (Audacity's RMS "will disappear" as you zoom in, "because there
    /// are not enough samples to provide a meaningful average"); fading rather
    /// than switching off is so the picture does not pop at a zoom step.
    #[test]
    fn the_body_fades_out_where_a_column_stops_averaging() {
        let samples: Vec<f32> = (0..400_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let alpha = |len: f64| {
            let (_, rms) = draw_measures(&samples, rect, (0.0, len));
            rms.alphas().fold(0.0f32, f32::max)
        };
        // A column holding a full summarizing bucket: the body at full weight.
        assert!(
            (alpha(100.0 * RMS_FLOOR) - 1.0).abs() < 1e-6,
            "{}",
            alpha(100.0 * RMS_FLOOR)
        );
        // A quarter of one: a quarter of the weight, on its way out.
        let quarter = alpha(25.0 * RMS_FLOOR);
        assert!((quarter - 0.25).abs() < 1e-3, "{quarter}");
        // And at sample zoom it is gone, leaving the samples themselves.
        assert!(alpha(50.0) < 0.03, "{}", alpha(50.0));
    }

    /// **A source that cannot measure draws no body at all.** The column is
    /// skipped rather than inked at zero, because zero is silence and a flat
    /// line across material that is not silent is the one picture worse than no
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
