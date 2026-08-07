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

/// The rectangle four corners describe when **every edge is axis-parallel**,
/// or `None` for anything rotated. Winding- and origin-agnostic: it checks the
/// edges rather than a corner order, so it recognizes the quad whichever corner
/// it starts from and whichever way it goes round — [`Mesh::rect`] builds one
/// order, an axis-parallel [`Mesh::line`] another.
///
/// The rect is the corners' bounding box, which for such a quad *is* the quad.
fn axis_aligned(p: &[[f32; 2]; 4]) -> Option<Rect> {
    for i in 0..4 {
        let (a, b) = (p[i], p[(i + 1) % 4]);
        // One edge with neither endpoint shared is a diagonal: not our case.
        if a[0] != b[0] && a[1] != b[1] {
            return None;
        }
    }
    let (mut x0, mut y0) = (f32::INFINITY, f32::INFINITY);
    let (mut x1, mut y1) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for q in p {
        x0 = x0.min(q[0]);
        y0 = y0.min(q[1]);
        x1 = x1.max(q[0]);
        y1 = y1.max(q[1]);
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

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

    /// A triangle, emitted verbatim — the caller has already established that
    /// it needs no clipping (there is none, or it survived the clamp).
    fn tri_raw(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: Color) {
        self.vertex(a, color);
        self.vertex(b, color);
        self.vertex(c, color);
    }

    /// A triangle (clipped to the active clip rectangle, if any).
    pub fn tri(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: Color) {
        let Some(clip) = self.clip else {
            self.tri_raw(a, b, c, color);
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
    ///
    /// **Clipping an axis-aligned quad is a clamp**, not a polygon pass, and
    /// that is the case almost all of the chrome is: a panel, a lane, a note
    /// body, a waveform column, a horizontal or vertical hairline. Since a
    /// rectangle intersected with a rectangle *is* a rectangle, the general
    /// [`tri`] clipper would spend a four-half-plane Sutherland-Hodgman pass
    /// per triangle to rediscover geometry two `min`/`max` pairs give exactly —
    /// measured at 2.6-6.9x the cost of the unclipped path, against 1.10-1.17x
    /// for the clamp. Only rotated geometry (a disc's fan, a diagonal line, a
    /// glyph outline) still needs the general pass, and still gets it.
    ///
    /// [`tri`]: Mesh::tri
    pub fn quad(&mut self, p: [[f32; 2]; 4], color: Color) {
        if let Some(clip) = self.clip
            && let Some(r) = axis_aligned(&p)
        {
            let x0 = r.x.max(clip.x);
            let y0 = r.y.max(clip.y);
            let x1 = (r.x + r.w).min(clip.x + clip.w);
            let y1 = (r.y + r.h).min(clip.y + clip.h);
            if x1 <= x0 || y1 <= y0 {
                return; // clipped away entirely (or degenerate to begin with)
            }
            self.tri_raw([x0, y1], [x1, y1], [x1, y0], color);
            self.tri_raw([x0, y1], [x1, y0], [x0, y0], color);
            return;
        }
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

    /// The mesh's triangles as corner triples.
    fn triangles(m: &Mesh) -> Vec<[[f32; 2]; 3]> {
        let p: Vec<(f32, f32)> = m.positions().collect();
        p.chunks_exact(3)
            .map(|t| [[t[0].0, t[0].1], [t[1].0, t[1].1], [t[2].0, t[2].1]])
            .collect()
    }

    /// Whether `(x, y)` falls in a triangle (winding-agnostic, like the
    /// pipeline, which does not cull).
    fn inside(t: &[[f32; 2]; 3], x: f32, y: f32) -> bool {
        // A *degenerate* triangle covers nothing, and that has to be said
        // explicitly: the half-plane test alone reports every point as inside
        // one, since all three edge signs are zero. The general clipper does
        // emit them — a quad flush against a clip edge collapses to a sliver of
        // three equal vertices — and the rasterizer paints no pixel for it, so
        // neither does this.
        let (u, v) = (
            [t[1][0] - t[0][0], t[1][1] - t[0][1]],
            [t[2][0] - t[0][0], t[2][1] - t[0][1]],
        );
        if (u[0] * v[1] - u[1] * v[0]).abs() < 1e-6 {
            return false;
        }
        let side =
            |a: [f32; 2], b: [f32; 2]| (x - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (y - b[1]);
        let (d0, d1, d2) = (side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0]));
        !((d0 < 0.0 || d1 < 0.0 || d2 < 0.0) && (d0 > 0.0 || d1 > 0.0 || d2 > 0.0))
    }

    fn covers(m: &Mesh, x: f32, y: f32) -> bool {
        triangles(m).iter().any(|t| inside(t, x, y))
    }

    /// The milestone's own check: the axis-aligned clamp must paint **exactly**
    /// what the general Sutherland-Hodgman pass paints. Vertex lists cannot be
    /// compared directly — the general path emits a fan of up to seven vertices
    /// where the clamp emits six — so the comparison is the only thing that
    /// actually matters, the covered area.
    ///
    /// The grid offsets are deliberately non-dyadic. The general path splits
    /// the quad into two triangles along a **diagonal**, and a sample landing
    /// exactly on it is inside both, neither or one depending on rounding — the
    /// one place the two paths may legitimately disagree, since a boundary
    /// point has no answer. (The rasterizer never sees it: the two triangles
    /// share exact vertices and the fill rule settles the seam. The clamp has
    /// no interior diagonal at all.) A half-pixel grid lands on those diagonals
    /// constantly, because they run between integer corners.
    #[test]
    fn the_axis_aligned_clamp_paints_what_the_general_clipper_paints() {
        let clip = Rect::new(10.0, 10.0, 40.0, 30.0);
        let cases = [
            ("fully inside", Rect::new(15.0, 15.0, 10.0, 10.0)),
            ("over the left edge", Rect::new(2.0, 15.0, 20.0, 10.0)),
            ("over the right edge", Rect::new(40.0, 15.0, 30.0, 10.0)),
            ("over the top edge", Rect::new(15.0, 2.0, 10.0, 20.0)),
            ("over the bottom edge", Rect::new(15.0, 30.0, 10.0, 30.0)),
            ("over a corner", Rect::new(5.0, 5.0, 12.0, 12.0)),
            ("larger on every side", Rect::new(0.0, 0.0, 100.0, 100.0)),
            ("flush with the clip", Rect::new(10.0, 10.0, 40.0, 30.0)),
            ("fully outside", Rect::new(70.0, 70.0, 10.0, 10.0)),
            ("touching one edge only", Rect::new(50.0, 15.0, 10.0, 10.0)),
        ];
        for (name, r) in cases {
            let corners = [
                [r.x, r.y + r.h],
                [r.x + r.w, r.y + r.h],
                [r.x + r.w, r.y],
                [r.x, r.y],
            ];
            let mut fast = Mesh::new();
            fast.set_clip(Some(clip));
            fast.quad(corners, [1.0; 4]);

            // The same quad through the general clipper: `tri` never takes the
            // fast path, so this is the reference implementation.
            let mut general = Mesh::new();
            general.set_clip(Some(clip));
            general.tri(corners[0], corners[1], corners[2], [1.0; 4]);
            general.tri(corners[0], corners[2], corners[3], [1.0; 4]);

            let mut x = 0.2713;
            while x < 80.0 {
                let mut y = 0.6367;
                while y < 80.0 {
                    assert_eq!(
                        covers(&fast, x, y),
                        covers(&general, x, y),
                        "{name}: the two paths disagree at ({x}, {y})"
                    );
                    y += 1.0;
                }
                x += 1.0;
            }
        }
    }

    #[test]
    fn axis_aligned_recognizes_the_chrome_and_rejects_the_rest() {
        // What `rect` builds, whichever corner it starts from.
        let r = Rect::new(4.0, 6.0, 10.0, 20.0);
        let corners = [[4.0, 26.0], [14.0, 26.0], [14.0, 6.0], [4.0, 6.0]];
        let got = axis_aligned(&corners).expect("a rect is axis-aligned");
        assert_eq!((got.x, got.y, got.w, got.h), (r.x, r.y, r.w, r.h));
        // Rotating the starting corner and reversing the winding keep it.
        let rotated = [corners[2], corners[3], corners[0], corners[1]];
        assert!(
            axis_aligned(&rotated).is_some(),
            "start corner is irrelevant"
        );
        let reversed = [corners[3], corners[2], corners[1], corners[0]];
        assert!(axis_aligned(&reversed).is_some(), "winding is irrelevant");
        // A diagonal line's quad is not, and neither is a sheared one.
        let mut diagonal = Mesh::new();
        diagonal.line([0.0, 0.0], [10.0, 10.0], 2.0, [1.0; 4]);
        assert!(
            axis_aligned(&[[0.0, 0.0], [10.0, 10.0], [12.0, 8.0], [2.0, -2.0]]).is_none(),
            "a diagonal quad needs the general clipper"
        );
        assert!(!diagonal.is_empty());
    }

    /// A hairline is a quad too — `line` funnels through `quad`, so a
    /// horizontal or vertical one (a divider, a tick, a playhead, a baseline:
    /// most of the chrome) takes the clamp, and only a slanted one does not.
    #[test]
    fn axis_parallel_lines_take_the_fast_path() {
        for (a, b) in [
            ([0.0f32, 30.0], [100.0f32, 30.0]),
            ([30.0, 0.0], [30.0, 100.0]),
        ] {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dy * dy).sqrt();
            let (nx, ny) = (-dy / len * 2.0, dx / len * 2.0);
            let corners = [
                [a[0] + nx, a[1] + ny],
                [b[0] + nx, b[1] + ny],
                [b[0] - nx, b[1] - ny],
                [a[0] - nx, a[1] - ny],
            ];
            assert!(axis_aligned(&corners).is_some(), "{a:?} -> {b:?}");
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
