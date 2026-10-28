//! **A window's picture, kept on the card**: everything a whole frame drew
//! but the live layers, in a texture of the window's own, so that a frame
//! drawn because a playhead or a meter moved copies it and draws those
//! layers over it, rather than drawing every heavy view and every panel
//! again.
//!
//! A frame of a window with live layers is up to three passes
//! ([`super::encode_pass`]):
//!
//! 1. **The picture**, into this texture, when a whole frame changed it:
//!    multisampled like any frame and resolved here.
//! 2. **The live layers' boxes**, when the window is multisampled: each box
//!    the live layers cover is drawn into a small atlas, the picture under
//!    it copied first and the live layers over it, multisampled and
//!    resolved, which is the antialiasing a single pass would have given
//!    them.
//! 3. **The frame**, into the surface, single-sampled: the picture copied
//!    whole, and each box copied over it (or, in a window that is not
//!    multisampled, the live layers drawn straight over it).
//!
//! So a frame of what moves costs one copy of the window's pixels and the
//! few the live layers cover, however much the picture under them costs to
//! draw: a spectrogram, a stack of waveforms and the panels around them are
//! drawn once per change, not once per tick of the clock. The multisampled
//! work is the boxes' alone, where a whole window of samples used to be
//! drawn and resolved on every tick.
//!
//! **It is the single pass's picture.** The live layers sat over the window's
//! overlays and under a dialog's heavy views, the host's bands and the open
//! menus. A window with a dialog keeps nothing (every frame is whole), and
//! the bands and menus are in the kept picture: where a live layer would lie
//! under one of them, the frame is drawn the single pass's way, so the order
//! the eye sees never changes ([`plan`]).
//!
//! It is passes, textures and texel loads, which every backend the host runs
//! on has (Vulkan, Metal, DirectX, WebGPU and WebGL2), so the native window
//! and the page draw it the same way.

use crate::host::paint::{Mesh, Painter};
use crate::view::Target;

/// The most boxes a frame draws through the atlas; more, and they are drawn
/// as the one box around them.
const MAX_BOXES: usize = 8;

/// Device pixels a box reaches past its triangles, for the antialiased edge.
const PAD: f32 = 2.0;

/// How close two boxes are drawn as one, in device pixels: a pass a box
/// costs more than the pixels between two near ones.
const JOIN: f32 = 16.0;

/// A box in device pixels: `[x, y, w, h]`.
pub(super) type PxBox = [u32; 4];

/// **Which boxes of a window the live layers are drawn in**, or `None` when
/// the frame is to be drawn in one pass.
///
/// `live` is the live layers' mesh, `over` where what was drawn over them
/// lies (the host's bands and the open menus), `fb` the window. One pass is
/// the answer where a live layer meets what is over it, since the kept
/// picture holds that over everything, and where the boxes would cover half
/// the window or more, which a copy and a pass of their own would not save.
pub(super) fn plan(live: &Mesh, over: &[[f32; 4]], fb: (u32, u32)) -> Option<Vec<PxBox>> {
    let tris = live.triangle_bounds();
    let joined = join(&tris, 0.0);
    let meets = joined.iter().any(|a| {
        over.iter()
            .any(|b| a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3])
    });
    if meets {
        return None;
    }
    let mut boxes = join(&tris, PAD);
    if boxes.len() > MAX_BOXES {
        boxes = vec![boxes.iter().fold(boxes[0], |a, b| union(&a, b))];
    }
    let (w, h) = (fb.0 as f32, fb.1 as f32);
    let boxes: Vec<PxBox> = boxes
        .iter()
        .filter_map(|b| {
            let x0 = b[0].floor().clamp(0.0, w);
            let y0 = b[1].floor().clamp(0.0, h);
            let x1 = b[2].ceil().clamp(0.0, w);
            let y1 = b[3].ceil().clamp(0.0, h);
            (x1 > x0 && y1 > y0).then_some([
                x0 as u32,
                y0 as u32,
                (x1 - x0) as u32,
                (y1 - y0) as u32,
            ])
        })
        .collect();
    let area: u64 = boxes
        .iter()
        .map(|b| u64::from(b[2]) * u64::from(b[3]))
        .sum();
    (area * 2 < u64::from(fb.0) * u64::from(fb.1)).then_some(boxes)
}

/// The triangles' bounds grown by `pad`, and those that meet or lie within
/// [`JOIN`] of each other joined.
pub(super) fn join(tris: &[[f32; 4]], pad: f32) -> Vec<[f32; 4]> {
    let mut boxes: Vec<[f32; 4]> = Vec::new();
    for t in tris {
        let mut b = [t[0] - pad, t[1] - pad, t[2] + pad, t[3] + pad];
        // a box joined may now reach others: take them in until none is near
        while let Some(i) = boxes.iter().position(|o| near(o, &b)) {
            b = union(&boxes.swap_remove(i), &b);
        }
        boxes.push(b);
    }
    boxes
}

fn near(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[0] - JOIN <= b[2] && b[0] - JOIN <= a[2] && a[1] - JOIN <= b[3] && b[1] - JOIN <= a[3]
}

fn union(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// The texture a window's picture is kept in, the atlas its live boxes are
/// drawn in, and what copies either into a pass.
pub(crate) struct Underlay {
    /// The copy, into the window's multisampled attachment (the atlas).
    multi: wgpu::RenderPipeline,
    /// The copy, into a single-sampled one (the surface).
    single: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    target: Target,
    held: Option<Held>,
    atlas: Option<Atlas>,
    /// The offsets each copy reads at, one buffer a copy, and the binding
    /// of each with the texture it copies, as of the textures' `epoch`.
    shifts: Vec<wgpu::Buffer>,
    bound: Vec<Option<(u64, wgpu::BindGroup)>>,
    /// Counts the textures made: a binding of an older one is made again.
    epoch: u64,
    /// What draws the live layers into each box.
    painters: Vec<Painter>,
    /// The boxes of the frame being encoded, each with its place in the
    /// atlas and the bindings that copy into it and out of it.
    placed: Vec<Placed>,
    /// The texture holds the picture of the batches as they stand.
    pub(super) fresh: bool,
}

/// The picture's texture at one size; the binding in slot 0 copies it whole.
struct Held {
    size: (u32, u32),
    view: wgpu::TextureView,
}

/// The atlas the boxes are drawn into, multisampled and resolved.
struct Atlas {
    size: (u32, u32),
    samples: wgpu::TextureView,
    resolved: wgpu::TextureView,
}

/// One box of a frame: where it is in the window and in the atlas.
struct Placed {
    window: PxBox,
    at: (u32, u32),
    /// The binding that copies the picture under the box into the atlas,
    /// and the one that copies the atlas' box into the window.
    under: usize,
    out: usize,
}

impl Underlay {
    pub(crate) fn new(device: &wgpu::Device, target: Target) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kept picture bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kept picture shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kept picture pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |samples: u32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("kept picture pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    // Replaced, not blended: a copy is what it copies.
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target.format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            multi: pipeline(target.samples.max(1)),
            single: pipeline(1),
            layout,
            target,
            held: None,
            atlas: None,
            shifts: Vec::new(),
            bound: Vec::new(),
            epoch: 0,
            painters: Vec::new(),
            placed: Vec::new(),
            fresh: false,
        }
    }

    /// The texture to draw the picture into at `size`, made again when the
    /// window's size changed (and the picture with it, so it is not fresh).
    pub(super) fn target(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
    ) -> wgpu::TextureView {
        if self.held.as_ref().is_none_or(|h| h.size != size) {
            self.fresh = false;
            let view = self.texture(device, "kept picture", size, 1);
            self.held = Some(Held { size, view });
        }
        let view = self.held.as_ref().expect("made above").view.clone();
        self.binding(device, queue, 0, &view, (0, 0));
        view
    }

    /// Whether it holds the picture for a target of `size`.
    pub(super) fn holds(&self, size: (u32, u32)) -> bool {
        self.fresh && self.held.as_ref().is_some_and(|h| h.size == size)
    }

    /// Lets the textures go: a window with nothing live keeps no copy.
    pub(super) fn release(&mut self) {
        self.held = None;
        self.atlas = None;
        self.placed.clear();
        self.fresh = false;
    }

    /// **Lays `boxes` out in the atlas** and uploads what draws `live` into
    /// each, its glyphs from `base`'s sheet. Answers the atlas' views, to
    /// draw into, or `None` when it would be larger than the card allows.
    pub(super) fn place(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &Painter,
        live: &Mesh,
        boxes: &[PxBox],
    ) -> Option<(wgpu::TextureView, wgpu::TextureView)> {
        self.placed.clear();
        let limit = device.limits().max_texture_dimension_2d;
        // In a row or in a column, whichever the card takes and is smaller.
        let row = (
            boxes.iter().map(|b| b[2]).sum::<u32>(),
            boxes.iter().map(|b| b[3]).max().unwrap_or(1),
        );
        let column = (
            boxes.iter().map(|b| b[2]).max().unwrap_or(1),
            boxes.iter().map(|b| b[3]).sum::<u32>(),
        );
        let fits = |s: (u32, u32)| s.0 <= limit && s.1 <= limit;
        let in_row = match (fits(row), fits(column)) {
            (true, true) => {
                u64::from(row.0) * u64::from(row.1) <= u64::from(column.0) * u64::from(column.1)
            }
            (true, false) => true,
            (false, true) => false,
            (false, false) => return None,
        };
        let need = if in_row { row } else { column };
        // Grown in steps, so a box that moves does not make it again.
        if self
            .atlas
            .as_ref()
            .is_none_or(|a| a.size.0 < need.0 || a.size.1 < need.1)
        {
            let grow = |n: u32| n.next_multiple_of(256).min(limit);
            let have = self.atlas.as_ref().map_or((0, 0), |a| a.size);
            let size = (grow(need.0.max(have.0)), grow(need.1.max(have.1)));
            self.atlas = Some(Atlas {
                size,
                samples: self.texture(device, "live boxes", size, self.target.samples.max(1)),
                resolved: self.texture(device, "live boxes resolved", size, 1),
            });
        }
        let atlas = self.atlas.as_ref().expect("made above");
        let (samples, resolved) = (atlas.samples.clone(), atlas.resolved.clone());
        let picture = self.held.as_ref()?.view.clone();
        while self.painters.len() < boxes.len() {
            self.painters
                .push(Painter::sharing(device, self.target, base));
        }
        let mut at = (0, 0);
        for (i, b) in boxes.iter().enumerate() {
            let shift_in = (b[0] as i32 - at.0 as i32, b[1] as i32 - at.1 as i32);
            let (under, out) = (1 + 2 * i, 2 + 2 * i);
            self.binding(device, queue, under, &picture, shift_in);
            self.binding(device, queue, out, &resolved, (-shift_in.0, -shift_in.1));
            self.painters[i].upload_at(
                device,
                queue,
                live,
                (b[0] as f32, b[1] as f32),
                (b[2], b[3]),
            );
            self.placed.push(Placed {
                window: *b,
                at,
                under,
                out,
            });
            at = if in_row {
                (at.0 + b[2], 0)
            } else {
                (0, at.1 + b[3])
            };
        }
        Some((samples, resolved))
    }

    /// **The boxes into the atlas**: each the picture under it, then the
    /// live layers.
    pub(super) fn draw_boxes(&self, pass: &mut wgpu::RenderPass<'_>) {
        for (p, painter) in self.placed.iter().zip(&self.painters) {
            let [_, _, w, h] = p.window;
            pass.set_viewport(p.at.0 as f32, p.at.1 as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(p.at.0, p.at.1, w, h);
            pass.set_pipeline(&self.multi);
            pass.set_bind_group(0, self.bound(p.under), &[]);
            pass.draw(0..3, 0..1);
            painter.draw(pass);
        }
    }

    /// **The frame, single-sampled**: the picture whole, then each box the
    /// atlas holds over it.
    pub(super) fn draw_frame(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.held.is_none() {
            return;
        }
        pass.set_pipeline(&self.single);
        pass.set_bind_group(0, self.bound(0), &[]);
        pass.draw(0..3, 0..1);
        for p in &self.placed {
            let [x, y, w, h] = p.window;
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(0, self.bound(p.out), &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Forgets the boxes, for a frame that draws the live layers itself.
    pub(super) fn unplace(&mut self) {
        self.placed.clear();
    }

    fn bound(&self, slot: usize) -> &wgpu::BindGroup {
        &self.bound[slot].as_ref().expect("bound as placed").1
    }

    fn texture(
        &mut self,
        device: &wgpu::Device,
        label: &str,
        size: (u32, u32),
        samples: u32,
    ) -> wgpu::TextureView {
        self.epoch += 1;
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if samples == 1 {
            usage |= wgpu::TextureUsages::TEXTURE_BINDING;
        }
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: self.target.format,
                usage,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// **Binds a copy of `view` in `slot`**, each pixel reading the texel
    /// `shift` from it: the offset is written into the slot's buffer, and
    /// the binding made again only when the textures were.
    fn binding(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: usize,
        view: &wgpu::TextureView,
        shift: (i32, i32),
    ) {
        while self.shifts.len() <= slot {
            self.shifts
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("kept picture shift"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            self.bound.push(None);
        }
        let offset = [shift.0 as f32, shift.1 as f32, 0.0, 0.0];
        queue.write_buffer(&self.shifts[slot], 0, bytemuck::cast_slice(&offset));
        if self.bound[slot]
            .as_ref()
            .is_some_and(|(epoch, _)| *epoch == self.epoch)
        {
            return;
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kept picture bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.shifts[slot].as_entire_binding(),
                },
            ],
        });
        self.bound[slot] = Some((self.epoch, group));
    }
}

/// One triangle over the viewport, each pixel the texel its shift away.
const SHADER: &str = r"
struct Shift {
    offset: vec4<f32>,
};

@group(0) @binding(0) var picture: texture_2d<f32>;
@group(0) @binding(1) var<uniform> shift: Shift;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(picture, vec2<i32>(floor(at.xy + shift.offset.xy)), 0);
}
";

#[cfg(test)]
mod tests {
    use super::*;

    /// **Near triangles are one box, far ones two**, each grown by its pad.
    #[test]
    fn near_triangles_join_and_far_ones_stay_apart() {
        let tris = [
            [10.0, 10.0, 11.0, 200.0],
            [12.0, 5.0, 20.0, 12.0],
            [500.0, 10.0, 520.0, 30.0],
        ];
        let boxes = join(&tris, 2.0);
        assert_eq!(boxes.len(), 2, "{boxes:?}");
        assert!(boxes.contains(&[8.0, 3.0, 22.0, 202.0]), "{boxes:?}");
        assert!(boxes.contains(&[498.0, 8.0, 522.0, 32.0]), "{boxes:?}");
    }

    /// **A box that joins two others takes both in**, whichever came first.
    #[test]
    fn a_box_between_two_joins_them_all() {
        let tris = [
            [0.0, 0.0, 10.0, 10.0],
            [100.0, 0.0, 110.0, 10.0],
            [20.0, 0.0, 90.0, 10.0],
        ];
        assert_eq!(join(&tris, 0.0), vec![[0.0, 0.0, 110.0, 10.0]]);
    }
}
