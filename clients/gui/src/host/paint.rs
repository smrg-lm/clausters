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
    /// The active clip rectangle, if any: every triangle emitted while it is
    /// set is clipped to it geometrically (so a scrolled widget's chrome never
    /// bleeds outside its `scroll` container). Geometry, not a GPU scissor —
    /// the batch stays **one** upload and one draw on every front.
    clip: Option<Rect>,
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

    /// The `(x, y)` of every accumulated vertex, for bounds/layout tests.
    #[cfg(test)]
    pub(crate) fn positions(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.verts
            .chunks_exact(FLOATS_PER_VERTEX)
            .map(|v| (v[0], v[1]))
    }

    fn vertex(&mut self, p: [f32; 2], c: Color) {
        self.verts
            .extend_from_slice(&[p[0], p[1], c[0], c[1], c[2], c[3]]);
    }

    /// Sets (or clears) the clip rectangle applied to everything emitted next.
    pub fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
    }

    /// A triangle (clipped to the active clip rectangle, if any).
    pub fn tri(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: Color) {
        let Some(clip) = self.clip else {
            self.vertex(a, color);
            self.vertex(b, color);
            self.vertex(c, color);
            return;
        };
        // Sutherland-Hodgman against the clip rect's four half-planes: a
        // triangle clips to a convex polygon of at most 7 vertices, emitted
        // as a fan.
        let mut poly = [[0.0f32; 2]; 8];
        let mut n = 3;
        poly[..3].copy_from_slice(&[a, b, c]);
        for edge in 0..4 {
            let inside = |p: [f32; 2]| match edge {
                0 => p[0] >= clip.x,
                1 => p[0] <= clip.x + clip.w,
                2 => p[1] >= clip.y,
                _ => p[1] <= clip.y + clip.h,
            };
            let cross = |p: [f32; 2], q: [f32; 2]| {
                let (bound, axis) = match edge {
                    0 => (clip.x, 0),
                    1 => (clip.x + clip.w, 0),
                    2 => (clip.y, 1),
                    _ => (clip.y + clip.h, 1),
                };
                let other = 1 - axis;
                let t = (bound - p[axis]) / (q[axis] - p[axis]);
                let mut v = [0.0f32; 2];
                // The clipped axis lands *exactly* on the boundary (an
                // interpolated value can round just past it and re-enter the
                // next edge's outside), the other one interpolates.
                v[axis] = bound;
                v[other] = p[other] + t * (q[other] - p[other]);
                v
            };
            let mut next = [[0.0f32; 2]; 8];
            let mut m = 0;
            for i in 0..n {
                let (p, q) = (poly[i], poly[(i + 1) % n]);
                if inside(p) {
                    next[m] = p;
                    m += 1;
                    if !inside(q) {
                        next[m] = cross(p, q);
                        m += 1;
                    }
                } else if inside(q) {
                    next[m] = cross(p, q);
                    m += 1;
                }
            }
            poly = next;
            n = m;
            if n == 0 {
                return; // fully outside
            }
        }
        for i in 1..n - 1 {
            self.vertex(poly[0], color);
            self.vertex(poly[i], color);
            self.vertex(poly[i + 1], color);
        }
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

    #[test]
    fn clip_keeps_every_vertex_inside_the_rect() {
        let clip = Rect::new(10.0, 10.0, 50.0, 40.0);
        let mut m = Mesh::new();
        m.set_clip(Some(clip));
        // A rect poking out on every side, a line crossing it, a disc around
        // a corner: all clipped geometrically.
        m.rect(Rect::new(0.0, 0.0, 100.0, 100.0), [1.0; 4]);
        m.line([0.0, 30.0], [100.0, 30.0], 4.0, [1.0; 4]);
        m.disc(10.0, 10.0, 20.0, [1.0; 4]);
        m.set_clip(None);
        assert!(!m.is_empty());
        for (x, y) in m.positions() {
            assert!((10.0..=60.0).contains(&x) && (10.0..=50.0).contains(&y));
        }
    }

    #[test]
    fn clip_drops_fully_outside_geometry_and_keeps_inside_intact() {
        let mut m = Mesh::new();
        m.set_clip(Some(Rect::new(0.0, 0.0, 10.0, 10.0)));
        m.rect(Rect::new(50.0, 50.0, 10.0, 10.0), [1.0; 4]);
        assert!(m.is_empty(), "fully outside geometry is dropped");
        m.rect(Rect::new(2.0, 2.0, 4.0, 4.0), [1.0; 4]);
        assert_eq!(m.vertex_count(), 6, "fully inside geometry is unchanged");
    }
}
