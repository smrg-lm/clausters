//! 2D colored geometry: the host's one drawing primitive for chrome, controls
//! and text.
//!
//! Everything the GUI host paints that is not the heavy `waveform` view — panel
//! backgrounds, sliders, knobs, buttons, toggles and the bitmap glyphs of labels
//! and values — is a batch of flat-colored triangles. [`Mesh`] accumulates them
//! in **device pixels** (top-left origin) with convenience builders (rect, quad,
//! line, disc), and [`Painter`] uploads the batch once and draws it in a single
//! call, converting pixel space to clip space in the shader feed. It is the same
//! shape the waveform's column pipeline draws, so it stands on verified ground;
//! one pipeline, no textures.

use super::layout::Rect;

/// `[x, y, r, g, b, a]` per vertex, position in device pixels.
const FLOATS_PER_VERTEX: usize = 6;

/// An RGBA color.
pub type Color = [f32; 4];

/// A batch of flat-colored triangles in device-pixel space.
#[derive(Default)]
pub struct Mesh {
    verts: Vec<f32>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.verts.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.verts.is_empty()
    }

    fn vertex(&mut self, p: [f32; 2], c: Color) {
        self.verts
            .extend_from_slice(&[p[0], p[1], c[0], c[1], c[2], c[3]]);
    }

    /// A triangle.
    pub fn tri(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: Color) {
        self.vertex(a, color);
        self.vertex(b, color);
        self.vertex(c, color);
    }

    /// A quad from four corners in order (two triangles); winding-agnostic
    /// because the pipeline does not cull.
    pub fn quad(&mut self, p: [[f32; 2]; 4], color: Color) {
        self.tri(p[0], p[1], p[2], color);
        self.tri(p[0], p[2], p[3], color);
    }

    /// An axis-aligned rectangle.
    pub fn rect(&mut self, r: Rect, color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        self.quad(
            [
                [r.x, r.y + r.h],
                [r.x + r.w, r.y + r.h],
                [r.x + r.w, r.y],
                [r.x, r.y],
            ],
            color,
        );
    }

    /// A thick line segment from `a` to `b` of width `w`.
    pub fn line(&mut self, a: [f32; 2], b: [f32; 2], w: f32, color: Color) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt().max(f32::MIN_POSITIVE);
        let (nx, ny) = (-dy / len * w * 0.5, dx / len * w * 0.5);
        self.quad(
            [
                [a[0] + nx, a[1] + ny],
                [b[0] + nx, b[1] + ny],
                [b[0] - nx, b[1] - ny],
                [a[0] - nx, a[1] - ny],
            ],
            color,
        );
    }

    /// A `w`-pixel-thick outline of `rect` (four edge rectangles).
    pub fn border(&mut self, rect: Rect, w: f32, color: Color) {
        self.rect(Rect::new(rect.x, rect.y, rect.w, w), color);
        self.rect(Rect::new(rect.x, rect.y + rect.h - w, rect.w, w), color);
        self.rect(Rect::new(rect.x, rect.y, w, rect.h), color);
        self.rect(Rect::new(rect.x + rect.w - w, rect.y, w, rect.h), color);
    }

    /// A filled circle approximated by a triangle fan.
    pub fn disc(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        const SEGMENTS: usize = 32;
        let center = [cx, cy];
        for i in 0..SEGMENTS {
            let a0 = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
            let a1 = std::f32::consts::TAU * (i + 1) as f32 / SEGMENTS as f32;
            self.tri(
                center,
                [cx + radius * a0.cos(), cy + radius * a0.sin()],
                [cx + radius * a1.cos(), cy + radius * a1.sin()],
                color,
            );
        }
    }

    /// The number of vertices accumulated (three per triangle).
    pub fn vertex_count(&self) -> u32 {
        (self.verts.len() / FLOATS_PER_VERTEX) as u32
    }
}

/// Uploads a [`Mesh`] and draws it.
pub struct Painter {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
}

impl Painter {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paint shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("paint.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paint pipeline layout"),
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
            label: Some("paint pipeline"),
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
        let capacity_vertices = 6 * 256;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint vertices"),
            size: capacity_vertices * FLOATS_PER_VERTEX as u64 * std::mem::size_of::<f32>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            vertex_buffer,
            capacity_vertices,
            num_vertices: 0,
        }
    }

    /// Uploads `mesh`, converting its device-pixel positions to clip space
    /// against a framebuffer of `fb_w` x `fb_h` (top-left origin, y flipped).
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        fb_w: u32,
        fb_h: u32,
    ) {
        let (fw, fh) = (fb_w.max(1) as f32, fb_h.max(1) as f32);
        let mut clip = Vec::with_capacity(mesh.verts.len());
        for v in mesh.verts.chunks_exact(FLOATS_PER_VERTEX) {
            clip.push((v[0] / fw) * 2.0 - 1.0);
            clip.push(1.0 - (v[1] / fh) * 2.0);
            clip.extend_from_slice(&v[2..6]);
        }
        let needed = mesh.vertex_count() as u64;
        if needed > self.capacity_vertices {
            self.capacity_vertices = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("paint vertices"),
                size: self.capacity_vertices
                    * FLOATS_PER_VERTEX as u64
                    * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&clip));
        self.num_vertices = needed as u32;
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.num_vertices == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.num_vertices, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_emits_two_triangles() {
        let mut m = Mesh::new();
        m.rect(Rect::new(0.0, 0.0, 10.0, 10.0), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(m.vertex_count(), 6);
    }

    #[test]
    fn zero_size_rect_emits_nothing() {
        let mut m = Mesh::new();
        m.rect(Rect::new(0.0, 0.0, 0.0, 10.0), [1.0; 4]);
        assert!(m.is_empty());
    }
}
