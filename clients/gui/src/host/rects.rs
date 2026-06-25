//! A tiny solid-rectangle renderer for the host's container/label chrome.
//!
//! The heavy `waveform` view owns its own pixels; everything else at this
//! milestone (panel backgrounds, a placeholder bar where a `label`'s text will
//! go) is a flat colored rectangle. This batches them all into one vertex buffer
//! of clip-space triangles and draws them in a single call — the same shape the
//! waveform's column pipeline already draws, so it stands on verified ground.
//! Glyph text for labels lands in a later milestone; here a label reserves and
//! marks its space.

use super::layout::Rect;

/// `[clip_x, clip_y, r, g, b, a]` per vertex.
const FLOATS_PER_VERTEX: usize = 6;

/// Batched solid-rectangle renderer.
pub struct RectRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
    scratch: Vec<f32>,
}

impl RectRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rects shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("rects.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rects pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: (FLOATS_PER_VERTEX * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: (2 * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
                    shader_location: 1,
                },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rects pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: std::slice::from_ref(&vertex_layout),
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capacity_vertices = 6 * 64;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rects vertices"),
            size: capacity_vertices * FLOATS_PER_VERTEX as u64 * std::mem::size_of::<f32>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            vertex_buffer,
            capacity_vertices,
            num_vertices: 0,
            scratch: Vec::new(),
        }
    }

    /// Rebuilds the geometry for `rects` (each a pixel rectangle and an RGBA
    /// color) against a framebuffer of `fb_w` x `fb_h` device pixels, converting
    /// to clip space (top-left pixel origin -> centered clip, y flipped).
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rects: &[(Rect, [f32; 4])],
        fb_w: u32,
        fb_h: u32,
    ) {
        self.scratch.clear();
        let (fw, fh) = (fb_w.max(1) as f32, fb_h.max(1) as f32);
        let to_clip = |px: f32, py: f32| ((px / fw) * 2.0 - 1.0, 1.0 - (py / fh) * 2.0);
        for (r, color) in rects {
            let (xl, yt) = to_clip(r.x, r.y);
            let (xr, yb) = to_clip(r.x + r.w, r.y + r.h);
            for &(x, y) in &[(xl, yb), (xr, yb), (xr, yt), (xl, yb), (xr, yt), (xl, yt)] {
                self.scratch
                    .extend_from_slice(&[x, y, color[0], color[1], color[2], color[3]]);
            }
        }
        let needed = (self.scratch.len() / FLOATS_PER_VERTEX) as u64;
        if needed > self.capacity_vertices {
            self.capacity_vertices = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rects vertices"),
                size: self.capacity_vertices
                    * FLOATS_PER_VERTEX as u64
                    * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.scratch));
        self.num_vertices = needed as u32;
    }

    /// Records the draw into an existing render pass (full-framebuffer viewport).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.num_vertices == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.num_vertices, 0..1);
    }
}
