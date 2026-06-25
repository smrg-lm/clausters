//! The common interface for a navigable, time-aligned view.
//!
//! Both the waveform and the spectrogram are driven the same way: a
//! `viewport::View` (the visible sample range) plus a render width in pixels go
//! in, GPU geometry/uniforms come out, and a draw is recorded. Expressing that
//! as a trait lets the native windowing harness (`native`) drive either view
//! with identical input handling, and keeps the door open for more views.

use crate::viewport::View;

/// A view over a buffer that can be panned/zoomed in time and drawn on the GPU.
pub trait TimelineView {
    /// Total length of the underlying buffer in samples (for `View::full`).
    fn total_samples(&self) -> usize;

    /// Prepare GPU resources for `view` at `render_width_px` device pixels.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &View,
        render_width_px: u32,
    );

    /// Record the draw into an existing render pass.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>);

    // --- Optional interactions (default no-op). Each returns whether the view
    // changed and should be redrawn. Kept windowing-agnostic: the harness
    // translates native events into these, so a web host can do the same. ---

    /// A printable character was typed (view-specific shortcuts).
    fn on_char(&mut self, c: char) -> bool {
        let _ = c;
        false
    }

    /// Zoom the secondary (e.g. frequency) axis by `factor` (<1 zooms in),
    /// keeping `anchor` (0 = bottom, 1 = top) fixed.
    fn on_vertical_zoom(&mut self, factor: f64, anchor: f64) -> bool {
        let _ = (factor, anchor);
        false
    }

    /// Snapshot the secondary axis at the start of a drag (mouse-down).
    fn on_vertical_drag_begin(&mut self) {}

    /// Update an in-progress secondary-axis drag. `total` is the cursor's total
    /// displacement since `on_vertical_drag_begin`, as a fraction of the window
    /// height. Panning is absolute (from the snapshot), so a clamped edge never
    /// drifts and the view re-aligns with the cursor when it returns.
    fn on_vertical_drag(&mut self, total: f64) -> bool {
        let _ = total;
        false
    }
}
