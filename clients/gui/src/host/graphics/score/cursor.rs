//! **What is where, and what time it is**: the page's two indexes and the
//! questions asked of them.
//!
//! An engraved page is a flat list of primitives, and everything interactive
//! needs it as something else: the **hit index** ([`ScoreData::index`]) — one
//! page-unit box per identified primitive, so a click can name the element
//! under it — and the **staff index** (`index_staves`), which is what a ledger
//! line and a diatonic step are measured against. Both are built once, when
//! the display list arrives, because the geometry never moves afterwards and
//! re-deriving it per click would mean re-parsing every glyph outline on every
//! press.
//!
//! Beside them, the two mappings a gesture needs: [`ScoreData::fit`], the
//! transform placing the page in its rectangle (the one every screen
//! coordinate goes through, in both directions), and [`ScoreData::head_ms`],
//! which reads the transport's position as a musical time on the client's
//! timemap.

use std::collections::HashMap;

use super::glyphs::path_bounds;
use super::tess::staff_distance;
use super::{Affine, Bounds, HitBox, HitShape, Prim, ScoreData, Staff, is_notehead};
use crate::host::layout::Rect;

impl ScoreData {
    /// Rebuild the hit-testing index from the placed primitives: one page-unit
    /// box per identified primitive, so a click can name the element under it.
    /// Done once when the display list arrives — the geometry never moves
    /// afterwards, and re-deriving it per click would mean re-parsing every
    /// glyph outline on every press.
    pub fn index(&mut self) {
        // glyph outlines repeat all over a page (one notehead shape, hundreds of
        // notes), so each codepoint's local extent is measured once.
        let mut local: HashMap<u32, Option<Bounds>> = HashMap::new();
        self.hits.clear();
        for prim in &self.prims {
            let Some(id) = prim.id() else { continue };
            let bounds = match prim {
                Prim::Glyph { cp, xf, .. } => {
                    let b = *local
                        .entry(*cp)
                        .or_insert_with(|| self.glyphs.get(cp).and_then(|d| path_bounds(d)));
                    b.map(|b| b.transformed(*xf))
                }
                Prim::Fill { d, xf, .. } => path_bounds(d).map(|b| b.transformed(*xf)),
                Prim::Line { pts, width, .. } => {
                    let mut b = Bounds {
                        x0: f32::MAX,
                        y0: f32::MAX,
                        x1: f32::MIN,
                        y1: f32::MIN,
                    };
                    for p in pts {
                        b.x0 = b.x0.min(p[0]);
                        b.y0 = b.y0.min(p[1]);
                        b.x1 = b.x1.max(p[0]);
                        b.y1 = b.y1.max(p[1]);
                    }
                    // a stroke is a hairline in one axis: give it its width.
                    Some(b.grown(width * 0.5))
                }
                Prim::Text { s, x, y, size, .. } => Some(Bounds {
                    x0: *x,
                    // the host font is roughly 0.6 em wide per character
                    x1: x + 0.6 * size * s.chars().count() as f32,
                    y0: y - size,
                    y1: *y,
                }),
            };
            // What the box stands for: a notehead is the oval inside it, and
            // everything else fills what was measured around it.
            let shape = match prim {
                Prim::Glyph { cp, .. } if is_notehead(*cp) => HitShape::Ellipse,
                _ => HitShape::Rect,
            };
            if let Some(bounds) = bounds {
                self.hits.push(HitBox {
                    id: id.to_string(),
                    bounds,
                    shape,
                });
            }
        }
        self.index_staves();
    }

    /// Cluster the staff lines into staves. A staff line is the one primitive
    /// every system draws the same way — a horizontal stroke running most of
    /// the page — and within a staff they sit exactly one space (two diatonic
    /// steps) apart, so a wider gap starts the next system.
    fn index_staves(&mut self) {
        let mut lines: Vec<(f32, f32)> = self
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, width, .. }
                    if pts.len() == 2
                        && (pts[0][1] - pts[1][1]).abs() < 1.0
                        && (pts[0][0] - pts[1][0]).abs() > 0.3 * self.vb_w =>
                {
                    Some((pts[0][1], *width))
                }
                _ => None,
            })
            .collect();
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        lines.dedup_by(|a, b| (a.0 - b.0).abs() < 0.5);
        self.staves.clear();
        let gap = 2.5 * self.step;
        let mut group: Option<Staff> = None;
        for (y, width) in lines {
            match &mut group {
                Some(s) if y - s.y1 <= gap => s.y1 = y,
                other => {
                    if let Some(s) = other.take() {
                        self.staves.push(s);
                    }
                    *other = Some(Staff {
                        y0: y,
                        y1: y,
                        width,
                    });
                }
            }
        }
        self.staves.extend(group);
    }

    /// The staff a page-y belongs to: the nearest one, since a note off the
    /// staff still belongs to it (that is what ledger lines are for).
    pub fn staff_at(&self, y: f32) -> Option<Staff> {
        self.staves
            .iter()
            .copied()
            .min_by(|a, b| staff_distance(a, y).total_cmp(&staff_distance(b, y)))
    }

    /// The MEI `xml:id` of the element under the screen point `(x, y)`, with the
    /// page fitted into `rect` — the smallest box containing it, so a notehead
    /// wins over the staff line it sits on. `None` when the click lands on blank
    /// paper.
    pub fn hit(&self, rect: Rect, x: f32, y: f32) -> Option<&str> {
        let inv = self.fit(rect).invert()?;
        let [px, py] = inv.apply(x, y);
        self.hits
            .iter()
            .filter(|h| h.bounds.holds(h.shape, px, py))
            .min_by(|a, b| {
                a.bounds
                    .area()
                    .partial_cmp(&b.bounds.area())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|h| h.id.as_str())
    }

    /// The musical time (ms) the cursor sits at this frame: the engine clock
    /// mapped through the `playhead_at` origin while a pass is playing, else the
    /// static `playhead`. `sample_clock` is the engine's sample count and
    /// `host_rate` the server's sample rate, used when the widget names none.
    pub fn head_ms(&self, sample_clock: f64, host_rate: f64) -> f32 {
        let rate = if self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            host_rate
        };
        if self.playhead_at >= 0.0 && sample_clock > 0.0 && rate > 0.0 {
            let swept = (((sample_clock - self.playhead_at) / rate) * 1000.0) as f32;
            if self.playhead_loop_len > 0.0 {
                // The same wrap the timeline views' chrome does, in ms: a
                // repeated passage keeps the cursor inside it. `rem_euclid`
                // so a loop starting past the anchor never parks the cursor
                // left of the region during the first pass.
                let start = self.playhead_loop_start.max(0.0);
                start + (swept - start).rem_euclid(self.playhead_loop_len)
            } else {
                swept
            }
        } else {
            self.playhead
        }
    }

    /// The transform fitting the whole page into `rect`, preserving aspect and
    /// centring (uniform scale, no navigation yet). Returns identity for an
    /// empty page so a def with no geometry is a harmless no-op.
    pub fn fit(&self, rect: Rect) -> Affine {
        if self.vb_w <= 0.0 || self.vb_h <= 0.0 {
            return Affine::IDENTITY;
        }
        let s = (rect.w / self.vb_w)
            .min(rect.h / self.vb_h)
            .max(f32::MIN_POSITIVE);
        Affine {
            tx: rect.x + (rect.w - s * self.vb_w) * 0.5,
            ty: rect.y + (rect.h - s * self.vb_h) * 0.5,
            sx: s,
            sy: s,
        }
    }

    /// How many **diatonic steps** a vertical drag of `dy` screen pixels
    /// amounts to, with the page fitted into `rect`: dragging up (a negative
    /// `dy`) is positive, and the result is whole steps — a pitch has no
    /// in-between position, so the gesture quantizes rather than the client.
    pub fn steps_for(&self, rect: Rect, dy: f32) -> i32 {
        let px = self.step * self.fit(rect).sy;
        if px <= 0.0 {
            return 0;
        }
        (-dy / px).round() as i32
    }
}
