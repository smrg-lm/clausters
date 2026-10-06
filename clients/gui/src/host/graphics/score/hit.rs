//! **What is under a point**: the hit index, and the shape each entry is
//! tested as.
//!
//! An engraved page is a flat list of primitives, and a press has to name the
//! element under it. Each identified primitive becomes one [`HitBox`]: its
//! page-unit extent, which is the quick test, and the **shape it is drawn as**,
//! which is the test that decides -- a glyph or a fill by its own outline, a
//! stroke by its distance from the line, a notehead by the oval it is, so a
//! press lands on what is drawn and not on the paper around it. A
//! [`HitGrid`] in front of them keeps a press from testing every primitive of
//! a full page: a page does not move between engravings, so it is built once,
//! when the display list arrives, and a query reads the few cells around the
//! point.
//!
//! **A press is answered in two passes.** First by what is drawn exactly under
//! the point; only when nothing is, by what lies within the hit slop of it --
//! so the slop that makes a hairline slur pressable never lets a note steal a
//! press that landed on the slur itself. The staff's own lines take no slop:
//! between them is the staff's paper, which is where a note is written, not the
//! line's.

use std::collections::HashMap;
use std::sync::Arc;

use lyon::algorithms::aabb::fast_bounding_box;
use lyon::algorithms::hit_test::hit_test_path;
use lyon::math::point;
use lyon::path::iterator::PathIterator;
use lyon::path::{FillRule, Path as LyonPath, PathEvent};

use super::glyphs::build_path;
use super::{Affine, Bounds, Prim, ScoreData};
use crate::host::layout::Rect;

/// **What an entry's extent stands for**, which is the shape a press is tested
/// against once the extent has let it through.
#[derive(Clone, Debug)]
pub enum HitShape {
    /// The box itself: a run of text, whose glyphs are the renderer's.
    Rect,
    /// The ellipse inscribed in the box: a notehead. Its outline would do for
    /// a filled head, but a half note's is a ring, and the hole is the note too.
    Ellipse,
    /// An outline the engraver drew: a glyph or a fill, as its path in its own
    /// units and the map from those onto the page.
    Outline { path: Arc<LyonPath>, xf: Affine },
    /// A stroke: the polyline in page units and half its width.
    Stroke { pts: Vec<[f32; 2]>, half: f32 },
}

/// One entry of the hit index: the page-unit extent of an identified primitive,
/// the shape it is tested as, and the MEI `xml:id` it was engraved from.
#[derive(Clone, Debug)]
pub struct HitBox {
    pub id: String,
    pub bounds: Bounds,
    pub shape: HitShape,
}

impl HitBox {
    /// Whether the page point `(x, y)` is on what this entry draws, or within
    /// `reach` page units of it.
    fn holds(&self, x: f32, y: f32, reach: f32) -> bool {
        if !self.bounds.grown(reach).contains(x, y) {
            return false;
        }
        match &self.shape {
            HitShape::Rect => true,
            HitShape::Ellipse => {
                let b = self.bounds.grown(reach);
                crate::host::graphics::shape::in_ellipse(
                    x as f64,
                    y as f64,
                    Rect::new(b.x0, b.y0, b.x1 - b.x0, b.y1 - b.y0),
                )
            }
            HitShape::Outline { path, xf } => {
                let Some(inv) = xf.invert() else {
                    return false;
                };
                let [lx, ly] = inv.apply(x, y);
                // the outline's own units per page unit, for the tolerance
                // and the reach: verovio scales a glyph uniformly, and the
                // smaller axis is the conservative one when it does not.
                let scale = xf.sx.abs().min(xf.sy.abs()).max(f32::MIN_POSITIVE);
                let tolerance = FLATTEN / scale;
                hit_test_path(&point(lx, ly), path.iter(), FillRule::NonZero, tolerance)
                    || (reach > 0.0 && outline_distance(path, lx, ly, tolerance) * scale <= reach)
            }
            HitShape::Stroke { pts, half } => {
                polyline_distance(pts.iter().copied(), x, y) <= half + reach
            }
        }
    }
}

/// How finely a curve is flattened to be measured, in page units: a few
/// hundredths of a staff space, far under anything a hand can aim at.
const FLATTEN: f32 = 1.0;

/// The distance from `(x, y)` to the nearest edge of `path`, in its own units.
fn outline_distance(path: &LyonPath, x: f32, y: f32, tolerance: f32) -> f32 {
    let mut best = f32::MAX;
    for event in path.iter().flattened(tolerance) {
        let (a, b) = match event {
            PathEvent::Line { from, to } => (from, to),
            PathEvent::End {
                last,
                first,
                close: true,
            } => (last, first),
            _ => continue,
        };
        best = best.min(segment_distance([a.x, a.y], [b.x, b.y], x, y));
    }
    best
}

/// The distance from `(x, y)` to a polyline.
fn polyline_distance(pts: impl Iterator<Item = [f32; 2]>, x: f32, y: f32) -> f32 {
    let mut best = f32::MAX;
    let mut prev: Option<[f32; 2]> = None;
    for p in pts {
        best = best.min(match prev {
            Some(a) => segment_distance(a, p, x, y),
            None => (p[0] - x).hypot(p[1] - y),
        });
        prev = Some(p);
    }
    best
}

/// The distance from `(x, y)` to the segment `a`-`b`.
fn segment_distance(a: [f32; 2], b: [f32; 2], x: f32, y: f32) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((x - a[0]) * dx + (y - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a[0] + t * dx - x).hypot(a[1] + t * dy - y)
}

/// **The spatial index in front of the hit boxes**: a uniform grid over the
/// page, each cell listing the entries whose extent crosses it.
///
/// A page is static between engravings and every query is a point, so the
/// structure only has to narrow a press to its neighbourhood, and a grid does
/// that with nothing to balance. Its cell is a few staff spaces: a notehead
/// sits in one or two, and a staff line, the longest thing on the page, in a
/// row of them.
#[derive(Clone, Debug, Default)]
pub struct HitGrid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl HitGrid {
    fn build(hits: &[HitBox], cell: f32) -> HitGrid {
        let mut grid = HitGrid {
            cell: cell.max(1.0),
            cells: HashMap::new(),
        };
        for (i, h) in hits.iter().enumerate() {
            let (c0, r0) = grid.at(h.bounds.x0, h.bounds.y0);
            let (c1, r1) = grid.at(h.bounds.x1, h.bounds.y1);
            for c in c0..=c1 {
                for r in r0..=r1 {
                    grid.cells.entry((c, r)).or_default().push(i as u32);
                }
            }
        }
        grid
    }

    fn at(&self, x: f32, y: f32) -> (i32, i32) {
        (
            (x / self.cell).floor() as i32,
            (y / self.cell).floor() as i32,
        )
    }

    /// The entries whose extent may lie within `reach` of `(x, y)`, each once,
    /// in index order.
    fn near(&self, x: f32, y: f32, reach: f32) -> Vec<usize> {
        let (c0, r0) = self.at(x - reach, y - reach);
        let (c1, r1) = self.at(x + reach, y + reach);
        let mut out: Vec<usize> = (c0..=c1)
            .flat_map(|c| (r0..=r1).map(move |r| (c, r)))
            .filter_map(|key| self.cells.get(&key))
            .flatten()
            .map(|&i| i as usize)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The tightest of a set of overlapping boxes.
fn smallest<'a>(boxes: impl Iterator<Item = &'a HitBox>) -> Option<&'a HitBox> {
    boxes.min_by(|a, b| a.bounds.area().total_cmp(&b.bounds.area()))
}

impl ScoreData {
    /// Rebuild the hit index from the placed primitives: one entry per
    /// identified primitive, so a press can name the element under it, and the
    /// grid in front of them. Done once when the display list arrives -- the
    /// geometry never moves afterwards, and re-deriving it per press would mean
    /// re-parsing every glyph outline on every press.
    pub fn index(&mut self) {
        // glyph outlines repeat all over a page (one notehead shape, hundreds of
        // notes), so each codepoint's outline is built and measured once.
        let mut outlines: HashMap<u32, Option<(Arc<LyonPath>, Bounds)>> = HashMap::new();
        self.hits.clear();
        for prim in &self.prims {
            let Some(id) = prim.id() else { continue };
            let entry = match prim {
                Prim::Glyph { cp, xf, .. } => outlines
                    .entry(*cp)
                    .or_insert_with(|| measured(self.glyphs.get(cp)?))
                    .as_ref()
                    .map(|(path, b)| {
                        let shape = if is_notehead(*cp) {
                            HitShape::Ellipse
                        } else {
                            HitShape::Outline {
                                path: path.clone(),
                                xf: *xf,
                            }
                        };
                        (b.transformed(*xf), shape)
                    }),
                Prim::Fill { d, xf, .. } => measured(d)
                    .map(|(path, b)| (b.transformed(*xf), HitShape::Outline { path, xf: *xf })),
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
                    (b.x0 <= b.x1).then(|| {
                        (
                            b.grown(width * 0.5),
                            HitShape::Stroke {
                                pts: pts.clone(),
                                half: width * 0.5,
                            },
                        )
                    })
                }
                Prim::Text {
                    s,
                    x,
                    y,
                    size,
                    anchor,
                    ..
                } => {
                    // As wide as the host draws it -- the width of the string
                    // in the host's own face, at the scale whose body box is
                    // this text's capitals -- and as high as a line of them.
                    let w = crate::host::font::width(s, 1.0) * size * super::CAP_PER_EM
                        / crate::host::font::GLYPH_H as f32;
                    let x0 = anchor.left(*x, w);
                    Some((
                        Bounds {
                            x0,
                            x1: x0 + w,
                            y0: y - size * super::CAP_PER_EM,
                            y1: *y,
                        },
                        HitShape::Rect,
                    ))
                }
            };
            if let Some((bounds, shape)) = entry {
                self.hits.push(HitBox {
                    id: id.to_string(),
                    bounds,
                    shape,
                });
            }
        }
        self.grid = HitGrid::build(&self.hits, GRID_STEPS * self.step);
        self.by_id.clear();
        for (i, h) in self.hits.iter().enumerate() {
            self.by_id.entry(h.id.clone()).or_default().push(i as u32);
        }
        self.index_staves();
        self.rows = vec![Vec::new(); self.staves.len()];
        for (i, h) in self.hits.iter().enumerate() {
            let (x, y) = h.bounds.middle();
            if let Some(staff) = self.staff_index_at(y) {
                self.rows[staff].push((x, i as u32));
            }
        }
        for row in &mut self.rows {
            row.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        }
    }

    /// The entries of `hits` drawn under `id`, in their order.
    pub fn boxes_of<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a HitBox> + 'a {
        self.by_id
            .get(id)
            .into_iter()
            .flatten()
            .map(|&i| &self.hits[i as usize])
    }

    /// The MEI `xml:id` of the element under the screen point `(x, y)`, with the
    /// page fitted into `rect` and `slop` screen pixels of reach for a press
    /// that lands beside a thin mark. `None` when the press lands on blank
    /// paper.
    ///
    /// **What is drawn under the point answers first**, and only when nothing
    /// is does what lies within the slop (see the module's docs). In either
    /// pass **a sounding element wins over anything drawn across it**, and only
    /// then does the smallest box decide: area is a bad proxy for "innermost"
    /// on an engraved page, since a staff line is a hairline the width of the
    /// system whose box is *thinner* than a notehead's, and by area alone it
    /// would take every note written on a line rather than in a space. The page
    /// says which ids are notes and rests ([`ScoreData::elements`]), so the
    /// question is answered by what a thing *is*; the same order keeps a note
    /// under a beam, a slur or a hairpin reachable.
    pub fn hit(&self, rect: Rect, x: f32, y: f32, slop: f32) -> Option<&str> {
        let fit = self.fit(rect);
        let [px, py] = fit.invert()?.apply(x, y);
        let reach = slop.max(0.0) / fit.sx.abs().max(f32::MIN_POSITIVE);
        let near = self.grid.near(px, py, reach);
        let pick = |reach: f32| {
            let under: Vec<&HitBox> = near
                .iter()
                .map(|&i| &self.hits[i])
                .filter(|h| {
                    let reach = if self.staff_ids.contains(&h.id) {
                        0.0
                    } else {
                        reach
                    };
                    h.holds(px, py, reach)
                })
                .collect();
            smallest(
                under
                    .iter()
                    .copied()
                    .filter(|h| self.elements.contains(&h.id)),
            )
            .or_else(|| smallest(under.iter().copied()))
            .map(|h| h.id.as_str())
        };
        pick(0.0).or_else(|| if reach > 0.0 { pick(reach) } else { None })
    }
}

/// The grid's cell, in diatonic steps: two staff spaces.
const GRID_STEPS: f32 = 4.0;

/// An outline built from its path data, with its extent in its own units.
fn measured(d: &str) -> Option<(Arc<LyonPath>, Bounds)> {
    let path = build_path(d)?;
    let b = fast_bounding_box(&path);
    Some((
        Arc::new(path),
        Bounds {
            x0: b.min.x,
            y0: b.min.y,
            x1: b.max.x,
            y1: b.max.y,
        },
    ))
}

/// The SMuFL **Noteheads** range (U+E0A0-U+E0FF): the glyphs whose shape is an
/// oval and whose box therefore over-answers for them. Whether a codepoint is a
/// notehead is a fact about the font's layout, not about this page, so it is
/// read straight off the range rather than configured.
fn is_notehead(cp: u32) -> bool {
    (0xE0A0..=0xE0FF).contains(&cp)
}
