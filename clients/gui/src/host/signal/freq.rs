//! **The frequency axis a spectrum carries alone.**
//!
//! Every other axis in the host is somebody else's: a timeline view navigates
//! the window's shared time, a clip's body the clip's local span, a lane's
//! chrome the group's. A navigable spectrum measures **hertz along x**, and
//! nothing else in a window does — so there is no axis to share, no navigation
//! group to join and no history to keep behind it, since every bin is there
//! every frame. The window is one normalized `(start, len)` on the element
//! ([`EditorProps::x_view`](super::super::widget::EditorProps::x_view)).
//!
//! What is here is the three things a gesture asks of such an axis, all of
//! which the machine used to reach into the element for: where it lies inside
//! the placement, what it shows, and how far in it may be asked to go. Each is
//! the element's because each is a fact about the *analysis* — its resolution,
//! its scale, its rate — and the machine only ever wanted the numbers.

use super::super::layout::Rect;
use super::super::metrics::Metrics;
use super::super::widget::element::FreqAxis;
use super::super::widget::{Ruler, RulerY};
use super::SignalElement;

impl SignalElement {
    /// The axis inside `rect`, resolved through the **renderer's own** region
    /// split — the label, the hertz strip and the value strip come off the
    /// rectangle exactly as the frame takes them, so a gesture anchors at the
    /// hertz the reader has the pointer on. `None` when the element navigates
    /// no frequency axis, or when nothing is left to draw in.
    pub fn freq_axis(&self, rect: Rect, m: &Metrics, sample_rate: f64) -> Option<FreqAxis> {
        if !self.navigates_freq() {
            return None;
        }
        let r = super::super::spectrum::regions(
            rect,
            self.display.label.is_some(),
            self.editor.ruler != Ruler::Off,
            self.editor.ruler_y != RulerY::Off,
            (self.spectral.db_floor, self.spectral.db_ceil),
            m,
        );
        if r.body.w <= 0.0 || r.body.h <= 0.0 {
            return None;
        }
        // The strip under the body carries the ticks, so the axis answers to a
        // pointer on it as much as on the picture itself.
        let surface = match r.strip_x {
            Some(strip) => Rect::new(r.body.x, r.body.y, r.body.w, r.body.h + strip.h),
            None => r.body,
        };
        // What the axis is showing, not what was asked of it: a gesture anchors
        // in the picture the reader is pointing at.
        let (start, len) = self.freq_window(sample_rate);
        Some(FreqAxis {
            body: r.body,
            surface,
            start,
            len,
            sample_rate,
        })
    }

    /// The window this axis would show for `want`, or the one it shows now.
    pub fn freq_window_shown(
        &self,
        sample_rate: f64,
        want: Option<(f64, f64)>,
    ) -> Option<(f64, f64)> {
        if !self.navigates_freq() {
            return None;
        }
        Some(match want {
            Some((start, len)) => self.freq_window_of(sample_rate, start, len),
            None => self.freq_window(sample_rate),
        })
    }

    /// The narrowest window the axis may be asked for at `start`: the display
    /// width of a handful of this element's own analysis bins, through the very
    /// geometry the curve and the ruler are drawn with — the fallback rate
    /// included, since the floor has to be the one the reader sees.
    pub fn freq_min_span(&self, sample_rate: f64, start: f64) -> Option<f64> {
        if !self.navigates_freq() {
            return None;
        }
        let (nyquist, f_lo_norm) =
            super::super::spectrum::axis_geometry(self.freq_rate(sample_rate));
        Some(super::super::spectrum::min_display_span(
            self.spectral.fft_size,
            nyquist * 2.0,
            self.spectral.freq_scale,
            f_lo_norm,
            start,
        ))
    }
}
