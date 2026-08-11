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

/// **What every pipeline drawing into one window's pass must agree on**: the
/// color format it writes into and how many samples that attachment carries.
///
/// The format was already threaded through every constructor; the sample count
/// joins it rather than becoming a second parameter, because the two are one
/// fact about the *pass* and a pipeline that disagrees with it on either is
/// rejected at draw time. Antialiasing is therefore one number per window with
/// nothing per widget: it is the attachment that is multisampled, and the flat
/// geometry, the glyphs, the heavy views and a user `canvas` all draw into it
/// unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub format: wgpu::TextureFormat,
    /// The MSAA sample count of the pass: `1` (no multisampling — the default,
    /// and what an oscilloscope wants) or a count the adapter reports for this
    /// format, typically 4.
    pub samples: u32,
}

impl Target {
    /// A single-sampled target of `format` — what a front that does not ask for
    /// antialiasing draws into, and what the demo harnesses use.
    pub fn new(format: wgpu::TextureFormat) -> Self {
        Self { format, samples: 1 }
    }

    /// The multisample state every pipeline drawing into this target declares.
    pub fn multisample(&self) -> wgpu::MultisampleState {
        wgpu::MultisampleState {
            count: self.samples.max(1),
            ..Default::default()
        }
    }
}

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
    pub fn new(device: &wgpu::Device, target: Target) -> Self {
        Self {
            waveform: WaveformRenderer::new(device, target),
            spectrogram: SpectrogramRenderer::new(device, target),
        }
    }
}

/// **Where a heavy view's picture sits inside the viewport it is drawn with** —
/// an affine `scale * ndc + offset` a view applies to its own geometry.
///
/// It exists because a viewport does not *cut*. Every heavy view builds its
/// geometry in normalized device coordinates filling `[-1, 1]` and lets
/// `set_viewport` place it, and wgpu refuses a viewport that leaves the
/// attachment — so an element hanging off the bottom of a window, given the
/// rectangle that is left, drew its whole picture squashed into it, and one
/// hanging off the top slid up instead of being cut. The viewport is a *scale*,
/// which is exactly what framing is not allowed to be.
///
/// So the viewport stays legal (the visible part of the rectangle) and the view
/// places its geometry for the **full** rectangle inside it. What falls outside
/// clip space is then rasterized away — cut, at a fixed size, which is what a
/// window edge is supposed to do. A fully visible element gets [`IDENTITY`] and
/// draws exactly as it always did.
///
/// [`IDENTITY`]: Framing::IDENTITY
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

impl Framing {
    /// The framing of an element the viewport already shows whole.
    pub const IDENTITY: Framing = Framing {
        scale: [1.0, 1.0],
        offset: [0.0, 0.0],
    };

    /// The framing that draws `rect` at its full size inside the viewport
    /// `visible`, both as `(x, y, w, h)` in device pixels with y downwards.
    /// `visible` is `rect` intersected with the framebuffer; when the two are
    /// equal this is [`Framing::IDENTITY`].
    pub fn new(rect: (f32, f32, f32, f32), visible: (f32, f32, f32, f32)) -> Framing {
        let (rx, ry, rw, rh) = rect;
        let (vx, vy, vw, vh) = visible;
        if vw <= 0.0 || vh <= 0.0 {
            return Framing::IDENTITY;
        }
        // A point at ndc x maps to pixel rx + (x + 1)/2 * rw, and back into the
        // viewport as 2 * (px - vx)/vw - 1. The y axis runs the other way in
        // clip space than it does in pixels, hence the mirrored second row.
        Framing {
            scale: [rw / vw, rh / vh],
            offset: [
                (2.0 * (rx - vx) + rw) / vw - 1.0,
                1.0 - (2.0 * (ry - vy) + rh) / vh,
            ],
        }
    }

    /// Places one normalized-device point.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            x * self.scale[0] + self.offset[0],
            y * self.scale[1] + self.offset[1],
        )
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

#[cfg(test)]
mod tests {
    use super::Framing;

    /// A view the window shows whole is framed by nothing at all — the property
    /// that keeps every existing picture byte-identical.
    #[test]
    fn a_visible_element_is_the_identity() {
        let r = (10.0, 20.0, 300.0, 120.0);
        assert_eq!(Framing::new(r, r), Framing::IDENTITY);
        assert_eq!(Framing::IDENTITY.apply(0.5, -0.25), (0.5, -0.25));
    }

    /// Hanging off the bottom: the top edge stays where it is and the bottom
    /// leaves clip space, so the picture is cut at a fixed size instead of
    /// being squashed into what is left.
    #[test]
    fn hanging_off_the_bottom_is_cut_not_squashed() {
        // A 40 px lane at y 80 in a 100 px window: 20 px of it are visible.
        let f = Framing::new((0.0, 80.0, 100.0, 40.0), (0.0, 80.0, 100.0, 20.0));
        assert_eq!(f.scale[1], 2.0);
        assert!(
            (f.apply(0.0, 1.0).1 - 1.0).abs() < 1e-6,
            "the top edge holds"
        );
        assert!(
            (f.apply(0.0, 0.0).1 + 1.0).abs() < 1e-6,
            "the middle is now the bottom"
        );
        assert!(
            f.apply(0.0, -1.0).1 < -1.0,
            "the bottom half is clipped away"
        );
        // ...and nothing moves horizontally.
        assert_eq!(f.scale[0], 1.0);
        assert_eq!(f.offset[0], 0.0);
    }

    /// Hanging off the top: the picture is cut there too rather than sliding
    /// down with a clamped origin, which is what the viewport alone did.
    #[test]
    fn hanging_off_the_top_does_not_slide() {
        // The same lane at y -30: 10 px of its bottom are visible.
        let f = Framing::new((0.0, -30.0, 100.0, 40.0), (0.0, 0.0, 100.0, 10.0));
        assert_eq!(f.scale[1], 4.0);
        assert!(f.apply(0.0, 1.0).1 > 1.0, "the top is clipped away");
        assert!(
            (f.apply(0.0, -1.0).1 + 1.0).abs() < 1e-6,
            "the bottom edge holds"
        );
        // The sample that sits on the window's top edge (3/4 down the rect)
        // lands on the top of the viewport.
        assert!((f.apply(0.0, -0.5).1 - 1.0).abs() < 1e-6);
    }

    /// The horizontal axis works the same way, and a degenerate viewport asks
    /// for nothing (the caller skips the draw).
    #[test]
    fn the_horizontal_axis_and_the_degenerate_case() {
        let f = Framing::new((-50.0, 0.0, 200.0, 10.0), (0.0, 0.0, 150.0, 10.0));
        assert!((f.scale[0] - 200.0 / 150.0).abs() < 1e-6);
        assert!(f.apply(-1.0, 0.0).0 < -1.0, "the left is clipped away");
        assert!(
            (f.apply(1.0, 0.0).0 - 1.0).abs() < 1e-6,
            "the right edge holds"
        );
        assert_eq!(
            Framing::new((0.0, 0.0, 10.0, 10.0), (0.0, 0.0, 0.0, 0.0)),
            Framing::IDENTITY
        );
    }
}
