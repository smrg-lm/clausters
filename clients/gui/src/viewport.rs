//! Coordinate systems: a bounded domain seen through a window.
//!
//! Two types, one of which is the other's raw material. A [`View`] is the
//! **window** — a `start` and a `len` in `f64`, so deep zoom stays precise over
//! multi-million-sample buffers — and zoom and pan are pure transforms on it
//! that never touch the data, which is what makes navigation independent of
//! buffer length.
//!
//! An [`Axis`] is that window **plus what bounds it**, plus what its numbers
//! mean ([`Unit`]), how it may move ([`Policy`]) and how much of its domain
//! exists at all ([`Reach`]). The difference is not cosmetic: `View`'s
//! transforms each take the domain's `total` as an argument, so the bound is
//! something every call site carries, and this crate ended up with two parallel
//! spellings of one navigation — `View` over a sample count, and free
//! `clamp_span`/`zoom_span` functions over a normalized `[0, 1]`, the same
//! arithmetic with a different bound and a different floor. An axis owns its
//! bound, so there is one implementation, and a container can hand one to
//! whatever it draws instead of each widget re-deriving a range and a clamp.
//!
//! Renderer-agnostic by construction, so the waveform, the spectrogram and
//! everything the element library grows share the exact same behaviour.

/// The visible window into a buffer, in source-sample units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// First visible sample (fractional), >= 0.
    pub start: f64,
    /// Visible length in samples, >= 1.
    pub len: f64,
}

impl View {
    /// A view spanning the whole buffer.
    pub fn full(total: usize) -> Self {
        Self {
            start: 0.0,
            len: (total.max(1)) as f64,
        }
    }

    /// How many source samples map onto one rendered pixel. This is the single
    /// number that drives peak analysis: the renderer must never resolve the
    /// signal finer than this, and the peak pyramid is selected to match it.
    pub fn samples_per_px(&self, render_width_px: u32) -> f64 {
        (self.len / render_width_px.max(1) as f64).max(f64::MIN_POSITIVE)
    }

    /// Zoom by `factor` (<1 zooms in) keeping the sample under `anchor`
    /// (0..1 across the window) fixed, then clamp to the buffer bounds.
    pub fn zoom(&mut self, factor: f64, anchor: f64, total: usize) {
        let pivot = self.start + self.len * anchor;
        let new_len = (self.len * factor).clamp(1.0, total.max(1) as f64);
        self.start = pivot - new_len * anchor;
        self.len = new_len;
        self.clamp(total);
    }

    /// Pan by `dx` as a fraction of the window width (drag-to-scroll).
    pub fn pan(&mut self, dx: f64, total: usize) {
        self.start += dx * self.len;
        self.clamp(total);
    }

    /// Set the window start (clamped). Used for *absolute* drag panning: the
    /// caller recomputes `start` from a snapshot taken at mouse-down plus the
    /// total cursor displacement, so hitting a bound never accumulates drift and
    /// the view re-aligns with the cursor exactly when it returns.
    pub fn set_start(&mut self, start: f64, total: usize) {
        self.start = start;
        self.clamp(total);
    }

    fn clamp(&mut self, total: usize) {
        let total = total.max(1) as f64;
        if self.len > total {
            self.len = total;
        }
        self.start = self.start.clamp(0.0, (total - self.len).max(0.0));
    }
}

/// The narrowest a normalized display-axis window may get — the vertical-zoom
/// floor of the editor views' y axes (amplitude, frequency), which navigate in
/// display units `[0, 1]` rather than samples.
pub const MIN_SPAN: f64 = 1e-3;

/// What an axis's numbers mean. Names the *domain*, not the widget: two views
/// measuring amplitude share [`Unit::Norm`] whatever they are.
///
/// This is the ruler vocabulary (`host::widget::Ruler`/`RulerY`) promoted to
/// where it belongs — a unit is a property of the axis, and the ruler strip is
/// one thing that reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Buffer or timeline samples.
    Samples,
    /// Clock time.
    Seconds,
    /// Musical time on a client's beat grid.
    Beats,
    /// Normalized amplitude.
    Norm,
    /// Amplitude in dBFS.
    Db,
    /// Integer sample values at some bit depth.
    Bits,
    /// Amplitude as a percentage of full scale.
    Percent,
    /// Frequency in hertz.
    Hz,
    /// MIDI pitch.
    Pitch,
    /// Logical pixels — a layout or workspace plane.
    Pixels,
}

/// How an axis's window may move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The axis navigates alone.
    Free,
    /// The window is shared with every axis naming the same group: a gesture on
    /// any member moves all of them.
    Linked(i32),
    /// The window never moves (a value range the script fixed).
    Fixed,
}

/// Whether the whole domain can be reached, or only its newest end.
///
/// This is what settles navigability, and it is a property of the **axis**
/// rather than of the presentation: a live spectrum's frequency axis is
/// [`Reach::Whole`] and can be zoomed into now, while a live scope's time axis
/// is [`Reach::Newest`] until something retains a history, because there is no
/// past to address. Consumed when the signal element grows its live sources;
/// carried here so the two cases are one type from the start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Every point of the domain exists (a buffer, a value range, a spectrum).
    Whole,
    /// Only the newest end is retained (a tap ring), so the window sits there.
    Newest,
}

/// A bounded domain seen through a window: **the** coordinate system primitive.
///
/// [`View`] is the window; an axis is the window *plus what bounds it*. That
/// difference is the whole point: `View`'s `zoom`/`pan`/`set_start` each take
/// the domain's `total` as an argument, so every call site has to remember it
/// and they disagree in interesting ways — this crate carried two parallel
/// spellings of the same navigation, `View` over a sample count and the free
/// `clamp_span`/`zoom_span` over a normalized `[0, 1]`, which are the same
/// arithmetic with different bounds and different floors. An axis owns its
/// bound, so there is one implementation and one floor rule.
///
/// It also owns its [`Unit`], its [`Policy`] and its [`Reach`], which is what
/// lets a container hand an axis to whatever it draws instead of every widget
/// re-deriving a range, a link and a clamp of its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    window: View,
    /// Where the domain starts in its own units. Zero for a count or a
    /// normalized axis; a value axis genuinely begins somewhere else (a
    /// bipolar range at `-1`, a pitch window at its lowest note).
    origin: f64,
    /// The domain's full length in its own units; the window lives in
    /// `[origin, origin + extent]`.
    extent: f64,
    /// The narrowest the window may get — one sample on a counted domain, the
    /// zoom floor on a normalized one.
    min_len: f64,
    pub unit: Unit,
    pub policy: Policy,
    pub reach: Reach,
}

impl Axis {
    /// An axis over a **normalized** `[0, 1]` domain, fully open — the display
    /// axes (a waveform's amplitude, a spectrogram's frequency), whose geometry
    /// is a fraction of the screen rather than a count.
    pub fn normalized(unit: Unit) -> Self {
        Self {
            window: View {
                start: 0.0,
                len: 1.0,
            },
            origin: 0.0,
            extent: 1.0,
            min_len: MIN_SPAN,
            unit,
            policy: Policy::Free,
            reach: Reach::Whole,
        }
    }

    /// An axis over a **counted** domain of `total` units, fully open — a
    /// buffer's samples, a timeline's span. The floor is one unit.
    pub fn counted(total: usize, unit: Unit) -> Self {
        Self {
            window: View::full(total),
            origin: 0.0,
            extent: total.max(1) as f64,
            min_len: 1.0,
            unit,
            policy: Policy::Free,
            reach: Reach::Whole,
        }
    }

    /// An axis over an arbitrary value range `[min, max]`, fully open — a
    /// break-point function's values, a scope's signal range, a roll's pitch
    /// window. The window is in the range's own units.
    pub fn ranged(min: f64, max: f64, unit: Unit) -> Self {
        let (lo, hi) = (min.min(max), min.max(max));
        let extent = (hi - lo).max(f64::MIN_POSITIVE);
        Self {
            window: View {
                start: lo,
                len: extent,
            },
            origin: lo,
            extent,
            min_len: extent * MIN_SPAN,
            unit,
            policy: Policy::Free,
            reach: Reach::Whole,
        }
    }

    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_reach(mut self, reach: Reach) -> Self {
        self.reach = reach;
        self
    }

    /// Whether navigating this axis means anything: a window can only be moved
    /// over a domain that is there to be addressed.
    pub fn is_navigable(&self) -> bool {
        self.policy != Policy::Fixed && self.reach == Reach::Whole
    }

    /// The group this axis's window is shared with, if any.
    pub fn group(&self) -> Option<i32> {
        match self.policy {
            Policy::Linked(id) => Some(id),
            _ => None,
        }
    }

    pub fn window(&self) -> View {
        self.window
    }

    pub fn start(&self) -> f64 {
        self.window.start
    }

    pub fn len(&self) -> f64 {
        self.window.len
    }

    pub fn extent(&self) -> f64 {
        self.extent
    }

    /// The window as the `(start, len)` pair the wire and the shaders carry.
    pub fn span(&self) -> (f64, f64) {
        (self.window.start, self.window.len)
    }

    /// The domain's own bounds, `(origin, origin + extent)`.
    pub fn bounds(&self) -> (f64, f64) {
        (self.origin, self.origin + self.extent)
    }

    /// Re-bounds the domain, keeping the window inside it (a buffer was
    /// replaced, a timeline grew).
    pub fn set_extent(&mut self, extent: f64) {
        self.extent = extent.max(f64::MIN_POSITIVE);
        self.clamp();
    }

    /// Sets the window, clamped into the domain. A non-positive length opens
    /// the axis fully — the wire's way of asking for a default it has no number
    /// for, the same shape a `scroll`'s zoom uses.
    pub fn set_span(&mut self, start: f64, len: f64) {
        if len <= 0.0 {
            self.window = View {
                start: self.origin,
                len: self.extent,
            };
            return;
        }
        self.window = View { start, len };
        self.clamp();
    }

    /// Zoom by `factor` (< 1 zooms in) keeping the point under `anchor`
    /// (0..1 across the window) fixed.
    pub fn zoom(&mut self, factor: f64, anchor: f64) {
        let pivot = self.window.start + self.window.len * anchor;
        self.window.len = (self.window.len * factor).clamp(self.min_len, self.extent);
        self.window.start = pivot - self.window.len * anchor;
        self.clamp();
    }

    /// Pan by `dx` as a fraction of the window width.
    pub fn pan(&mut self, dx: f64) {
        self.window.start += dx * self.window.len;
        self.clamp();
    }

    /// Set the window start (clamped) — absolute drag panning from a snapshot,
    /// so a clamped edge never accumulates drift.
    pub fn set_start(&mut self, start: f64) {
        self.window.start = start;
        self.clamp();
    }

    /// Where a domain value falls across the visible window, `0` at its start
    /// and `1` at its end — the mapping an axis *is*.
    pub fn fraction_of(&self, value: f64) -> f64 {
        (value - self.window.start) / self.window.len.max(f64::MIN_POSITIVE)
    }

    /// The domain value at `fraction` across the window (the inverse).
    pub fn value_at(&self, fraction: f64) -> f64 {
        self.window.start + fraction * self.window.len
    }

    fn clamp(&mut self) {
        self.window.len = self
            .window
            .len
            .clamp(self.min_len.min(self.extent), self.extent);
        self.window.start = self
            .window
            .start
            .clamp(0.0, (self.extent - self.window.len).max(0.0));
    }
}

/// Clamps a normalized display window to `[0, 1]`: the length into
/// `[MIN_SPAN, 1]`, the start so the window stays inside the axis.
pub fn clamp_span(start: f64, len: f64) -> (f64, f64) {
    let len = len.clamp(MIN_SPAN, 1.0);
    (start.clamp(0.0, 1.0 - len), len)
}

/// Anchor-preserving zoom of a normalized `[0, 1]` display window — the same
/// math as [`View::zoom`], in display units: scale `len` by `factor` (<1
/// zooms in) keeping the point under `anchor` (0 = bottom, 1 = top) fixed,
/// then clamp to the axis.
pub fn zoom_span(start: f64, len: f64, factor: f64, anchor: f64) -> (f64, f64) {
    let pivot = start + len * anchor;
    let new_len = (len * factor).clamp(MIN_SPAN, 1.0);
    clamp_span(pivot - new_len * anchor, new_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_spans_buffer() {
        let v = View::full(1000);
        assert_eq!(v.start, 0.0);
        assert_eq!(v.len, 1000.0);
        assert_eq!(v.samples_per_px(500), 2.0);
    }

    #[test]
    fn zoom_keeps_anchor_sample_fixed() {
        let total = 1000;
        let mut v = View::full(total);
        // Anchor at the centre; the sample there must stay put across a zoom-in.
        let anchor = 0.5;
        let before = v.start + v.len * anchor;
        v.zoom(0.5, anchor, total);
        let after = v.start + v.len * anchor;
        assert!((before - after).abs() < 1e-9);
        assert_eq!(v.len, 500.0);
    }

    #[test]
    fn zoom_clamps_to_bounds() {
        let total = 1000;
        let mut v = View::full(total);
        // Zooming out past the buffer cannot exceed it nor go negative.
        v.zoom(4.0, 0.5, total);
        assert_eq!(v.len, 1000.0);
        assert_eq!(v.start, 0.0);
    }

    #[test]
    fn zoom_span_keeps_the_anchor_point_fixed_and_clamps() {
        let (start, len) = zoom_span(0.0, 1.0, 0.5, 0.75);
        // The display point at 0.75 stays put across the zoom-in.
        assert!((start + len * 0.75 - 0.75).abs() < 1e-12);
        assert_eq!(len, 0.5);
        // Zooming out past the axis clamps to the full window.
        assert_eq!(zoom_span(start, len, 4.0, 0.5), (0.0, 1.0));
        // The zoom-in floor.
        let (_, tiny) = zoom_span(0.4, MIN_SPAN, 0.1, 0.5);
        assert_eq!(tiny, MIN_SPAN);
    }

    #[test]
    fn clamp_span_keeps_the_window_inside_the_axis() {
        assert_eq!(clamp_span(0.9, 0.5), (0.5, 0.5));
        assert_eq!(clamp_span(-0.2, 0.5), (0.0, 0.5));
        assert_eq!(clamp_span(0.5, 2.0), (0.0, 1.0));
    }

    /// The milestone's contract: an axis over a counted domain does exactly
    /// what `View` plus a `total` argument did. Same zooms, same pans, same
    /// clamps — so replacing the call sites cannot move anything.
    #[test]
    fn a_counted_axis_navigates_exactly_like_a_view() {
        let total = 1000usize;
        let mut view = View::full(total);
        let mut axis = Axis::counted(total, Unit::Samples);
        let steps: [(f64, f64); 6] = [
            (0.5, 0.5),
            (0.5, 0.0),
            (0.25, 1.0),
            (8.0, 0.5),
            (0.1, 0.3),
            (100.0, 0.9),
        ];
        for (factor, anchor) in steps {
            view.zoom(factor, anchor, total);
            axis.zoom(factor, anchor);
            assert_eq!((axis.start(), axis.len()), (view.start, view.len));
        }
        for dx in [0.25, -3.0, 10.0, -0.5] {
            view.pan(dx, total);
            axis.pan(dx);
            assert_eq!((axis.start(), axis.len()), (view.start, view.len));
        }
        for start in [-50.0, 0.0, 400.0, 99_999.0] {
            view.set_start(start, total);
            axis.set_start(start);
            assert_eq!((axis.start(), axis.len()), (view.start, view.len));
        }
    }

    /// The other half: a normalized axis does exactly what the free
    /// `clamp_span`/`zoom_span` pair did for the vertical display windows.
    #[test]
    fn a_normalized_axis_navigates_exactly_like_the_span_helpers() {
        let mut axis = Axis::normalized(Unit::Norm);
        let mut span = (0.0f64, 1.0f64);
        for (factor, anchor) in [
            (0.5, 0.75),
            (0.5, 0.0),
            (4.0, 0.5),
            (0.01, 0.2),
            (0.001, 0.5),
        ] {
            span = zoom_span(span.0, span.1, factor, anchor);
            axis.zoom(factor, anchor);
            assert_eq!(axis.span(), span, "zoom {factor} @ {anchor}");
        }
        for (start, len) in [(0.9, 0.5), (-0.2, 0.5), (0.5, 2.0), (0.25, 0.25)] {
            span = clamp_span(start, len);
            axis.set_span(start, len);
            assert_eq!(axis.span(), span, "set {start}, {len}");
        }
    }

    /// A non-positive length opens the axis, which is how the wire asks for a
    /// default it has no number for (`EditorProps::y_view`'s rule).
    #[test]
    fn a_non_positive_length_opens_the_axis() {
        let mut axis = Axis::normalized(Unit::Hz);
        axis.set_span(0.3, 0.2);
        axis.set_span(0.3, 0.0);
        assert_eq!(axis.span(), (0.0, 1.0));
        let mut counted = Axis::counted(500, Unit::Samples);
        counted.set_span(100.0, 50.0);
        counted.set_span(100.0, -1.0);
        assert_eq!(counted.span(), (0.0, 500.0));
    }

    #[test]
    fn an_axis_maps_a_value_to_the_window_and_back() {
        let mut axis = Axis::counted(1000, Unit::Samples);
        axis.set_span(200.0, 400.0);
        assert!((axis.fraction_of(200.0) - 0.0).abs() < 1e-12);
        assert!((axis.fraction_of(600.0) - 1.0).abs() < 1e-12);
        assert!((axis.fraction_of(400.0) - 0.5).abs() < 1e-12);
        for f in [0.0, 0.25, 0.5, 1.0] {
            assert!((axis.fraction_of(axis.value_at(f)) - f).abs() < 1e-12);
        }
    }

    /// Navigability is the axis's, not the presentation's: a fixed range and a
    /// forward-only source are the two things that make a window meaningless.
    #[test]
    fn navigability_belongs_to_the_axis() {
        assert!(Axis::normalized(Unit::Hz).is_navigable());
        assert!(
            !Axis::normalized(Unit::Hz)
                .with_policy(Policy::Fixed)
                .is_navigable()
        );
        assert!(
            !Axis::counted(1000, Unit::Samples)
                .with_reach(Reach::Newest)
                .is_navigable(),
            "a tap ring keeps no past to address"
        );
        assert_eq!(
            Axis::counted(10, Unit::Samples)
                .with_policy(Policy::Linked(7))
                .group(),
            Some(7)
        );
    }

    /// A ranged axis carries the value domain the widgets each re-derived.
    #[test]
    fn a_ranged_axis_spans_its_value_range() {
        let axis = Axis::ranged(-1.0, 1.0, Unit::Norm);
        assert_eq!(axis.extent(), 2.0);
        assert_eq!(axis.bounds(), (-1.0, 1.0));
        assert_eq!(axis.span(), (-1.0, 2.0));
        // The range's own units: the bottom of the range is the bottom of the
        // window, which is what every ad-hoc `min`/`max` pair meant.
        assert!((axis.fraction_of(-1.0) - 0.0).abs() < 1e-12);
        assert!((axis.fraction_of(0.0) - 0.5).abs() < 1e-12);
        assert!((axis.fraction_of(1.0) - 1.0).abs() < 1e-12);
        // Reversed bounds are the same axis.
        assert_eq!(Axis::ranged(1.0, -1.0, Unit::Norm).bounds(), (-1.0, 1.0));
        // Degenerate ranges do not divide by zero.
        let flat = Axis::ranged(5.0, 5.0, Unit::Db);
        assert!(flat.extent() > 0.0);
        assert!(flat.fraction_of(5.0).is_finite());
    }

    #[test]
    fn pan_clamps_at_edges() {
        let total = 1000;
        let mut v = View {
            start: 400.0,
            len: 200.0,
        };
        v.pan(10.0, total); // way past the right edge
        assert_eq!(v.start, 800.0); // 1000 - 200
        v.pan(-10.0, total); // way past the left edge
        assert_eq!(v.start, 0.0);
    }
}
