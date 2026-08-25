//! **The painting**: the lyon fill, the painter's strokes, and the drag's
//! ledger lines.
//!
//! Everything that puts a triangle in the mesh lives here — the page's
//! primitives ([`ScoreData::render`]), the selection highlight, the playback
//! cursor's line, and the ledger lines a pitch drag owes the page while it is
//! in flight. Glyphs are filled through lyon in the path's **own** coordinate
//! space and the vertices mapped afterwards, which is cheaper than transforming
//! every bezier control point and correct because the page transform is affine.
//!
//! The one geometric subtlety is [`xf_shrink`]: a tolerance expressed in a
//! glyph's local font units has to be pre-divided by the local scale, or a
//! notehead flattens to a different smoothness than the staff line beside it.

use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};

use super::glyphs::build_path;
use super::{Affine, Prim, ScoreColors, ScoreData, Staff};
use crate::host::layout::Rect;
use crate::host::paint::{Color, Mesh};

impl ScoreData {
    /// `fit`, shifted by the drag preview when `id` is the element being
    /// dragged — the one place the displacement enters the drawing, so every
    /// primitive of a note (notehead, stem, dots) travels with it.
    pub(super) fn prim_fit(&self, fit: Affine, id: Option<&str>) -> Affine {
        match &self.drag {
            // page y grows downward, so a step up is a negative offset
            Some(d) if id == Some(d.id.as_str()) => Affine {
                ty: fit.ty - fit.sy * d.steps as f32 * self.step,
                ..fit
            },
            _ => fit,
        }
    }

    /// Tessellate the whole page into `mesh`, mapped into `rect` by [`fit`] and
    /// painted in `colors`: the engraving in the ink, the selected element
    /// highlighted under it, and the playback cursor over it at musical time
    /// `head` ms (negative = none, as returned by [`head_ms`]). Geometry is
    /// clipped to `rect` intersected with the caller's `clip` (the enclosing
    /// scroll area, if any); the caller's clip is restored on return so the
    /// surrounding frame pass is unaffected.
    ///
    /// [`fit`]: ScoreData::fit
    /// [`head_ms`]: ScoreData::head_ms
    pub fn render(
        &self,
        mesh: &mut Mesh,
        rect: Rect,
        clip: Option<Rect>,
        head: f32,
        colors: ScoreColors,
    ) {
        let color = colors.ink;
        let fit = self.fit(rect);
        mesh.set_clip(Some(intersect(rect, clip)));
        // Curve-flattening tolerance in page units so it lands ~1/3 device
        // pixel after fitting — fine enough to read smooth, coarse enough to
        // keep the triangle count bounded by the screen, not the notation.
        let tol_page = 0.33 / fit.sx.max(f32::MIN_POSITIVE);
        // under the ink, so the engraving still reads through the highlight
        self.draw_selection(mesh, fit, colors.selection);
        // A dragged notehead takes its ledger lines with it: the engraved ones
        // stay where the staff put them, so they are dropped and re-derived at
        // the displaced pitch — which is also how they disappear when the note
        // comes back onto the staff.
        let ledgers = self.drag_ledgers();
        if let Some(l) = &ledgers {
            let w = (l.width * fit.sx).max(1.0);
            for y in &l.ys {
                mesh.line(fit.apply(l.x0, *y), fit.apply(l.x1, *y), w, color);
            }
        }
        let mut tess = FillTessellator::new();
        for prim in &self.prims {
            if ledgers.as_ref().is_some_and(|l| l.covers(prim, self.vb_w)) {
                continue;
            }
            // the page fit, displaced while this element is being dragged
            let fit = self.prim_fit(fit, prim.id());
            match prim {
                Prim::Line { pts, width, .. } => {
                    let w = (width * fit.sx).max(1.0);
                    for seg in pts.windows(2) {
                        mesh.line(
                            fit.apply(seg[0][0], seg[0][1]),
                            fit.apply(seg[1][0], seg[1][1]),
                            w,
                            color,
                        );
                    }
                }
                Prim::Glyph { cp, xf, .. } => {
                    if let Some(d) = self.glyphs.get(cp) {
                        // font -> page (xf) -> screen (fit): still translate+scale.
                        fill_path(
                            mesh,
                            &mut tess,
                            d,
                            fit.then(*xf),
                            tol_page * xf_shrink(*xf),
                            color,
                        );
                    }
                }
                Prim::Fill { d, xf, .. } => {
                    fill_path(
                        mesh,
                        &mut tess,
                        d,
                        fit.then(*xf),
                        tol_page * xf_shrink(*xf),
                        color,
                    );
                }
                Prim::Text { s, x, y, size, .. } => {
                    // baseline -> top-left for the host font; em height in px.
                    let em = (size * fit.sy).abs();
                    let scale = (em / crate::host::font::GLYPH_H as f32).max(0.5);
                    let [sx, sy] = fit.apply(*x, *y);
                    crate::host::font::text(mesh, s, sx, sy - em, scale, color);
                }
            }
        }
        self.draw_playhead(mesh, fit, head, colors.playhead);
        mesh.set_clip(clip);
    }

    /// Highlight every primitive of the selected element — one MEI id can own
    /// several (a note is a notehead plus its stem), so the whole gesture of it
    /// lights up rather than one glyph of it.
    fn draw_selection(&self, mesh: &mut Mesh, fit: Affine, color: Color) {
        let Some(sel) = self.selected.as_deref() else {
            return;
        };
        let fit = self.prim_fit(fit, Some(sel));
        for h in self.hits.iter().filter(|h| h.id == sel) {
            // a hair of page-unit padding so a hairline stem still shows a band
            let b = h.bounds.grown(20.0).transformed(fit);
            mesh.rect(
                Rect::new(b.x0, b.y0, b.x1 - b.x0, b.y1 - b.y0),
                crate::host::theme::with_alpha(color, 0.30),
            );
        }
    }

    /// Draw the playback cursor at musical time `head` (ms): the vertical
    /// staff-spanning line of the latest cursor at or before it. A no-op when no
    /// playhead is set or no timemap was sent.
    fn draw_playhead(&self, mesh: &mut Mesh, fit: Affine, head: f32, color: Color) {
        if head < 0.0 || self.cursors.is_empty() {
            return;
        }
        // the cursor active at `head`: the last one whose time is <= it.
        let idx = self
            .cursors
            .partition_point(|c| c.t <= head)
            .saturating_sub(1);
        let c = self.cursors[idx.min(self.cursors.len() - 1)];
        // points are already in screen pixels after `fit`, so width is px.
        mesh.line(fit.apply(c.x, c.y0), fit.apply(c.x, c.y1), 2.0, color);
    }

    /// The ledger lines a notehead centred on page-y `y` needs on `staff`,
    /// outward from it — empty while the note is on the staff. One line per
    /// whole line position past the staff's own, and a note in the space
    /// *beyond* the last one gains no further line: the engraving rule, and the
    /// reason this is not simply "one line per step".
    pub fn ledger_ys(&self, staff: Staff, y: f32) -> Vec<f32> {
        let space = 2.0 * self.step;
        let (mut ly, dir) = if y < staff.y0 {
            (staff.y0 - space, -1.0)
        } else {
            (staff.y1 + space, 1.0)
        };
        let mut out = Vec::new();
        // the cap keeps a degenerate page (or a drag off into nowhere) finite
        while (y - ly) * dir >= -0.01 && out.len() < 32 {
            out.push(ly);
            ly += dir * space;
        }
        out
    }

    /// The ledger lines the drag preview owes the page: where the displaced
    /// notehead needs them, how wide, and the box whose engraved ones it
    /// replaces. `None` when nothing is being dragged, the element is not on a
    /// staff, or it left no measurable mark.
    fn drag_ledgers(&self) -> Option<Ledgers> {
        let drag = self.drag.as_ref()?;
        // the element's first primitive is its notehead (verovio draws it
        // before the stem), which is what a ledger line is centred on and sized
        // from — the stem and flag would stretch the box out of shape.
        let head = self.hits.iter().find(|h| h.id == drag.id)?.bounds;
        let y = 0.5 * (head.y0 + head.y1);
        let staff = self.staff_at(y)?;
        let pad = LEDGER_OVERHANG * (head.x1 - head.x0);
        Some(Ledgers {
            ys: self.ledger_ys(staff, y - drag.steps as f32 * self.step),
            x0: head.x0 - pad,
            x1: head.x1 + pad,
            width: staff.width * LEDGER_WEIGHT,
            staff,
        })
    }
}

/// A ledger line reaches past the notehead by about a fifth of its width on
/// each side, and is stroked heavier than a staff line — verovio's proportions,
/// so a previewed ledger sits among the engraved ones without looking foreign.
const LEDGER_OVERHANG: f32 = 0.22;
const LEDGER_WEIGHT: f32 = 1.7;

/// What the drag preview owes the page in ledger lines: `ys` to draw across
/// `x0..x1`, and the `staff` they belong to — which is also what identifies the
/// engraved ledger lines the dragged notehead is leaving behind.
struct Ledgers {
    ys: Vec<f32>,
    x0: f32,
    x1: f32,
    width: f32,
    staff: Staff,
}

impl Ledgers {
    /// Whether this primitive is a ledger line of the dragged notehead — a
    /// short horizontal stroke off the staff, over the notehead's own column.
    /// The engraver draws them per staff, not inside the note, so they carry
    /// the staff's id and cannot travel with it: they are dropped from the
    /// drawing and re-derived at the displaced position instead.
    fn covers(&self, prim: &Prim, vb_w: f32) -> bool {
        let Prim::Line { pts, .. } = prim else {
            return false;
        };
        if pts.len() != 2 || (pts[0][1] - pts[1][1]).abs() > 1.0 {
            return false;
        }
        let (x0, x1) = (pts[0][0].min(pts[1][0]), pts[0][0].max(pts[1][0]));
        let y = pts[0][1];
        (x1 - x0) < 0.15 * vb_w
            && staff_distance(&self.staff, y) > 0.0
            && x0 < self.x1
            && x1 > self.x0
    }
}

/// How far a page-y sits outside a staff (zero anywhere between its lines).
pub(super) fn staff_distance(staff: &Staff, y: f32) -> f32 {
    (staff.y0 - y).max(y - staff.y1).max(0.0)
}

/// `rect` clamped to `clip` (or `rect` itself when there is no outer clip).
fn intersect(rect: Rect, clip: Option<Rect>) -> Rect {
    let Some(c) = clip else { return rect };
    let x0 = rect.x.max(c.x);
    let y0 = rect.y.max(c.y);
    let x1 = (rect.x + rect.w).min(c.x + c.w);
    let y1 = (rect.y + rect.h).min(c.y + c.h);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// How much a glyph's own transform shrinks page units, so the tolerance passed
/// to [`fill_path`] (expressed in the glyph's *local* font units) still lands at
/// the same on-screen size. `fill_path` flattens in the path's own coordinates
/// then maps, so its tolerance must be pre-divided by the local scale.
fn xf_shrink(xf: Affine) -> f32 {
    1.0 / xf.sx.abs().max(f32::MIN_POSITIVE)
}

/// Parse an SVG path `d`, flatten + fill it with lyon, and emit the triangles
/// into `mesh` after mapping each vertex through `xf`. Tessellation happens in
/// the path's own coordinate space (tolerance `tol`), then the resulting
/// vertices are mapped — cheaper than transforming every bezier control point,
/// and correct because `xf` is affine.
pub(super) fn fill_path(
    mesh: &mut Mesh,
    tess: &mut FillTessellator,
    d: &str,
    xf: Affine,
    tol: f32,
    color: Color,
) {
    let Some(path) = build_path(d) else { return };
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let opts = FillOptions::tolerance(tol.max(f32::MIN_POSITIVE)).with_fill_rule(FillRule::NonZero);
    let ok = tess.tessellate_path(
        &path,
        &opts,
        &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
            let p = v.position();
            [p.x, p.y]
        }),
    );
    if ok.is_err() {
        return;
    }
    for tri in buffers.indices.as_chunks::<3>().0 {
        let a = buffers.vertices[tri[0] as usize];
        let b = buffers.vertices[tri[1] as usize];
        let c = buffers.vertices[tri[2] as usize];
        mesh.tri(
            xf.apply(a[0], a[1]),
            xf.apply(b[0], b[1]),
            xf.apply(c[0], c[1]),
            color,
        );
    }
}
