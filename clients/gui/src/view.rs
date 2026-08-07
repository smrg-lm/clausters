//! The common interface for a navigable, time-aligned view.
//!
//! Both the waveform and the spectrogram are driven the same way: a
//! `viewport::View` (the visible sample range) plus a render width in pixels go
//! in, GPU geometry/uniforms come out, and a draw is recorded. Expressing that
//! as a trait lets the native windowing harness (`native`) drive either view
//! with identical input handling, and keeps the door open for more views.

use crate::spectrogram::SpectrogramRenderer;
use crate::viewport::View;
use crate::waveform::WaveformRenderer;

/// The heavy views' shared GPU machinery: **one per window**, holding every
/// pipeline the timeline views draw through, and nothing that identifies a
/// particular view.
///
/// A render pipeline is a pure function of the device and the target format, so
/// it is the same object for every waveform and every spectrogram on a surface.
/// Keeping one per *element* — which is what a view used to do, compiling its
/// own shader module and pipelines on construction, and the spectrogram one set
/// per channel — makes a slot expensive exactly where the element library wants
/// it cheap: a composition should be able to give a slot to every clip body it
/// shows. The per-element state that remains is real (a vertex buffer and its
/// ranges, a magnitude texture, uniforms), and lives in the views.
///
/// The mirror of `host::paint::Painter`, which has always had this shape.
pub struct Renderers {
    pub waveform: WaveformRenderer,
    pub spectrogram: SpectrogramRenderer,
}

impl Renderers {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            waveform: WaveformRenderer::new(device, format),
            spectrogram: SpectrogramRenderer::new(device, format),
        }
    }
}

/// A view over a buffer that can be panned/zoomed in time and drawn on the GPU.
pub trait TimelineView {
    /// Total length of the underlying buffer in samples (for `View::full`).
    fn total_samples(&self) -> usize;

    /// Prepare GPU resources for `view` at `render_width_px` device pixels.
    /// `renderers` is the window's shared machinery — taken by `&mut` because
    /// building the frame's geometry borrows its scratch space.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderers: &mut Renderers,
        view: &View,
        render_width_px: u32,
    );

    /// Record the draw into an existing render pass, through the window's
    /// shared pipelines.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, renderers: &Renderers);

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
