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
}
