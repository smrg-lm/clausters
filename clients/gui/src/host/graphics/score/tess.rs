//! **The painting**: the lyon fill, the painter's strokes, and the drag's
//! ledger lines.
//!
//! Everything that puts a triangle in the mesh lives here -- the page's
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
use super::{Affine, FillOf, Prim, ScoreColors, ScoreData, Staff};
use crate::host::layout::Rect;
use crate::host::paint::{Color, Mesh};

impl ScoreData {
    /// `fit`, shifted by the drag preview when `id` is the element being
    /// dragged -- the one place the displacement enters the drawing, so every
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
        // pixel after fitting -- fine enough to read smooth, coarse enough to
        // keep the triangle count bounded by the screen, not the notation.
        let tol_page = 0.33 / fit.sx.max(f32::MIN_POSITIVE);
        // under the ink, so the engraving still reads through the highlight
        self.draw_selection(mesh, fit, colors.selection);
        // the edit cursor of note entry, under the ink as the selection is,
        // in the cursors' color so that it is not taken for what is selected
        if let Some(b) = self.edit_cursor_box() {
            let b = b.transformed(fit);
            mesh.rect(
                Rect::new(b.x0, b.y0, b.x1 - b.x0, b.y1 - b.y0),
                crate::host::theme::with_alpha(colors.playhead, 0.35),
            );
        }
        // A dragged notehead takes its ledger lines with it: the engraved ones
        // stay where the staff put them, so they are dropped and re-derived at
        // the displaced pitch -- which is also how they disappear when the note
        // comes back onto the staff.
        let ledgers = self.drag_ledgers();
        if let Some(l) = &ledgers {
            let w = (l.width * fit.sx).max(1.0);
            for y in &l.ys {
                mesh.line(fit.apply(l.x0, *y), fit.apply(l.x1, *y), w, color);
            }
        }
        // a fill mapped to the screen: font or page units, through `xf`
        let filled = |mesh: &mut Mesh, fill: &[[f32; 2]], xf: Affine| {
            for corner in fill.as_chunks::<3>().0 {
                mesh.tri(
                    xf.apply(corner[0][0], corner[0][1]),
                    xf.apply(corner[1][0], corner[1][1]),
                    xf.apply(corner[2][0], corner[2][1]),
                    color,
                );
            }
        };
        for (at, prim) in self.prims.iter().enumerate() {
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
                        // One fill to the glyph, however often it is placed.
                        let tol = tol_page * xf_shrink(*xf);
                        let fill = self.fills.of(FillOf::Glyph(*cp), d, tol);
                        filled(mesh, &fill, fit.then(*xf));
                    }
                }
                Prim::Fill { d, xf, .. } => {
                    let tol = tol_page * xf_shrink(*xf);
                    let fill = self.fills.of(FillOf::Prim(at), d, tol);
                    filled(mesh, &fill, fit.then(*xf));
                }
                Prim::Text {
                    s,
                    x,
                    y,
                    size,
                    anchor,
                    id,
                } => {
                    // a text being typed over is drawn as it stands
                    let editing = self
                        .editing
                        .as_ref()
                        .filter(|edit| id.as_deref() == Some(edit.id.as_str()));
                    let s = editing.map_or(s.as_str(), |edit| edit.value.as_str());
                    // **The size is the em, and the host's line is the
                    // capitals.** A font size names the em; the host's scale is
                    // set by the body box, which a face's capitals fill. Taking
                    // the one for the other drew a page's text at 1.4 times the
                    // size the engraver laid it out for, and two lines of a
                    // head that the engraver had kept apart ran together.
                    let cap = (size * fit.sy).abs() * super::CAP_PER_EM;
                    let scale = (cap / crate::host::font::GLYPH_H as f32).max(0.5);
                    let [sx, sy] = fit.apply(*x, *y);
                    // The anchor is resolved here and nowhere earlier, because
                    // it takes the width of the string *in the host's font*,
                    // which is the one thing the engraver could not know.
                    let left = anchor.left(sx, crate::host::font::width(s, scale));
                    // baseline -> the body box's top
                    let top = sy - cap;
                    if let Some(edit) = editing {
                        use crate::host::font;
                        // what is selected, behind the text; the caret, over it
                        let at = |pos: usize| {
                            let cols = edit.value[..pos.min(edit.value.len())].chars().count();
                            left + font::prefix_width(&edit.value, cols, scale)
                        };
                        let (above, below) = (cap * 0.25, cap * 0.3);
                        if let Some((from, to)) = edit.caret.selection() {
                            mesh.rect(
                                Rect::new(
                                    at(from),
                                    top - above,
                                    at(to) - at(from),
                                    cap + above + below,
                                ),
                                crate::host::theme::with_alpha(colors.selection, 0.45),
                            );
                        }
                        font::text(mesh, s, left, top, scale, color);
                        let caret = at(edit.caret.pos).round();
                        mesh.rect(
                            Rect::new(
                                caret,
                                top - above,
                                scale.max(1.0).round(),
                                cap + above + below,
                            ),
                            color,
                        );
                    } else {
                        crate::host::font::text(mesh, s, left, top, scale, color);
                    }
                }
            }
        }
        self.draw_playhead(mesh, fit, head, colors.playhead);
        mesh.set_clip(clip);
    }

    /// Highlight every primitive of the selected element -- one MEI id can own
    /// several (a note is a notehead plus its stem), so the whole gesture of it
    /// lights up rather than one glyph of it.
    fn draw_selection(&self, mesh: &mut Mesh, fit: Affine, color: Color) {
        // a text being typed over shows what is selected *in* it instead
        let typed = self.editing.as_ref().map(|edit| edit.id.as_str());
        for sel in self
            .selected
            .iter()
            .filter(|sel| Some(sel.as_str()) != typed)
        {
            let fit = self.prim_fit(fit, Some(sel));
            for h in self.boxes_of(sel) {
                // a hair of page-unit padding so a hairline stem still shows a band
                let b = h.bounds.grown(20.0).transformed(fit);
                mesh.rect(
                    Rect::new(b.x0, b.y0, b.x1 - b.x0, b.y1 - b.y0),
                    crate::host::theme::with_alpha(color, 0.30),
                );
            }
        }
    }

    /// **The playback cursor alone**, at musical time `head` ms, cut to `rect`
    /// and the caller's `clip` as [`render`](Self::render) cuts the page: what
    /// a clock moves of a page, for a caller that keeps the rest.
    pub fn render_playhead(
        &self,
        mesh: &mut Mesh,
        rect: Rect,
        clip: Option<Rect>,
        head: f32,
        color: Color,
    ) {
        mesh.set_clip(Some(intersect(rect, clip)));
        self.draw_playhead(mesh, self.fit(rect), head, color);
        mesh.set_clip(clip);
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
    /// outward from it -- empty while the note is on the staff. One line per
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
        // from -- the stem and flag would stretch the box out of shape.
        let head = self.boxes_of(&drag.id).next()?.bounds;
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
/// each side, and is stroked heavier than a staff line -- verovio's proportions,
/// so a previewed ledger sits among the engraved ones without looking foreign.
const LEDGER_OVERHANG: f32 = 0.22;
const LEDGER_WEIGHT: f32 = 1.7;

/// What the drag preview owes the page in ledger lines: `ys` to draw across
/// `x0..x1`, and the `staff` they belong to -- which is also what identifies the
/// engraved ledger lines the dragged notehead is leaving behind.
struct Ledgers {
    ys: Vec<f32>,
    x0: f32,
    x1: f32,
    width: f32,
    staff: Staff,
}

impl Ledgers {
    /// Whether this primitive is a ledger line of the dragged notehead -- a
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
/// to the fill (expressed in the glyph's *local* font units) still lands at
/// the same on-screen size. A fill is flattened in the path's own coordinates
/// and then mapped, so its tolerance must be pre-divided by the local scale.
pub(super) fn xf_shrink(xf: Affine) -> f32 {
    1.0 / xf.sx.abs().max(f32::MIN_POSITIVE)
}

/// **The fill of the path `d`, as triangle corners** in the path's own
/// coordinates, three to a triangle -- nothing for a path that fills nothing.
///
/// What a caller keeps when it draws one outline many times at sizes it does
/// not know yet: the triangles are tessellated once, at tolerance `tol` in the
/// path's units, and mapped where they are drawn.
pub fn triangles(d: &str, tol: f32) -> Vec<[f32; 2]> {
    let Some(path) = build_path(d) else {
        return Vec::new();
    };
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let opts = FillOptions::tolerance(tol.max(f32::MIN_POSITIVE)).with_fill_rule(FillRule::NonZero);
    let filled = FillTessellator::new().tessellate_path(
        &path,
        &opts,
        &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
            let p = v.position();
            [p.x, p.y]
        }),
    );
    if filled.is_err() {
        return Vec::new();
    }
    buffers
        .indices
        .iter()
        .map(|&i| buffers.vertices[i as usize])
        .collect()
}

/// **The edges of the path `d`**: its contours flattened at tolerance `tol`,
/// one segment after another, every contour closed -- nothing for a path that
/// draws nothing. What a caller that draws an outline small keeps beside its
/// fill, since an edge is where a stroke thinner than a pixel is lost.
pub fn edges(d: &str, tol: f32) -> Vec<[[f32; 2]; 2]> {
    use lyon::path::PathEvent;
    use lyon::path::iterator::PathIterator;

    let Some(path) = build_path(d) else {
        return Vec::new();
    };
    path.iter()
        .flattened(tol.max(f32::MIN_POSITIVE))
        .filter_map(|event| match event {
            PathEvent::Line { from, to } => Some([[from.x, from.y], [to.x, to.y]]),
            PathEvent::End { last, first, .. } => Some([[last.x, last.y], [first.x, first.y]]),
            _ => None,
        })
        .filter(|[a, b]| a != b)
        .collect()
}
