//! Reusable navigation for any time-based view (waveform, spectrogram, ...).
//!
//! A `View` is the visible window into a buffer, expressed in source-sample
//! units with `f64` so that deep zoom stays precise over multi-million-sample
//! buffers. Zoom and pan are pure transforms on this window; they never touch
//! the data, which is what makes navigation independent of buffer length. This
//! type is intentionally renderer-agnostic so the waveform and the (future)
//! spectrogram share the exact same panning/zooming behaviour.

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
