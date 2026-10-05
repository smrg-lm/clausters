//! **Where the staves are, and what time it is**: the staff index and the
//! questions asked of it.
//!
//! An engraved page is a flat list of primitives, and a pitch needs it as
//! something else: the **staff index** (`index_staves`), which is what a ledger
//! line, a diatonic step and a press on blank paper are measured against. It is
//! built with the hit index (`super::hit`), once, when the display list
//! arrives, because the geometry never moves afterwards.
//!
//! Beside them, the two mappings a gesture needs: [`ScoreData::fit`], the
//! transform placing the page in its rectangle (the one every screen
//! coordinate goes through, in both directions), and [`ScoreData::head_ms`],
//! which reads the transport's position as a musical time on the client's
//! timemap.

use super::tess::staff_distance;
use super::{Affine, Entry, Prim, ScoreData, Staff};
use crate::host::layout::Rect;

impl ScoreData {
    /// Cluster the staff lines into staves. A staff line is the one primitive
    /// every system draws the same way -- one of the page's **longest**
    /// horizontal strokes -- and within a staff they sit exactly one space (two
    /// diatonic steps) apart, so a wider gap starts the next system.
    ///
    /// Long relative to the other horizontal strokes rather than to the page:
    /// a short phrase engraved onto a wide sheet draws its system across a
    /// fraction of the viewBox (a two-note page comes out 21000 units wide with
    /// a 2838-unit system), and measuring against the viewBox finds no staff at
    /// all. What this must tell a staff line apart from is a ledger line (a
    /// notehead wide) and a beam (a few noteheads), both far shorter. The rule
    /// is `clausters_core::notation::DisplayList::staves`' and is kept
    /// identical to it: a pitch position measured here and resolved there has
    /// to mean the same thing.
    pub(super) fn index_staves(&mut self) {
        let horizontals: Vec<(f32, f32, f32, Option<&str>)> = self
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Line { pts, width, id, .. }
                    if pts.len() == 2 && (pts[0][1] - pts[1][1]).abs() < 1.0 =>
                {
                    Some((
                        (pts[0][0] - pts[1][0]).abs(),
                        pts[0][1],
                        *width,
                        id.as_deref(),
                    ))
                }
                _ => None,
            })
            .collect();
        let longest = horizontals.iter().fold(0.0f32, |m, (len, ..)| m.max(*len));
        // **The ids of the lines that survived**, which is the staff's own
        // drawing and nothing else on the page: the same filter, so the set and
        // the staves cannot disagree about what a staff line is.
        self.staff_ids = horizontals
            .iter()
            .filter(|(len, ..)| *len >= 0.5 * longest && longest > 0.0)
            .filter_map(|(_, _, _, id)| id.map(str::to_string))
            .collect();
        let mut lines: Vec<(f32, f32)> = horizontals
            .into_iter()
            .filter(|(len, ..)| *len >= 0.5 * longest && longest > 0.0)
            .map(|(_, y, width, _)| (y, width))
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

    /// Which staff **of the score** a drawn staff is: its rank inside its own
    /// system, not its place down the page.
    ///
    /// The two differ the moment a score wraps: the third system's upper staff
    /// is the fifth drawn on a two-staff page, and naming it staff 4 names a
    /// staff no model has. Systems come from the client (`systems`), because
    /// telling a grand staff from two systems is a notation fact -- a barline
    /// through the brace -- and not something a measurement settles. With none
    /// sent, a one-system page is the only case that can be right, and the rank
    /// down the page is that.
    fn staff_of_system(&self, staff: Staff) -> Option<usize> {
        let down_the_page = self.staves.iter().position(|s| *s == staff)?;
        let mid = 0.5 * (staff.y0 + staff.y1);
        let Some(system) = self
            .systems
            .iter()
            .find(|[y0, y1]| mid >= *y0 - self.step && mid <= *y1 + self.step)
        else {
            return Some(down_the_page);
        };
        Some(
            self.staves[..down_the_page]
                .iter()
                .filter(|s| {
                    let m = 0.5 * (s.y0 + s.y1);
                    m >= system[0] - self.step && m <= system[1] + self.step
                })
                .count(),
        )
    }

    /// The staff a page-y belongs to: the nearest one, since a note off the
    /// staff still belongs to it (that is what ledger lines are for).
    pub fn staff_at(&self, y: f32) -> Option<Staff> {
        self.staves
            .iter()
            .copied()
            .min_by(|a, b| staff_distance(a, y).total_cmp(&staff_distance(b, y)))
    }

    /// Where an engraved element sits on its staff, in **whole diatonic steps
    /// from the staff's top line**, positive upward -- the absolute coordinate a
    /// pitch edit names instead of a displacement.
    ///
    /// It is the reading `ledger_ys` already performs, given a name: both
    /// measure the same page-y against the same staff in the same `step` units,
    /// which is what makes the position something the host may report without
    /// owning any notation. The client can derive the identical number from the
    /// display list it engraved and sent, so the two sides cannot disagree
    /// about it.
    ///
    /// The element's **first** primitive is its notehead (verovio draws it
    /// before the stem), and the anchor is that glyph's **placement origin**
    /// rather than the middle of its bounds: a SMuFL notehead is drawn about
    /// its origin, so the origin *is* the pitch, while the bounds' middle is
    /// only the same number for a vertically symmetric outline. Ledger lines
    /// can use the bounds -- they are centred on the ink -- but a pitch cannot,
    /// and `clausters_core::notation::DisplayList::staff_position` reads the
    /// origin, which this must agree with byte for byte.
    pub fn staff_position(&self, id: &str) -> Option<i32> {
        let y = self
            .prims
            .iter()
            .find(|p| p.id() == Some(id))
            .and_then(|p| {
                Some(match p {
                    Prim::Glyph { xf, .. } | Prim::Fill { xf, .. } => xf.ty,
                    Prim::Line { pts, .. } => {
                        let (lo, hi) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                            (lo.min(p[1]), hi.max(p[1]))
                        });
                        if lo > hi {
                            return None;
                        }
                        0.5 * (lo + hi)
                    }
                    Prim::Text { y, .. } => *y,
                })
            })?;
        let staff = self.staff_at(y)?;
        Some((((staff.y0 - y) / self.step).round()) as i32)
    }

    /// Where a press on **blank paper** landed, for a page that takes note
    /// entry: which staff, how far up it, and the element the new note would
    /// follow.
    ///
    /// This is the gesture's whole contribution, and the division is the same
    /// one every other score gesture keeps: the host measures the *page*, which
    /// is the only thing it has, and names what it found by the ids the client
    /// engraved. It decides no duration, no pitch and no spelling -- a staff
    /// position is not a pitch until something knows the clef and the key, and
    /// the host knows neither.
    ///
    /// `after` is the last element to the **left** on that staff, so a client
    /// inserts after it; `None` means the press was before everything on the
    /// staff, which is where a score with nothing written yet begins.
    pub fn entry_at(&self, rect: Rect, x: f32, y: f32) -> Option<Entry> {
        let inv = self.fit(rect).invert()?;
        let [px, py] = inv.apply(x, y);
        let staff = self.staff_at(py)?;
        let index = self.staff_of_system(staff)?;
        // The elements of this staff, which is the nearest one to each: a hit
        // is placed by where it is drawn, exactly as a note off the staff still
        // belongs to the staff its ledger lines count from.
        let after = self
            .hits
            .iter()
            .filter(|h| self.elements.contains(&h.id))
            .filter(|h| {
                let mid = 0.5 * (h.bounds.y0 + h.bounds.y1);
                self.staff_at(mid) == Some(staff)
            })
            .filter(|h| h.bounds.x1 <= px)
            .max_by(|a, b| a.bounds.x1.total_cmp(&b.bounds.x1))
            .map(|h| h.id.clone());
        Some(Entry {
            staff: index,
            position: (((staff.y0 - py) / self.step).round()) as i32,
            after,
        })
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
    /// `dy`) is positive, and the result is whole steps -- a pitch has no
    /// in-between position, so the gesture quantizes rather than the client.
    pub fn steps_for(&self, rect: Rect, dy: f32) -> i32 {
        let px = self.step * self.fit(rect).sy;
        if px <= 0.0 {
            return 0;
        }
        (-dy / px).round() as i32
    }
}
