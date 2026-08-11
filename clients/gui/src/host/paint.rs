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
//!
//! [`Draw`] is the other half: the batch never travels alone, it travels with
//! the two role tables every paint site reads, and that triple is what a draw
//! function takes.

use super::layout::Rect;
use super::metrics::Metrics;
use super::theme::Theme;

/// What a paint site draws with: the batch it emits into, plus the **size
/// roles** and the **color roles** it reads them from.
///
/// The three are one context because no paint site ever has fewer: a rectangle
/// needs a color role, its inset needs a size role, and both land in the same
/// batch. Carrying them as three parameters made every draw function three
/// wider than its own subject, which is what the `too_many_arguments` allows on
/// them were really saying.
///
/// Both tables are **passed, never global**: each window resolves its own
/// metrics at its own `ui_scale` (see [`metrics`](super::metrics)), so the
/// context is built per frame by the renderer and handed down.
///
/// A draw function takes `&mut Draw` and either passes it on to the sub-draws
/// it delegates to, or — a leaf that paints itself — opens with
/// [`parts`](Self::parts) and works with the three directly.
pub struct Draw<'a> {
    /// The batch this site emits its triangles into.
    pub mesh: &'a mut Mesh,
    /// The size roles, resolved for the window being painted.
    pub m: &'a Metrics,
    /// The color roles.
    pub theme: &'a Theme,
}

impl<'a> Draw<'a> {
    pub fn new(mesh: &'a mut Mesh, m: &'a Metrics, theme: &'a Theme) -> Self {
        Self { mesh, m, theme }
    }

    /// The context's three parts, for a leaf that paints with them directly.
    pub fn parts(&mut self) -> (&mut Mesh, &Metrics, &Theme) {
        (self.mesh, self.m, self.theme)
    }
}

/// `[x, y, r, g, b, a]` per vertex, position in device pixels.
const FLOATS_PER_VERTEX: usize = 6;

/// `[x, y, u, v, r, g, b, a]` per glyph vertex — the same, plus where in the
/// atlas texture it samples.
#[cfg(feature = "font-atlas")]
const FLOATS_PER_GLYPH_VERTEX: usize = 8;

/// An RGBA color.
pub type Color = [f32; 4];

/// **How a widget's own triangles are emitted** — the two paint capabilities
/// that are a property of the *widget* rather than of a drawing site: its
/// resolved opacity and the corner radius of the boxes it lays down.
///
/// Both ride the [`Mesh`] beside its clip rectangle, for the same reason the
/// clip does: they apply to a *run* of triangles (everything one widget
/// contributes) rather than to one call, so no draw function grows a parameter
/// and no element has to be told it is being faded. The frame sets one of these
/// per placement, and every primitive emitted until the next one carries it.
///
/// The bound is deliberate and is the milestone's own: this is **per-primitive
/// alpha**, not layer compositing. Two overlapping shapes inside a faded widget
/// show through each other, because there is no second target to compose — and
/// a second target is exactly the batch the crate does not split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ink {
    /// The multiplier applied to every emitted vertex's alpha. `1.0` is opaque,
    /// which is what everything that says nothing draws at.
    pub alpha: f32,
    /// The corner radius of the axis-aligned boxes emitted while it is set, in
    /// **physical** pixels (the wire's logical number, through the placement's
    /// scale). `0.0` is a square corner. Clamped per box to half its shorter
    /// side, so a hairline keeps its shape and only a real box rounds.
    pub radius: f32,
}

impl Default for Ink {
    /// Opaque and square: what every widget draws at unless it asked otherwise.
    fn default() -> Self {
        Self {
            alpha: 1.0,
            radius: 0.0,
        }
    }
}

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

/// The four corner arcs of a rounded `r`, as `(centre x, centre y, the angle
/// the quarter starts at)` — top-left, top-right, bottom-right, bottom-left,
/// in a y-downwards space. One table, so a filled box and the frame around it
/// turn about the same centres.
fn corners(r: Rect, radius: f32) -> [(f32, f32, f32); 4] {
    let (x0, y0) = (r.x + radius, r.y + radius);
    let (x1, y1) = (r.x + r.w - radius, r.y + r.h - radius);
    [
        (x0, y0, std::f32::consts::PI),
        (x1, y0, 1.5 * std::f32::consts::PI),
        (x1, y1, 0.0),
        (x0, y1, 0.5 * std::f32::consts::PI),
    ]
}

/// A batch of flat-colored triangles in device-pixel space.
#[derive(Default)]
pub struct Mesh {
    verts: Vec<f32>,
    /// The **glyph** vertices of the `font-atlas` feature, when a face is
    /// loaded: `[x, y, u, v, r, g, b, a]` each, sampling the window's atlas
    /// texture. A second list rather than a second `Mesh` because the two are
    /// one picture: they share this batch's clip rectangle, they are uploaded
    /// together, and they are drawn back to back — the glyphs over the flat
    /// geometry of *their own* batch, which is where text always sat (the
    /// overlay batch still paints over both).
    #[cfg(feature = "font-atlas")]
    glyphs: Vec<f32>,
    /// The active clip rectangle, if any: every triangle emitted while it is
    /// set is clipped to it geometrically (so a scrolled widget's chrome never
    /// bleeds outside its `scroll` container). Geometry, not a GPU scissor —
    /// the batch stays **one** upload and one draw on every front.
    clip: Option<Rect>,
    /// The active [`Ink`]: the opacity every emitted vertex is multiplied by
    /// and the corner radius [`rect`](Self::rect) lays its boxes down with. Set
    /// per widget by the frame, exactly where the clip is.
    ink: Ink,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.verts.clear();
        #[cfg(feature = "font-atlas")]
        self.glyphs.clear();
    }

    pub fn is_empty(&self) -> bool {
        #[cfg(feature = "font-atlas")]
        if !self.glyphs.is_empty() {
            return false;
        }
        self.verts.is_empty()
    }

    /// A glyph quad: `r` in device pixels, textured with `uv` (`[u0, v0, u1,
    /// v1]` of the atlas) and tinted `color`.
    ///
    /// Clipping is the same clamp an axis-aligned [`quad`](Self::quad) takes —
    /// a glyph is always axis-aligned — with the texture coordinates cut in the
    /// same proportion, so half a letter at a `scroll`'s edge shows exactly its
    /// left half rather than a squeezed whole one.
    #[cfg(feature = "font-atlas")]
    pub fn glyph(&mut self, r: Rect, uv: [f32; 4], color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
        let [mut u0, mut v0, mut u1, mut v1] = uv;
        if let Some(clip) = self.clip {
            let (cx0, cy0) = (clip.x, clip.y);
            let (cx1, cy1) = (clip.x + clip.w, clip.y + clip.h);
            if x1 <= cx0 || x0 >= cx1 || y1 <= cy0 || y0 >= cy1 {
                return; // fully outside
            }
            let (du, dv) = ((u1 - u0) / r.w, (v1 - v0) / r.h);
            if x0 < cx0 {
                u0 += (cx0 - x0) * du;
                x0 = cx0;
            }
            if x1 > cx1 {
                u1 -= (x1 - cx1) * du;
                x1 = cx1;
            }
            if y0 < cy0 {
                v0 += (cy0 - y0) * dv;
                y0 = cy0;
            }
            if y1 > cy1 {
                v1 -= (y1 - cy1) * dv;
                y1 = cy1;
            }
        }
        let alpha = self.ink.alpha;
        let mut vertex = |x: f32, y: f32, u: f32, v: f32| {
            self.glyphs.extend_from_slice(&[
                x,
                y,
                u,
                v,
                color[0],
                color[1],
                color[2],
                color[3] * alpha,
            ]);
        };
        vertex(x0, y1, u0, v1);
        vertex(x1, y1, u1, v1);
        vertex(x1, y0, u1, v0);
        vertex(x0, y1, u0, v1);
        vertex(x1, y0, u1, v0);
        vertex(x0, y0, u0, v0);
    }

    /// The glyph vertices accumulated, `[x, y, u, v, r, g, b, a]` each.
    #[cfg(feature = "font-atlas")]
    pub(crate) fn glyph_vertices(&self) -> &[f32] {
        &self.glyphs
    }

    /// The alpha of every accumulated flat vertex — what a test asks to see
    /// the [`Ink`]'s opacity in the batch itself.
    #[cfg(test)]
    pub(crate) fn alphas(&self) -> impl Iterator<Item = f32> + '_ {
        self.verts.chunks_exact(FLOATS_PER_VERTEX).map(|v| v[5])
    }

    /// The `(x, y)` of every accumulated vertex, for bounds/layout tests —
    /// the flat geometry's and, where a face is drawing them, the glyphs'.
    #[cfg(test)]
    pub(crate) fn positions(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        let flat = self
            .verts
            .chunks_exact(FLOATS_PER_VERTEX)
            .map(|v| (v[0], v[1]));
        #[cfg(feature = "font-atlas")]
        let flat = flat.chain(
            self.glyphs
                .chunks_exact(FLOATS_PER_GLYPH_VERTEX)
                .map(|v| (v[0], v[1])),
        );
        flat
    }

    fn vertex(&mut self, p: [f32; 2], c: Color) {
        self.verts
            .extend_from_slice(&[p[0], p[1], c[0], c[1], c[2], c[3] * self.ink.alpha]);
    }

    /// Sets (or clears) the clip rectangle applied to everything emitted next.
    pub fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
    }

    /// Sets the [`Ink`] — the opacity and the corner radius — everything
    /// emitted next carries. The frame sets one per placed widget, beside its
    /// clip; [`Ink::default`] restores opaque square drawing.
    pub fn set_ink(&mut self, ink: Ink) {
        self.ink = Ink {
            alpha: ink.alpha.clamp(0.0, 1.0),
            radius: ink.radius.max(0.0),
        };
    }

    /// The [`Ink`] currently in force.
    pub fn ink(&self) -> Ink {
        self.ink
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

    /// An axis-aligned rectangle — **the box primitive**, and so the one that
    /// honors the active [`Ink`]'s corner radius: a widget that asked for
    /// rounded corners gets them on every box it lays down, and on nothing
    /// else. A line, a disc, a glyph and a raw quad keep their own shape,
    /// which is what stops a rounded widget from rounding its hairlines and
    /// its traces too.
    pub fn rect(&mut self, r: Rect, color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        if self.ink.radius > 0.0 {
            let radius = self.ink.radius;
            return self.round_rect(r, radius, color);
        }
        self.square_rect(r, color);
    }

    /// A rectangle with square corners, whatever the active [`Ink`] says — the
    /// primitive [`rect`](Self::rect) is when nothing asked for a radius, and
    /// what [`round_rect`](Self::round_rect) builds its straight parts from.
    fn square_rect(&mut self, r: Rect, color: Color) {
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

    /// An axis-aligned rectangle with `radius`-rounded corners, tessellated
    /// into **this same batch**: three straight bands plus a quarter fan per
    /// corner, no second pipeline and no texture — the way the score's outlines
    /// already reach the mesh.
    ///
    /// `radius` is clamped to half the shorter side (so it degenerates to a
    /// stadium, never to a self-crossing shape) and a radius under one pixel is
    /// a square corner: the arc would land inside a single pixel, which is a
    /// dozen triangles nobody can see. That clamp is also what lets the radius
    /// ride the [`Ink`] for a whole widget — a divider, a track edge and a tick
    /// are one or two pixels thick, so they come out unchanged while the
    /// widget's own box rounds.
    pub fn round_rect(&mut self, r: Rect, radius: f32, color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let radius = radius.min(r.w * 0.5).min(r.h * 0.5);
        if radius < 1.0 {
            return self.square_rect(r, color);
        }
        // The straight parts: a full-height middle band and the two caps
        // between the corners. Every one of them is axis-aligned, so each takes
        // the clip's cheap clamp.
        self.square_rect(Rect::new(r.x, r.y + radius, r.w, r.h - 2.0 * radius), color);
        self.square_rect(
            Rect::new(r.x + radius, r.y, r.w - 2.0 * radius, radius),
            color,
        );
        self.square_rect(
            Rect::new(r.x + radius, r.y + r.h - radius, r.w - 2.0 * radius, radius),
            color,
        );
        // ...and a quarter fan per corner: the ring from the centre out, which
        // is a fan because its inner radius is zero.
        for (cx, cy, from) in corners(r, radius) {
            self.corner_ring(cx, cy, from, radius, 0.0, color);
        }
    }

    /// One corner's arc as a strip between an outer and an inner radius (a fan
    /// when `inner` is zero) — the piece both the filled box and the frame are
    /// built from, so a rounded border follows exactly the edge its fill drew.
    fn corner_ring(&mut self, cx: f32, cy: f32, from: f32, outer: f32, inner: f32, color: Color) {
        // The segment count follows the radius (a corner is never more than a
        // few pixels of arc), the way `disc` fixes its own.
        let segments = ((outer * 0.75).ceil() as usize).clamp(2, 12);
        let step = std::f32::consts::FRAC_PI_2 / segments as f32;
        for i in 0..segments {
            let (a0, a1) = (from + i as f32 * step, from + (i + 1) as f32 * step);
            let at = |a: f32, rad: f32| [cx + rad * a.cos(), cy + rad * a.sin()];
            self.tri(at(a0, inner), at(a0, outer), at(a1, outer), color);
            if inner > 0.0 {
                self.tri(at(a0, inner), at(a1, outer), at(a1, inner), color);
            }
        }
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

    /// A `w`-pixel-thick outline of `rect` — four edge rectangles, or a
    /// rounded frame when the active [`Ink`] carries a radius, so a widget's
    /// edge (and the focus ring the frame draws over it) follows the box its
    /// fill drew rather than cutting its corners off.
    pub fn border(&mut self, rect: Rect, w: f32, color: Color) {
        if self.ink.radius > 0.0 {
            let radius = self.ink.radius;
            return self.round_border(rect, w, radius, color);
        }
        self.square_border(rect, w, color);
    }

    /// The four edge strips of a square outline, whatever the [`Ink`] says.
    fn square_border(&mut self, rect: Rect, w: f32, color: Color) {
        self.square_rect(Rect::new(rect.x, rect.y, rect.w, w), color);
        self.square_rect(Rect::new(rect.x, rect.y + rect.h - w, rect.w, w), color);
        self.square_rect(Rect::new(rect.x, rect.y, w, rect.h), color);
        self.square_rect(Rect::new(rect.x + rect.w - w, rect.y, w, rect.h), color);
    }

    /// A `w`-thick frame around `rect` with `radius`-rounded corners: four
    /// straight edge strips between the corners, and a ring segment per corner
    /// from `radius` in to `radius - w` (a fan where the frame is thicker than
    /// the corner is round).
    pub fn round_border(&mut self, rect: Rect, w: f32, radius: f32, color: Color) {
        if rect.w <= 0.0 || rect.h <= 0.0 || w <= 0.0 {
            return;
        }
        let radius = radius.min(rect.w * 0.5).min(rect.h * 0.5);
        if radius < 1.0 {
            return self.square_border(rect, w, color);
        }
        let span_w = (rect.w - 2.0 * radius).max(0.0);
        let span_h = (rect.h - 2.0 * radius).max(0.0);
        self.square_rect(Rect::new(rect.x + radius, rect.y, span_w, w), color);
        self.square_rect(
            Rect::new(rect.x + radius, rect.y + rect.h - w, span_w, w),
            color,
        );
        self.square_rect(Rect::new(rect.x, rect.y + radius, w, span_h), color);
        self.square_rect(
            Rect::new(rect.x + rect.w - w, rect.y + radius, w, span_h),
            color,
        );
        let inner = (radius - w).max(0.0);
        for (cx, cy, from) in corners(rect, radius) {
            self.corner_ring(cx, cy, from, radius, inner, color);
        }
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

    /// The bounding box of everything accumulated, `None` for an empty mesh —
    /// how much of the area a drawing actually inked, which is the one thing a
    /// test can ask about a picture without a window.
    #[cfg(test)]
    pub(crate) fn extent(&self) -> Option<Rect> {
        let mut it = self.positions();
        let (fx, fy) = it.next()?;
        let (mut x0, mut x1) = (fx, fx);
        let (mut y0, mut y1) = (fy, fy);
        for (x, y) in it {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// Uploads a [`Mesh`] and draws it.
pub struct Painter {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
    /// The glyph half of this batch, when the crate was built with a
    /// rasterizer. Absent until the batch first carries a glyph, so a build
    /// with the feature on and no face pays no texture.
    #[cfg(feature = "font-atlas")]
    text: Option<TextLayer>,
    /// The pass this painter draws into, kept for the glyph layer's pipeline:
    /// it is built on the first batch that carries a letter, and it has to
    /// agree with the flat one on the format **and** the sample count.
    #[cfg(feature = "font-atlas")]
    target: crate::view::Target,
}

/// The textured pipeline and this window's copy of the glyph atlas.
#[cfg(feature = "font-atlas")]
struct TextLayer {
    pipeline: wgpu::RenderPipeline,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
    /// The atlas version this texture holds: the sheet is re-uploaded only when
    /// the shared cache has rasterized something new.
    version: u64,
}

#[cfg(feature = "font-atlas")]
impl TextLayer {
    fn new(device: &wgpu::Device, target: crate::view::Target) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("paint_text.wgsl").into()),
        });
        let side = super::font::atlas::SIDE;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // One coverage byte per texel: the smallest thing WebGL2 and
            // WebGPU both sample, which is what a glyph sheet is.
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Nearest: a glyph is rasterized at the size it draws at and its quad
        // is snapped to whole pixels, so every texel lands on its own pixel.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph atlas sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph bind layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph bind group"),
            layout: &bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyph pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let float = std::mem::size_of::<f32>() as wgpu::BufferAddress;
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: FLOATS_PER_GLYPH_VERTEX as wgpu::BufferAddress * float,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 2 * float,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 4 * float,
                    shader_location: 2,
                },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph pipeline"),
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
                    format: target.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: target.multisample(),
            multiview_mask: None,
            cache: None,
        });
        let capacity_vertices = 6 * 256;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph vertices"),
            size: capacity_vertices * FLOATS_PER_GLYPH_VERTEX as u64 * float,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            texture,
            bind_group,
            vertex_buffer,
            capacity_vertices,
            num_vertices: 0,
            // 0 is "nothing uploaded": the atlas starts at 0 too and only
            // counts up once it holds a glyph, so the first sheet is copied.
            version: 0,
        }
    }

    /// Copies the shared cache's coverage sheet into this window's texture, if
    /// this copy is behind it.
    fn sync(&mut self, queue: &wgpu::Queue, atlas: &super::font::atlas::Atlas) {
        let pixels = atlas.pixels();
        if self.version == atlas.version() || pixels.is_empty() {
            return;
        }
        let side = super::font::atlas::SIDE;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(side),
                rows_per_image: Some(side),
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
        self.version = atlas.version();
    }
}

impl Painter {
    pub fn new(device: &wgpu::Device, target: crate::view::Target) -> Self {
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
                    format: target.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: target.multisample(),
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
            #[cfg(feature = "font-atlas")]
            text: None,
            #[cfg(feature = "font-atlas")]
            target,
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
        #[cfg(feature = "font-atlas")]
        self.upload_glyphs(device, queue, mesh, fw, fh);
    }

    /// The glyph half of the same batch: the vertices into this painter's
    /// buffer, and — only when the shared cache has rasterized something new —
    /// the coverage sheet into this window's texture.
    #[cfg(feature = "font-atlas")]
    fn upload_glyphs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        fw: f32,
        fh: f32,
    ) {
        let verts = mesh.glyph_vertices();
        if verts.is_empty() {
            if let Some(text) = &mut self.text {
                text.num_vertices = 0;
            }
            return;
        }
        let text = self
            .text
            .get_or_insert_with(|| TextLayer::new(device, self.target));
        let mut clip = Vec::with_capacity(verts.len());
        for v in verts.chunks_exact(FLOATS_PER_GLYPH_VERTEX) {
            clip.push((v[0] / fw) * 2.0 - 1.0);
            clip.push(1.0 - (v[1] / fh) * 2.0);
            clip.extend_from_slice(&v[2..FLOATS_PER_GLYPH_VERTEX]);
        }
        let needed = (verts.len() / FLOATS_PER_GLYPH_VERTEX) as u64;
        if needed > text.capacity_vertices {
            text.capacity_vertices = needed.next_power_of_two();
            text.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("glyph vertices"),
                size: text.capacity_vertices
                    * FLOATS_PER_GLYPH_VERTEX as u64
                    * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&text.vertex_buffer, 0, bytemuck::cast_slice(&clip));
        text.num_vertices = needed as u32;
        super::font::atlas::with(|atlas| text.sync(queue, atlas));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.num_vertices > 0 {
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.num_vertices, 0..1);
        }
        // The glyphs of this batch, over its own flat geometry: one bind group
        // and one draw for every letter in the window.
        #[cfg(feature = "font-atlas")]
        if let Some(text) = &self.text
            && text.num_vertices > 0
        {
            pass.set_pipeline(&text.pipeline);
            pass.set_bind_group(0, &text.bind_group, &[]);
            pass.set_vertex_buffer(0, text.vertex_buffer.slice(..));
            pass.draw(0..text.num_vertices, 0..1);
        }
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

    /// The opacity reaches the batch the only way it can without a second
    /// target: every vertex the widget emits carries it, glyphs included, and
    /// nothing else in the mesh is touched.
    #[test]
    fn the_inks_opacity_multiplies_every_emitted_alpha() {
        let mut m = Mesh::new();
        m.set_ink(Ink {
            alpha: 0.5,
            ..Ink::default()
        });
        m.rect(Rect::new(0.0, 0.0, 10.0, 10.0), [1.0, 1.0, 1.0, 1.0]);
        // A color that is already translucent composes with it rather than
        // being replaced — a selection band at 0.18 inside a widget at 0.5 is
        // fainter, not reset.
        m.rect(Rect::new(0.0, 0.0, 10.0, 10.0), [1.0, 1.0, 1.0, 0.4]);
        let alphas: Vec<f32> = m.alphas().collect();
        assert_eq!(alphas.len(), 12);
        assert!(alphas[..6].iter().all(|a| (a - 0.5).abs() < 1e-6));
        assert!(alphas[6..].iter().all(|a| (a - 0.2).abs() < 1e-6));
        // ...and the default is opaque, which is what every widget that says
        // nothing draws at.
        m.set_ink(Ink::default());
        m.rect(Rect::new(0.0, 0.0, 10.0, 10.0), [1.0; 4]);
        assert!(m.alphas().skip(12).all(|a| a == 1.0));
    }

    /// A rounded box is the same box with its corners cut: it never leaves its
    /// rectangle, it still fills the middle and the edges, and the corner pixel
    /// it used to cover is gone.
    #[test]
    fn a_radius_cuts_the_corners_and_keeps_the_box() {
        let r = Rect::new(10.0, 10.0, 80.0, 40.0);
        let mut m = Mesh::new();
        m.set_ink(Ink {
            radius: 8.0,
            ..Ink::default()
        });
        m.rect(r, [1.0; 4]);
        for (x, y) in m.positions() {
            assert!(
                (10.0..=90.0).contains(&x) && (10.0..=50.0).contains(&y),
                "the rounding stays inside the rect: ({x}, {y})"
            );
        }
        assert!(covers(&m, 50.0, 30.0), "the middle is filled");
        assert!(covers(&m, 11.0, 30.0), "the left edge is filled");
        assert!(covers(&m, 50.0, 11.0), "the top edge is filled");
        assert!(!covers(&m, 10.6, 10.6), "the corner is cut");
        assert!(!covers(&m, 89.4, 49.4), "every corner is cut");
    }

    /// The clamp is what lets one radius ride a whole widget: a hairline (a
    /// divider, a tick, a track edge) has no room for an arc, so it comes out
    /// exactly as it always did, while the widget's own box rounds.
    #[test]
    fn a_hairline_is_unchanged_by_a_radius_the_box_uses() {
        let hairline = Rect::new(0.0, 0.0, 100.0, 1.0);
        let mut square = Mesh::new();
        square.rect(hairline, [1.0; 4]);
        let mut rounded = Mesh::new();
        rounded.set_ink(Ink {
            radius: 6.0,
            ..Ink::default()
        });
        rounded.rect(hairline, [1.0; 4]);
        assert_eq!(rounded.vertex_count(), square.vertex_count());
        assert_eq!(
            rounded.positions().collect::<Vec<_>>(),
            square.positions().collect::<Vec<_>>()
        );
    }

    /// A frame follows the box its fill drew: the corner the fill cut is not
    /// squared off by the border (or by the focus ring, which is one).
    #[test]
    fn a_border_follows_the_rounded_box() {
        let r = Rect::new(10.0, 10.0, 80.0, 40.0);
        let mut m = Mesh::new();
        m.set_ink(Ink {
            radius: 8.0,
            ..Ink::default()
        });
        m.border(r, 2.0, [1.0; 4]);
        assert!(covers(&m, 50.0, 10.6), "the top edge is drawn");
        assert!(covers(&m, 10.6, 30.0), "the left edge is drawn");
        assert!(!covers(&m, 10.6, 10.6), "and the corner is not squared off");
        assert!(!covers(&m, 50.0, 30.0), "a frame is not a fill");
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
