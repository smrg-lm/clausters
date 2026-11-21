//! **A pane's dialog's heavy views, drawn over the dialog.**
//!
//! A pane's dialog stands over the whole dock: its card and what its
//! elements draw over themselves are batches the dock draws over every pane
//! ([`super::Batches::float`]). Its heavy views -- a spectrogram, a shader
//! `canvas` -- are passes of the pane's own pipelines, which only the pane
//! draws, and the pane's texture is under the dialog. So the pane draws them
//! into a texture of the card's size, over the card's own picture, and the
//! dock copies the rectangles they cover into its surface between the card
//! and what is drawn over it (`Composer::compose`), each pixel the pane's
//! own.

use clausters_gui_draw::rect::Rect;

use super::items::Collected;
use super::{Batches, Space, begin_into, draw_heavy};
use crate::gpu::Gpu;
use crate::painter::Painter;
use crate::view::Renderers;

/// **The dialog a pane floats with heavy views in it**: where its card is,
/// in the pane's pixels, and the rectangles its views cover, in the same.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FloatDialog {
    /// The card, grown to whole pixels.
    pub(crate) card: Rect,
    /// What the heavy views cover, each cut to its clip and to the card.
    pub(crate) rects: Vec<Rect>,
}

impl FloatDialog {
    /// The dialog whose card is `card`, holding the views of `inside`;
    /// `None` when it holds none, and the dialog is all flat batches.
    pub(super) fn of(card: Rect, inside: &Collected) -> Option<Self> {
        let x0 = card.x.floor();
        let y0 = card.y.floor();
        let card = Rect::new(
            x0,
            y0,
            (card.x + card.w).ceil() - x0,
            (card.y + card.h).ceil() - y0,
        );
        let cut = |r: Rect, clip: Option<Rect>| {
            let r = clip.map_or(r, |c| c.intersect(r)).intersect(card);
            (r.w >= 1.0 && r.h >= 1.0).then_some(r)
        };
        let rects: Vec<Rect> = inside
            .timeline_items
            .iter()
            .filter(|item| {
                item.look
                    .layers
                    .has(crate::host::graphics::signal::layers::Paint::Spectrogram)
            })
            .filter_map(|item| cut(item.body, item.clip))
            .chain(
                inside
                    .canvas_frames
                    .iter()
                    .filter_map(|f| cut(f.body, f.clip)),
            )
            .collect();
        (!rects.is_empty()).then_some(Self { card, rects })
    }

    /// The framebuffer its views are drawn in: the card's.
    pub(super) fn space(&self) -> Space {
        Space {
            origin: (self.card.x, self.card.y),
            fb: self.size(),
        }
    }

    fn size(&self) -> (u32, u32) {
        ((self.card.w.max(1.0)) as u32, (self.card.h.max(1.0)) as u32)
    }
}

/// **The texture a pane's dialog's views are drawn in**, kept while its
/// size stands, and the batch that draws the card under them.
pub(crate) struct DialogLayer {
    size: (u32, u32),
    view: wgpu::TextureView,
    /// The multisampled attachment, when the window antialiases.
    msaa: Option<wgpu::TextureView>,
    painter: Painter,
}

impl DialogLayer {
    fn new(gpu: &Gpu, size: (u32, u32), base: &Painter) -> Self {
        let target = gpu.target();
        let texture = |samples: u32, usage| {
            gpu.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("pane dialog"),
                    size: wgpu::Extent3d {
                        width: size.0.max(1),
                        height: size.1.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: target.format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        Self {
            size,
            view: texture(
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            msaa: (target.samples > 1)
                .then(|| texture(target.samples, wgpu::TextureUsages::RENDER_ATTACHMENT)),
            painter: Painter::sharing(&gpu.device, target, base),
        }
    }
}

/// **What the dock copies of a pane's dialog**: the texture, where its
/// corner is and the rectangles to copy, in the pane's pixels.
pub(crate) struct DialogCopy<'a> {
    pub(crate) view: &'a wgpu::TextureView,
    pub(crate) card: Rect,
    pub(crate) rects: &'a [Rect],
}

impl Batches {
    /// **What the dock copies over this pane's dialog**, when the dialog up
    /// in it holds heavy views ([`DialogLayer`]).
    pub(crate) fn dialog_copy(&self) -> Option<DialogCopy<'_>> {
        let dialog = self.kept.dialog.as_ref()?;
        let layer = self.dialog_layer.as_ref()?;
        (layer.size == dialog.size()).then_some(DialogCopy {
            view: &layer.view,
            card: dialog.card,
            rects: &dialog.rects,
        })
    }
}

/// **Draws a pane's dialog's views into its layer**: the card's own
/// picture first (the first of the batches the pane floats, which holds the
/// card), so a view that is not opaque stands on it, then the views. Nothing
/// when no dialog with views is up.
pub(super) fn encode(
    gpu: &Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    spectrograms: &std::collections::HashMap<super::SlotAt, super::SpectrogramSlot>,
    canvases: &std::collections::HashMap<i32, crate::canvas::CanvasView>,
    encoder: &mut wgpu::CommandEncoder,
) {
    let Some(dialog) = batches.kept.dialog.clone() else {
        batches.dialog_layer = None;
        return;
    };
    let Some(inside) = batches.kept.inside.as_ref() else {
        return;
    };
    let size = dialog.size();
    if batches
        .dialog_layer
        .as_ref()
        .is_none_or(|layer| layer.size != size)
    {
        batches.dialog_layer = Some(DialogLayer::new(gpu, size, &batches.base));
    }
    let Some(layer) = batches.dialog_layer.as_mut() else {
        return;
    };
    layer.painter.upload_at(
        &gpu.device,
        &gpu.queue,
        &batches.float[0],
        (dialog.card.x, dialog.card.y),
        size,
    );
    let clear = wgpu::Color::TRANSPARENT;
    let mut pass = match &layer.msaa {
        Some(ms) => begin_into(encoder, ms, Some(&layer.view), clear),
        None => begin_into(encoder, &layer.view, None, clear),
    };
    layer.painter.draw(&mut pass);
    draw_heavy(
        &mut pass,
        renderers,
        spectrograms,
        canvases,
        inside,
        dialog.space(),
    );
}
