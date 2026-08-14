//! Waveform: the audio-specific **data holder** and the navigation state of a
//! view over it, built on the reusable `viewport::View` and `peaks::Pyramid`.
//!
//! What is *not* here is the drawing. A signal against time is drawn in exactly
//! one place — `host::graphics::signal::trace::draw_channel`, into the window's
//! triangle mesh — and this module is what that renderer reads: per channel, the
//! raw samples (shared, for the zoomed-in regime) plus a peak pyramid (for the
//! zoomed-out one), all sharing the time axis, so an editor-grade view draws
//! stacked lanes or overlaid traces from one [`WaveformData`].
//!
//! [`WaveformData::column`] is the one place the regimes below the screen's
//! resolution are decided: raw samples while they are finer than the pyramid's
//! base bucket, and the pyramid otherwise — **cross-faded** between the two
//! levels adjacent to the zoom, so switching levels never pops.
//!
//! [`WaveformView`] adds what a *navigable* view keeps between frames and the
//! data does not: the vertical (amplitude) window, the value domain, and the
//! drag anchor. Its horizontal window lives in the widget's timeline group.

use std::sync::Arc;

use crate::host::graphics::signal::trace::{self, Trace, TraceStyle};
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Mesh;
use crate::host::theme::Theme;
use crate::peaks::{self, MultiPyramid, Pyramid};
use crate::view::TimelineView;
use crate::viewport::{Axis, Unit, View};

/// One channel's data: its raw samples (possibly empty, for a cache-only view)
/// plus its peak pyramid.
struct Channel {
    samples: Arc<[f32]>,
    pyramid: Pyramid,
}

/// A waveform's data: per channel, the raw samples (shared, for the zoomed-in
/// regimes) plus a peak pyramid (for the zoomed-out regime). The pyramids are
/// the cache that can be persisted via `peaks::MultiPyramid::write_cache`.
pub struct WaveformData {
    channels: Vec<Channel>,
}

/// A summary, not a dump: the data behind a view is megabytes of samples, and it
/// lives inside the widget tree (a `clip` body), which is `Debug`-printed in
/// logs and tests.
impl std::fmt::Debug for WaveformData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaveformData")
            .field("channels", &self.num_channels())
            .field("samples", &self.total_samples())
            .field("raw", &self.has_raw())
            .finish()
    }
}

impl WaveformData {
    /// A mono waveform from `samples`, building its pyramid at `base_bucket`.
    pub fn new(samples: Arc<[f32]>, base_bucket: usize) -> Self {
        let pyramid = Pyramid::build(&samples, base_bucket);
        Self {
            channels: vec![Channel { samples, pyramid }],
        }
    }

    /// A multichannel waveform from `samples` holding `channels` interleaved
    /// channels (a trailing partial frame is ignored), one pyramid per channel.
    pub fn from_interleaved(samples: &[f32], channels: usize, base_bucket: usize) -> Self {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        let built = (0..channels)
            .map(|ch| {
                let one: Vec<f32> = (0..frames).map(|f| samples[f * channels + ch]).collect();
                let samples: Arc<[f32]> = one.into();
                let pyramid = Pyramid::build(&samples, base_bucket);
                Channel { samples, pyramid }
            })
            .collect();
        Self { channels: built }
    }

    /// Build from samples and an already-computed pyramid (e.g. read back from a
    /// cache file with `Pyramid::read_cache`). The samples may be **empty** — a
    /// cache-only view (the bulk path where the host maps just the compact
    /// pyramid, never the raw buffer): it renders the resolution-matched overview
    /// from the pyramid, and the zoomed-in raw-sample regimes simply have nothing
    /// finer to show.
    pub fn with_pyramid(samples: Arc<[f32]>, pyramid: Pyramid) -> Self {
        Self {
            channels: vec![Channel { samples, pyramid }],
        }
    }

    /// A multichannel view from already-split raw channels paired with their
    /// pyramids (e.g. a mapped file whose sibling cache was still valid, so
    /// the pyramids were read back instead of rebuilt). Pairs must agree in
    /// length and bucket; the bulk loader validates before calling.
    pub fn from_parts(parts: Vec<(Arc<[f32]>, Pyramid)>) -> Self {
        assert!(!parts.is_empty());
        let channels = parts
            .into_iter()
            .map(|(samples, pyramid)| Channel { samples, pyramid })
            .collect();
        Self { channels }
    }

    /// A cache-only multichannel view from a mapped [`MultiPyramid`] (no raw
    /// samples; every regime renders from the per-channel pyramids).
    pub fn with_multi_pyramid(multi: MultiPyramid) -> Self {
        let channels = multi
            .into_channels()
            .into_iter()
            .map(|pyramid| Channel {
                samples: Arc::from([] as [f32; 0]),
                pyramid,
            })
            .collect();
        Self { channels }
    }

    /// The buffer length the view spans, in per-channel samples. Taken from the
    /// pyramid (which is built over the whole buffer), so a cache-only view with
    /// no raw `samples` still reports the right length.
    pub fn total_samples(&self) -> usize {
        self.channels[0].pyramid.total_samples()
    }

    /// How many channels this waveform holds.
    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Channel 0's pyramid (the persistable cache of a mono view).
    pub fn pyramid(&self) -> &Pyramid {
        &self.channels[0].pyramid
    }

    /// Whether raw samples are present. A cache-only view (`with_pyramid` with an
    /// empty buffer) has only the peak pyramid, so every regime — including the
    /// zoomed-in ones — must render from it; reading the empty raw buffer would
    /// instead collapse the wave to a flat line (it "disappears" on zoom-in).
    pub fn has_raw(&self) -> bool {
        !self.channels[0].samples.is_empty()
    }

    /// Min/max of channel `ch` for a pixel column spanning `[s0, s1)`, choosing
    /// the cheapest accurate source for the given `samples_per_px`: raw samples
    /// when finer than the pyramid's base bucket, the pyramid otherwise —
    /// **cross-faded** between the two adjacent levels so zooming never pops
    /// when the level selection switches.
    pub fn column(&self, ch: usize, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
        let Some(channel) = self.channels.get(ch) else {
            return (0.0, 0.0);
        };
        let pyramid = &channel.pyramid;
        if samples_per_px < pyramid.base_bucket() as f64 && self.has_raw() {
            let a = (s0.floor().max(0.0) as usize).min(channel.samples.len());
            let b = (s1.ceil() as usize).clamp(a, channel.samples.len());
            peaks::min_max(&channel.samples[a..b]).unwrap_or((0.0, 0.0))
        } else {
            // At or above the base bucket, or whenever there is no raw buffer to
            // resolve finer (a cache-only view): read the pyramid. `level_for`
            // clamps to level 0, so zooming past the cache shows its finest
            // overview rather than collapsing to a flat line.
            level_crossfade(pyramid, samples_per_px, s0, s1)
        }
    }

    /// Single-sample access for the line regime, clamped to bounds. A
    /// cache-only view has no sample to give and answers silence, which is why
    /// the renderer asks [`Self::has_raw`] before entering that regime.
    pub fn samples_at(&self, ch: usize, i: usize) -> f32 {
        self.channels
            .get(ch)
            .and_then(|c| c.samples.get(i))
            .copied()
            .unwrap_or(0.0)
    }

    /// `frames` frames from `start`, **interleaved** — the shape a block of
    /// audio travels in everywhere else in this project, and what a copy puts
    /// on the clipboard.
    ///
    /// `None` for a **cache-only** view: a mapped pyramid has an overview and
    /// no samples, so there is nothing here that could honestly be copied, and
    /// a block of silence is the one answer worse than declining. Clamped at the
    /// end rather than refused, because a selection reaching past the last
    /// sample is an ordinary thing a sweep does.
    pub fn block(&self, start: usize, frames: usize) -> Option<Vec<f32>> {
        if !self.has_raw() {
            return None;
        }
        let channels = self.num_channels().max(1);
        let end = start.saturating_add(frames).min(self.total_samples());
        let start = start.min(end);
        let mut out = Vec::with_capacity((end - start) * channels);
        for f in start..end {
            for ch in 0..channels {
                out.push(self.samples_at(ch, f));
            }
        }
        Some(out)
    }
}

/// A pyramid column blended between the level matching `samples_per_px` and
/// the next coarser one, weighted by the fractional position of the zoom
/// between their bucket sizes (log2). At exactly a level's bucket the blend is
/// pure fine; approaching the next level's bucket it converges to pure coarse
/// — which is where `level_for` switches — so the min/max envelope is
/// continuous across the switch instead of popping.
fn level_crossfade(pyramid: &Pyramid, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
    let level = pyramid.level_for(samples_per_px);
    let (lo, hi) = pyramid.column(level, s0, s1).unwrap_or((0.0, 0.0));
    let Some(bucket) = pyramid.level_bucket(level) else {
        return (lo, hi);
    };
    if samples_per_px <= bucket as f64 || level + 1 >= pyramid.num_levels() {
        return (lo, hi);
    }
    let t = (samples_per_px / bucket as f64).log2().clamp(0.0, 1.0) as f32;
    let (clo, chi) = pyramid.column(level + 1, s0, s1).unwrap_or((lo, hi));
    (lo + (clo - lo) * t, hi + (chi - hi) * t)
}

/// The vertical margin the trace leaves inside its lane: the value domain's
/// full span maps to this fraction of the lane's height. Shared with the
/// amplitude ruler and the cursor readout so a tick labeled 1.0 sits exactly on
/// the trace's full-scale line.
pub(crate) const AMP_MARGIN: f32 = 0.92;

/// The **default value domain** of a trace: full-scale amplitude. An element
/// that names no `min`/`max` is audio, and audio is bipolar about zero.
pub const DEFAULT_DOMAIN: (f32, f32) = (-1.0, 1.0);

/// The **zero line** of a value domain, or `None` when the domain does not
/// straddle it and there is no silence to draw.
///
/// It is a line and nothing more. A column is **never** extended to reach it:
/// the GPU pipeline used to clamp every column to zero and the mesh renderers
/// did not, and closing that divergence the other way — by clamping everywhere
/// — was the wrong half to keep. Filling to the baseline **inks a band the
/// signal was never in**: a column covering three samples that all sit at +0.6
/// is drawn from 0 to 0.6, which is a lie at any zoom where cycles are legible,
/// and it needs a threshold nobody can name to decide where that zoom begins.
///
/// The solid body of an overview needs no rule, because at that zoom the data
/// already fills it: a column summarizing hundreds of samples of audio crosses
/// zero by itself. So the envelope is drawn as it is measured, everywhere, and
/// what changes with the zoom is the signal — not the drawing's mind about it.
///
/// **And the zoom could not have been the criterion anyway.** A subsonic
/// signal — a 1 Hz LFO, a control curve, a long envelope — has far more samples
/// than the screen has pixels at any zoom where a whole cycle is visible, so
/// every "fill once the samples no longer fit" rule fills it; and a cycle a
/// second is a *curve*, which is exactly what a filled body destroys. What
/// separates a body from a curve is whether the signal crosses the span inside
/// one column, and the min/max already answers that — measured, per column, at
/// no cost.
pub fn baseline_of(min: f32, max: f32) -> Option<f32> {
    (min < 0.0 && max > 0.0).then_some(0.0)
}

/// Display coordinate of a value in the domain `[min, max]`: 0 at the lane
/// bottom, 1 at its top, with [`AMP_MARGIN`] of headroom left about the
/// domain's centre. The default domain reduces it to `amp * AMP_MARGIN`
/// mapped about the half-lane, which is what every view drew before a domain
/// could be named.
pub fn value_to_display(v: f32, min: f32, max: f32) -> f64 {
    let (centre, half) = domain_centre_half(min, max);
    ((v - centre) as f64 / half as f64 * AMP_MARGIN as f64) * 0.5 + 0.5
}

/// The inverse of [`value_to_display`] — what the cursor's height names.
pub fn display_to_value(d: f64, min: f32, max: f32) -> f32 {
    let (centre, half) = domain_centre_half(min, max);
    centre + ((d - 0.5) * 2.0 / AMP_MARGIN as f64) as f32 * half
}

/// How much of one lane a unit of value covers, before the vertical window is
/// applied — the resolution the cursor readout rounds to.
pub fn value_per_display(min: f32, max: f32) -> f64 {
    let (_, half) = domain_centre_half(min, max);
    2.0 * half as f64 / AMP_MARGIN as f64
}

/// A domain as its centre and half-span, with a degenerate one (`min == max`,
/// or reversed) widened so nothing divides by zero and the value simply sits
/// in the middle of its lane.
fn domain_centre_half(min: f32, max: f32) -> (f32, f32) {
    let (lo, hi) = (min.min(max), min.max(max));
    let half = ((hi - lo) * 0.5).max(f32::MIN_POSITIVE);
    ((lo + hi) * 0.5, half)
}

/// A `WaveformData` paired with **what a navigable view keeps between frames**:
/// the vertical (amplitude) display window, the value domain the trace is
/// mapped through, and the drag anchor. Nothing here is GPU state — the picture
/// is drawn into the window's mesh by
/// `host::graphics::signal::trace::draw_channel`, like every other signal.
///
/// The horizontal window is deliberately absent: it lives in the widget's
/// timeline group, because a group may span windows while a slot is per window.
pub struct WaveformView {
    data: WaveformData,
    /// The vertical display axis: the visible slice of the value domain,
    /// normalized (`0, 1` = no zoom).
    amp: Axis,
    /// The **value domain** the trace is mapped through — the element's
    /// `min`/`max`, [`DEFAULT_DOMAIN`] when it names none.
    domain: (f32, f32),
    /// The amplitude window's start, snapshotted for absolute drag panning.
    drag_amp_start: f64,
}

impl WaveformView {
    pub fn new(data: WaveformData) -> Self {
        Self {
            data,
            amp: Axis::normalized(Unit::Norm),
            domain: DEFAULT_DOMAIN,
            drag_amp_start: 0.0,
        }
    }

    /// The samples and pyramids behind this view — what the renderer reads.
    pub fn data(&self) -> &WaveformData {
        &self.data
    }

    /// Sets the **value domain** the trace maps through — the element's
    /// `min`/`max`. Left alone it is [`DEFAULT_DOMAIN`], full-scale amplitude,
    /// which is what every view that names no bounds draws at.
    pub fn set_domain(&mut self, min: f32, max: f32) {
        self.domain = (min, max);
    }

    /// The domain in force, which the vertical ruler and the cursor readout
    /// must name the same values through.
    pub fn domain(&self) -> (f32, f32) {
        self.domain
    }

    /// How many channels the underlying data holds (the lane count).
    pub fn num_channels(&self) -> usize {
        self.data.num_channels()
    }

    /// The buffer length the view spans, in per-channel samples.
    pub fn total_samples(&self) -> usize {
        self.data.total_samples()
    }

    /// Sets the visible vertical display window (normalized; clamped) — the
    /// live `y_start`/`y_len` props of the editor-grade widget.
    pub fn set_amp_window(&mut self, start: f64, len: f64) {
        self.amp.set_span(start, len);
    }

    /// The visible vertical display window, as `(start, len)`.
    pub fn amp_window(&self) -> (f64, f64) {
        self.amp.span()
    }
}

/// The y **pixel** a value lands on inside `lane`, through the value `domain`
/// and the visible vertical window `amp` (`(0.0, 1.0)` = the whole axis).
///
/// Display coordinate 0 is the lane *bottom* — the convention the vertical
/// ruler reads too, so a vertical zoom moves the trace and the ticks by exactly
/// the same amount. A value outside the window lands outside the lane, and the
/// mesh's clip rectangle cuts it there.
pub fn value_to_y(v: f32, domain: (f32, f32), amp: (f64, f64), lane: Rect) -> f32 {
    let (y0, y_len) = (amp.0, amp.1.max(crate::viewport::MIN_SPAN));
    let d = value_to_display(v, domain.0, domain.1);
    lane.y + lane.h * (1.0 - ((d - y0) / y_len) as f32)
}

/// **Draws one lane of a navigable waveform** — the whole of what a `waveform`
/// element's picture is, and the same call the demo harness makes.
///
/// It is three coordinate maps handed to the one signal renderer
/// ([`trace::draw_channel`]): `view` places the horizontal window, `domain` and
/// the vertical window `amp` place the values. Nothing else distinguishes a
/// navigable view from a clip's take or a plot's series.
// The lane, the source, the channel and the two axes it is placed on: distinct
// inputs to one drawing pass, clearer flat than bundled — as in `draw_channel`,
// which this hands them to.
#[allow(clippy::too_many_arguments)]
pub fn draw_lane(
    mesh: &mut Mesh,
    lane: Rect,
    trace: &Trace,
    ch: usize,
    view: &View,
    domain: (f32, f32),
    amp: (f64, f64),
    style: TraceStyle,
) {
    let w = lane.w.max(1.0) as f64;
    trace::draw_channel(
        mesh,
        lane,
        trace,
        ch,
        |x| view.start + (x - lane.x) as f64 / w * view.len,
        |s| lane.x + ((s - view.start) / view.len * w) as f32,
        |v| value_to_y(v, domain, amp, lane),
        style,
    );
}

/// The lane one channel of `lanes` occupies inside `body`, stacked top to
/// bottom. Overlaid traces are `lanes == 1`: every channel takes the whole body.
pub fn lane_rect(body: Rect, lanes: usize, ch: usize) -> Rect {
    let lanes = lanes.max(1) as f32;
    let h = body.h / lanes;
    Rect::new(body.x, body.y + ch as f32 * h, body.w, h)
}

impl TimelineView for WaveformView {
    fn total_samples(&self) -> usize {
        self.data.total_samples()
    }

    fn mesh(&self, mesh: &mut Mesh, rect: Rect, view: &View, m: &Metrics, theme: &Theme) {
        let lanes = self.num_channels();
        let trace = Trace::Data(&self.data);
        for ch in 0..lanes {
            draw_lane(
                mesh,
                lane_rect(rect, lanes, ch),
                &trace,
                ch,
                view,
                self.domain,
                self.amp.span(),
                TraceStyle::new(theme.series(ch), m.trace_w).with_dots(m.point_radius),
            );
        }
    }

    fn on_vertical_zoom(&mut self, factor: f64, anchor: f64) -> bool {
        self.amp.zoom(factor, anchor);
        true
    }

    fn on_vertical_drag_begin(&mut self) {
        self.drag_amp_start = self.amp.start();
    }

    fn on_vertical_drag(&mut self, total: f64) -> bool {
        // Dragging down (total > 0) moves the window down with the cursor.
        // Absolute from the snapshot.
        self.amp
            .set_start(self.drag_amp_start + total * self.amp.len());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An alternating +/-0.5 signal: every base bucket has min -0.5, max +0.5.
    fn envelope_signal(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect()
    }

    #[test]
    fn cache_only_view_resolves_zoom_in_from_the_pyramid() {
        // Cache-only: no raw samples, only the pyramid (the bulk `cache=` path).
        let pyramid = Pyramid::build(&envelope_signal(4096), 256);
        let data = WaveformData::with_pyramid(Arc::from([] as [f32; 0]), pyramid);
        assert!(!data.has_raw());
        // Zoomed in past the base bucket (spp < 256): the raw regime would read
        // the empty buffer and collapse to (0, 0) — the disappearing wave. The
        // fallback reads the pyramid's finest level, so the envelope survives.
        let (lo, hi) = data.column(0, 8.0, 0.0, 8.0);
        assert!(
            lo <= -0.4 && hi >= 0.4,
            "cache-only zoom-in should show the pyramid envelope, got ({lo}, {hi})"
        );
    }

    #[test]
    fn raw_view_still_uses_raw_samples_when_zoomed_in() {
        let data = WaveformData::new(Arc::from(envelope_signal(4096)), 256);
        assert!(data.has_raw());
        let (lo, hi) = data.column(0, 8.0, 0.0, 8.0);
        assert!(
            lo <= -0.4 && hi >= 0.4,
            "raw zoom-in lost the signal: ({lo}, {hi})"
        );
    }

    #[test]
    fn interleaved_channels_split_and_share_the_time_axis() {
        // Stereo: channel 0 the envelope, channel 1 silence.
        let inter: Vec<f32> = envelope_signal(2048)
            .into_iter()
            .flat_map(|s| [s, 0.0])
            .collect();
        let data = WaveformData::from_interleaved(&inter, 2, 64);
        assert_eq!(data.num_channels(), 2);
        assert_eq!(data.total_samples(), 2048, "frames, not flat samples");
        let (lo0, hi0) = data.column(0, 128.0, 0.0, 128.0);
        assert!(lo0 <= -0.4 && hi0 >= 0.4, "channel 0 keeps the envelope");
        let (lo1, hi1) = data.column(1, 128.0, 0.0, 128.0);
        assert_eq!((lo1, hi1), (0.0, 0.0), "channel 1 is silent");
        // An out-of-range channel reads zero instead of panicking.
        assert_eq!(data.column(5, 128.0, 0.0, 128.0), (0.0, 0.0));
    }

    #[test]
    fn cache_only_multichannel_view_reads_every_lane() {
        let inter: Vec<f32> = envelope_signal(2048)
            .into_iter()
            .flat_map(|s| [s, s * 0.5])
            .collect();
        let multi = MultiPyramid::build_interleaved(&inter, 2, 64);
        let data = WaveformData::with_multi_pyramid(multi);
        assert_eq!(data.num_channels(), 2);
        assert!(!data.has_raw());
        let (_, hi0) = data.column(0, 8.0, 0.0, 64.0);
        let (_, hi1) = data.column(1, 8.0, 0.0, 64.0);
        assert!(hi0 >= 0.4 && (0.2..0.4).contains(&hi1));
    }

    /// The lane the vertical-mapping tests measure against: 100 px tall, so a
    /// display coordinate reads straight off the y in percent from the bottom.
    const LANE: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 200.0,
        h: 100.0,
    };

    #[test]
    fn amp_window_maps_the_trace_through_the_visible_slice() {
        // Full axis: the classic margin map — full scale stops AMP_MARGIN of
        // the way to the top, and silence sits on the middle line.
        let top = value_to_y(1.0, DEFAULT_DOMAIN, (0.0, 1.0), LANE);
        assert!(
            (top - 100.0 * (1.0 - (1.0 + AMP_MARGIN) / 2.0)).abs() < 1e-4,
            "{top}"
        );
        assert!((value_to_y(0.0, DEFAULT_DOMAIN, (0.0, 1.0), LANE) - 50.0).abs() < 1e-4);
        // Zoomed into the top half: the zero line sits on the lane's bottom
        // edge and full scale inside the lane, above the middle.
        assert!((value_to_y(0.0, DEFAULT_DOMAIN, (0.5, 0.5), LANE) - 100.0).abs() < 1e-4);
        let full = value_to_y(1.0, DEFAULT_DOMAIN, (0.5, 0.5), LANE);
        assert!((0.0..50.0).contains(&full), "{full}");
        // A value below the window leaves the lane (the clip rect cuts it).
        assert!(value_to_y(-1.0, DEFAULT_DOMAIN, (0.5, 0.5), LANE) > 100.0);
    }

    /// A named domain is the *same* map over another range: its ends land where
    /// full scale lands on the amplitude axis, so the margin is a property of
    /// the lane and not of what the signal happens to measure.
    #[test]
    fn a_named_domain_maps_its_ends_where_full_scale_maps() {
        for (min, max) in [(0.0f32, 1.0f32), (-0.25, 0.75), (20.0, 20_000.0)] {
            for (v, amp) in [(min, -1.0f32), (max, 1.0)] {
                let named = value_to_display(v, min, max);
                let default = value_to_display(amp, DEFAULT_DOMAIN.0, DEFAULT_DOMAIN.1);
                assert!(
                    (named - default).abs() < 1e-9,
                    "[{min}, {max}] end {v} at {named}, full scale at {default}"
                );
            }
            // ...and the inverse names it back, which is what the readout does.
            let mid = (min + max) * 0.5;
            let back = display_to_value(value_to_display(mid, min, max), min, max);
            assert!(
                (back - mid).abs() <= (max - min).abs() * 1e-6,
                "{back} {mid}"
            );
        }
    }

    /// A degenerate domain divides by nothing and parks the value mid-lane,
    /// rather than producing a NaN the vertex buffer would carry to the GPU.
    #[test]
    fn a_degenerate_domain_is_finite() {
        let d = value_to_display(3.0, 3.0, 3.0);
        assert!(d.is_finite(), "{d}");
        assert!(value_to_y(3.0, (3.0, 3.0), (0.0, 1.0), LANE).is_finite());
    }

    /// The fill rule, which the three renderers now share: a domain straddling
    /// zero has a baseline (audio is a deviation from silence), one that does
    /// not is drawn as its own envelope (an envelope, an automation, a
    /// unipolar take).
    #[test]
    fn only_a_domain_that_straddles_zero_has_a_baseline() {
        assert_eq!(baseline_of(-1.0, 1.0), Some(0.0));
        assert_eq!(baseline_of(-0.25, 0.75), Some(0.0));
        assert_eq!(
            baseline_of(0.0, 1.0),
            None,
            "unipolar: no baseline to fill to"
        );
        assert_eq!(baseline_of(20.0, 20_000.0), None, "an offset quantity");
        assert_eq!(baseline_of(-1.0, 0.0), None, "wholly negative");
    }

    #[test]
    fn lod_crossfade_is_continuous_across_a_level_switch() {
        // A signal whose envelope shrinks with time makes adjacent pyramid
        // levels disagree, so a hard level switch would jump. Sample the column
        // just below and just above the switch point (spp = 2 * base_bucket):
        // the cross-faded values must be close.
        let n = 65536;
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                let env = 1.0 - i as f32 / n as f32;
                if i % 2 == 0 { env } else { -env }
            })
            .collect();
        let data = WaveformData::new(Arc::from(samples), 64);
        let (s0, s1) = (40_000.0, 40_256.0);
        let switch = 128.0; // 2 * base_bucket: level_for flips from 0 to 1 here
        let (lo_a, hi_a) = data.column(0, switch - 1e-3, s0, s1);
        let (lo_b, hi_b) = data.column(0, switch + 1e-3, s0, s1);
        assert!(
            (lo_a - lo_b).abs() < 1e-3 && (hi_a - hi_b).abs() < 1e-3,
            "envelope must be continuous at the level switch: ({lo_a},{hi_a}) vs ({lo_b},{hi_b})"
        );
        // And in between the blend moves monotonically toward the coarse level.
        let (_, hi_mid) = data.column(0, 64.0 * 1.5, s0, s1);
        let (_, hi_fine) = data.column(0, 64.0 + 1e-3, s0, s1);
        assert!(
            hi_mid >= hi_fine - 1e-6,
            "blend widens toward the coarse level"
        );
    }
}
