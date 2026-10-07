//! **A page drawn on a time axis**: proportional notation.
//!
//! An engraving spaces its notes by what has to fit between them, and a box
//! on a multitrack stands on an axis where a distance is a time. So a page
//! drawn in a box is not fitted to it: every note stands **at its time**,
//! in line with whatever is on the other tracks, and the page is kept for
//! everything else -- which staff a note is on, how it is spelled, what is
//! written on it.
//!
//! # How a note stands at its time without being stretched
//!
//! The page is drawn through a [`Warp`] of its `x`. Around each note is a
//! **zone** the engraving is drawn in as it is -- the head, its stem, its
//! accidental, its dot, its ledger lines -- moved whole so the head starts at
//! the note's time. Between two zones the warp is the straight stretch that
//! joins them, and what joins notes follows it: a beam's and a slur's corners
//! are where their notes are, and the staff's lines run through. A **glyph is
//! never stretched**: it is placed where its own place falls, and drawn at
//! the size the row gives the staff.
//!
//! # What has no time takes no room
//!
//! The first clef and the key signature stand **before the first note**, in
//! the room before it, and stay in view: held at the left edge of what is
//! visible once the box's start has scrolled off, as a roll's keyboard is.
//! A meter is written **above the staff**, at the note it is the meter from,
//! so neither it nor a change of it moves a notehead.

use super::tess::xf_shrink;
use super::{Affine, FillOf, Prim, ScoreData};
use crate::host::layout::Rect;
use crate::host::paint::{Color, Mesh};

/// How far a note's zone reaches to the left of its head, in staff spaces:
/// an accidental and the air before it.
const REACH_LEFT: f32 = 2.2;
/// ...and to the right: the head, its dot, the flag of its stem.
const REACH_RIGHT: f32 = 2.4;
/// The staff spaces of air kept above and below what the page draws.
const AIR: f32 = 1.0;
/// The room a staff is given above and below its lines, in staff spaces,
/// whatever it holds: where the meter and the first ledger lines go.
const ROOM: f32 = 3.0;
/// The size a meter is written at over its staff, to the size it is engraved
/// at in one: what fits the room over a staff.
const METER: f32 = 0.5;
/// How far over the staff's top line a meter's foot stands, in staff spaces.
const METER_GAP: f32 = 0.4;
/// How long a line with a staff's id has to be, in staff spaces, to be one of
/// the staff's own lines rather than a note's ledger line.
const SYSTEM_LINE: f32 = 8.0;
/// The smallest a staff space is drawn, in pixels: under it a staff is five
/// lines nobody can count, and the box is drawn another way.
const LEGIBLE: f32 = 2.5;

/// **The page's `x` as the screen's**: straight through each note's zone, at
/// the scale the staff is drawn at, and stretched between two of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Warp {
    /// `(page x, screen x)`, ascending in both.
    points: Vec<(f32, f32)>,
    /// Pixels to a page unit, inside a zone and past the last of them.
    scale: f32,
}

impl Warp {
    /// A warp that puts each of `anchors` -- `(page x of a note's head, the
    /// screen x of its time)` -- where it sounds, a zone of `reach` page
    /// units (left, right) around each drawn at `scale`.
    ///
    /// Where two notes are nearer on the screen than their zones are wide,
    /// the zones give way in proportion: the warp stays in order, and what
    /// is drawn through it is squeezed rather than folded over.
    #[must_use]
    pub fn new(anchors: &[(f32, f32)], scale: f32, reach: (f32, f32)) -> Warp {
        let mut anchors: Vec<(f32, f32)> = anchors.to_vec();
        anchors.sort_by(|a, b| a.0.total_cmp(&b.0));
        // notes of one column are one anchor
        anchors.dedup_by(|b, a| (b.0 - a.0).abs() < 1.0);
        let (left, right) = reach;
        let whole = (left + right).max(f32::MIN_POSITIVE);
        let count = anchors.len();
        let mut points = Vec::with_capacity(count * 3);
        for (i, &(page, screen)) in anchors.iter().enumerate() {
            // each side reaches its share of the gap to its neighbour, and
            // gives way where the screen has less room than the page
            let side = |other: Option<&(f32, f32)>, reach: f32| -> (f32, f32) {
                let Some(&(other_page, other_screen)) = other else {
                    return (reach, 1.0);
                };
                let gap = (other_page - page).abs();
                let reach = reach.min(gap * reach / whole);
                let need = scale * gap.min(whole);
                let room = (other_screen - screen).abs();
                let give = if need > 0.0 {
                    (room / need).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                (reach, give)
            };
            let before = i.checked_sub(1).and_then(|i| anchors.get(i));
            let (l, give_l) = side(before, left);
            let (r, give_r) = side(anchors.get(i + 1), right);
            points.push((page - l, screen - give_l * scale * l));
            points.push((page, screen));
            points.push((page + r, screen + give_r * scale * r));
        }
        Warp { points, scale }
    }

    /// Where the page's `x` is drawn.
    #[must_use]
    pub fn x(&self, page: f32) -> f32 {
        let (Some(first), Some(last)) = (self.points.first(), self.points.last()) else {
            return page * self.scale;
        };
        if page <= first.0 {
            return first.1 - self.scale * (first.0 - page);
        }
        if page >= last.0 {
            return last.1 + self.scale * (page - last.0);
        }
        let at = self.points.partition_point(|point| point.0 <= page);
        let (a, b) = (self.points[at - 1], self.points[at]);
        let span = (b.0 - a.0).max(f32::MIN_POSITIVE);
        a.1 + (b.1 - a.1) * (page - a.0) / span
    }

    /// Where the first zone starts on the page: what is left of it has no
    /// time.
    #[must_use]
    pub fn start(&self) -> f32 {
        self.points.first().map_or(f32::INFINITY, |point| point.0)
    }
}

/// What a page on a time axis is painted in.
#[derive(Clone, Copy, Debug)]
pub struct TimeColors {
    /// The engraving.
    pub ink: Color,
    /// What is put behind the clef where it is held over the notes: the
    /// box's own fill.
    pub backdrop: Color,
}

/// Where a page on a time axis is drawn.
#[derive(Clone, Copy, Debug)]
pub struct TimeFrame {
    /// The box: the notes are cut to it.
    pub body: Rect,
    /// The row the box is on: the clef before the box is cut to this.
    pub row: Rect,
}

impl ScoreData {
    /// The page's `y` span a box shows: its staves with the room a staff is
    /// given, grown to what the notes named reach.
    fn span_y(&self, heads: &[super::Bounds]) -> Option<(f32, f32)> {
        let space = 2.0 * self.step;
        let top = self
            .staves
            .iter()
            .map(|s| s.y0)
            .fold(f32::INFINITY, f32::min);
        let bottom = self
            .staves
            .iter()
            .map(|s| s.y1)
            .fold(f32::NEG_INFINITY, f32::max);
        if !top.is_finite() || !bottom.is_finite() {
            return None;
        }
        let (mut y0, mut y1) = (top - ROOM * space, bottom + ROOM * space);
        for head in heads {
            y0 = y0.min(head.y0 - AIR * space);
            y1 = y1.max(head.y1 + AIR * space);
        }
        Some((y0, y1))
    }

    /// **The page on a time axis** (see the module): `anchors` name the
    /// elements that have a time -- `(id, time)` -- and `x_of` is where a
    /// time stands on the screen. Answers `false`, having drawn nothing,
    /// where the row is too low for a staff to be read: the caller draws the
    /// box another way.
    pub fn render_on_time(
        &self,
        mesh: &mut Mesh,
        frame: TimeFrame,
        anchors: &[(String, f64)],
        x_of: &dyn Fn(f64) -> f32,
        colors: TimeColors,
    ) -> bool {
        let TimeFrame { body, row } = frame;
        if body.h <= 0.0 || self.step <= 0.0 {
            return false;
        }
        // each named element's head, and where its time is
        let placed: Vec<(super::Bounds, f32)> = anchors
            .iter()
            .filter_map(|(id, time)| {
                let head = self.boxes_of(id).next()?.bounds;
                Some((head, x_of(*time)))
            })
            .collect();
        let heads: Vec<super::Bounds> = placed.iter().map(|(head, _)| *head).collect();
        let Some((y0, y1)) = self.span_y(&heads) else {
            return false;
        };
        let space = 2.0 * self.step;
        let scale = body.h / (y1 - y0).max(f32::MIN_POSITIVE);
        if scale * space < LEGIBLE {
            return false;
        }
        let columns: Vec<(f32, f32)> = placed.iter().map(|(head, x)| (head.x0, *x)).collect();
        let warp = Warp::new(&columns, scale, (REACH_LEFT * space, REACH_RIGHT * space));
        let y = |page: f32| body.y + (page - y0) * scale;
        let tol_page = 0.33 / scale;
        let outer = mesh.clip();
        let cut = |rect: Rect| -> Rect {
            let Some(clip) = outer else {
                return rect;
            };
            let x0 = rect.x.max(clip.x);
            let y0 = rect.y.max(clip.y);
            let x1 = (rect.x + rect.w).min(clip.x + clip.w);
            let y1 = (rect.y + rect.h).min(clip.y + clip.h);
            Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
        };

        // What has no time: the meters, written over their staves, and what
        // stands before the first note.
        let is_meter = |prim: &Prim| {
            prim.id()
                .and_then(|id| self.kinds.get(id))
                .is_some_and(|kind| kind == "meterSig")
        };
        // Where a primitive stands across the page, `(left, right)`. A fill's
        // outline is in its own coordinates and its transform places it, so
        // its extent is its outline's, placed -- the transform's offset alone
        // says where the outline's origin is, which for a dot of a rest is
        // nowhere near the dot.
        let across = |at: usize, prim: &Prim| -> (f32, f32) {
            match prim {
                Prim::Glyph { xf, .. } => (xf.tx, xf.tx),
                Prim::Text { x, .. } => (*x, *x),
                Prim::Line { pts, .. } => pts
                    .iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                        (lo.min(p[0]), hi.max(p[0]))
                    }),
                Prim::Fill { d, xf, .. } => {
                    let fill = self
                        .fills
                        .of(FillOf::Prim(at), d, tol_page * xf_shrink(*xf));
                    let span =
                        fill.iter()
                            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), c| {
                                let x = xf.apply(c[0], c[1])[0];
                                (lo.min(x), hi.max(x))
                            });
                    if span.0.is_finite() {
                        span
                    } else {
                        (xf.tx, xf.tx)
                    }
                }
            }
        };
        let x_at = |at: usize, prim: &Prim| across(at, prim).0;
        let first_meter = self
            .prims
            .iter()
            .enumerate()
            .filter(|(_, prim)| is_meter(prim))
            .map(|(at, prim)| x_at(at, prim))
            .fold(f32::INFINITY, f32::min);
        // the page before this edge is the clef and the key: it has no time
        let edge = warp.start().min(first_meter);
        let in_prefix = |at: usize, prim: &Prim| !is_meter(prim) && across(at, prim).1 < edge;

        // one primitive, its `x` through `place` and its glyphs unstretched
        let draw = |mesh: &mut Mesh,
                    at: usize,
                    prim: &Prim,
                    place: &dyn Fn(f32) -> f32,
                    lift: f32| {
            let color = colors.ink;
            match prim {
                Prim::Line { pts, width, .. } => {
                    let w = (width * scale).max(1.0);
                    // **A ledger line is its notehead's**, drawn as long as
                    // engraved and where its head is, never stretched with
                    // the stretch between two notes nor squeezed with it.
                    if let Some((mid, half, line)) = ledger_line(self, prim, space) {
                        let at = place(mid);
                        mesh.line(
                            [at - half * scale, y(line - lift)],
                            [at + half * scale, y(line - lift)],
                            w,
                            color,
                        );
                        return;
                    }
                    for seg in pts.windows(2) {
                        mesh.line(
                            [place(seg[0][0]), y(seg[0][1] - lift)],
                            [place(seg[1][0]), y(seg[1][1] - lift)],
                            w,
                            color,
                        );
                    }
                }
                Prim::Glyph { cp, xf, .. } => {
                    let Some(d) = self.glyphs.get(cp) else {
                        return;
                    };
                    let fill = self
                        .fills
                        .of(FillOf::Glyph(*cp), d, tol_page * xf_shrink(*xf));
                    // placed where its own place falls, and not stretched
                    let fit = Affine {
                        sx: scale,
                        sy: scale,
                        tx: place(xf.tx) - scale * xf.tx,
                        ty: y(0.0) - scale * lift,
                    }
                    .then(*xf);
                    for corner in fill.as_chunks::<3>().0 {
                        mesh.tri(
                            fit.apply(corner[0][0], corner[0][1]),
                            fit.apply(corner[1][0], corner[1][1]),
                            fit.apply(corner[2][0], corner[2][1]),
                            color,
                        );
                    }
                }
                Prim::Fill { d, xf, .. } => {
                    let fill = self
                        .fills
                        .of(FillOf::Prim(at), d, tol_page * xf_shrink(*xf));
                    // every corner where the warp puts it: what joins two
                    // notes reaches both
                    let corner = |c: [f32; 2]| {
                        let [px, py] = xf.apply(c[0], c[1]);
                        [place(px), y(py - lift)]
                    };
                    for tri in fill.as_chunks::<3>().0 {
                        mesh.tri(corner(tri[0]), corner(tri[1]), corner(tri[2]), color);
                    }
                }
                Prim::Text {
                    s,
                    x,
                    y: ty,
                    size,
                    anchor,
                    ..
                } => {
                    let cap = (size * scale).abs() * super::CAP_PER_EM;
                    let text_scale = (cap / crate::host::font::GLYPH_H as f32).max(0.5);
                    let left = anchor.left(place(*x), crate::host::font::width(s, text_scale));
                    crate::host::font::text(mesh, s, left, y(*ty - lift) - cap, text_scale, color);
                }
            }
        };

        // The notes and what joins them, cut to the box.
        mesh.set_clip(Some(cut(body)));
        // **The staff runs the whole box**, as a roll's lanes do: a page on a
        // time line has no end, so its lines go on past the last note to
        // wherever the box is pulled, whatever the engraving reached.
        for prim in &self.prims {
            if let Some((line, w)) = staff_line(self, prim, space) {
                mesh.line(
                    [body.x, y(line)],
                    [body.x + body.w, y(line)],
                    (w * scale).max(1.0),
                    colors.ink,
                );
            }
        }
        let through = |page: f32| warp.x(page);
        for (at, prim) in self.prims.iter().enumerate() {
            if is_meter(prim) || in_prefix(at, prim) {
                continue;
            }
            draw(mesh, at, prim, &through, 0.0);
        }
        // A meter, over its staff, starting where the note after it does --
        // at half its size, which is what the room over a staff holds.
        for prim in &self.prims {
            let Prim::Glyph { cp, xf, id } = prim else {
                continue;
            };
            if !is_meter(prim) {
                continue;
            }
            let (Some(d), Some(staff)) = (
                self.glyphs.get(cp),
                self.staves.iter().min_by(|a, b| {
                    super::tess::staff_distance(a, xf.ty)
                        .total_cmp(&super::tess::staff_distance(b, xf.ty))
                }),
            ) else {
                continue;
            };
            // every digit of one meter keeps its place beside the others
            let group = self
                .prims
                .iter()
                .enumerate()
                .filter(|(_, other)| other.id() == id.as_deref())
                .map(|(at, other)| x_at(at, other))
                .fold(f32::INFINITY, f32::min);
            let next = columns
                .iter()
                .map(|(page, _)| *page)
                .filter(|page| *page >= group)
                .fold(f32::INFINITY, f32::min);
            let anchor = if next.is_finite() {
                warp.x(next)
            } else {
                warp.x(group)
            };
            // the staff's own height, halved, its foot just over the top line
            let small = scale * METER;
            let foot = y(staff.y0 - METER_GAP * space);
            let fit = Affine {
                sx: small,
                sy: small,
                tx: anchor - small * group,
                ty: foot - small * staff.y1,
            }
            .then(*xf);
            let fill = self
                .fills
                .of(FillOf::Glyph(*cp), d, tol_page / METER * xf_shrink(*xf));
            for corner in fill.as_chunks::<3>().0 {
                mesh.tri(
                    fit.apply(corner[0][0], corner[0][1]),
                    fit.apply(corner[1][0], corner[1][1]),
                    fit.apply(corner[2][0], corner[2][1]),
                    colors.ink,
                );
            }
        }

        // The clef and the key: before the first note, before the box where
        // the box starts with it, and held at the left of what is visible.
        let prefix: Vec<(usize, &Prim)> = self
            .prims
            .iter()
            .enumerate()
            .filter(|(at, prim)| in_prefix(*at, prim))
            .collect();
        if !prefix.is_empty() && edge.is_finite() {
            let from = prefix
                .iter()
                .map(|(at, p)| x_at(*at, p))
                .fold(f32::INFINITY, f32::min);
            let width = scale * (edge - from);
            let written = warp.x(from);
            let left = written.max(body.x - width).max(row.x);
            let shift = left - written;
            mesh.set_clip(Some(cut(row)));
            // Held over the notes, what it covers is covered -- in the box's
            // own fill, and inside its frame, so the box keeps its edges --
            // and the staff's lines run on behind the clef. Where it stands
            // in its own place, before the first note, it covers nothing.
            if shift > 0.5 {
                let x0 = left.max(body.x);
                let inner = Rect::new(x0, body.y + 1.0, left + width - x0, body.h - 2.0);
                if inner.w > 0.0 && inner.h > 0.0 {
                    mesh.rect(inner, colors.backdrop);
                }
            }
            for prim in &self.prims {
                let Some((line, w)) = staff_line(self, prim, space) else {
                    continue;
                };
                mesh.line(
                    [left, y(line)],
                    [left + width, y(line)],
                    (w * scale).max(1.0),
                    colors.ink,
                );
            }
            let held = |page: f32| warp.x(page) + shift;
            for (at, prim) in prefix {
                draw(mesh, at, prim, &held, 0.0);
            }
        }
        mesh.set_clip(outer);
        true
    }
}

/// A horizontal line with a staff's id: its page `y`, its stroke, and how far
/// it runs across, `(left, right)`.
fn staff_owned(page: &ScoreData, prim: &Prim) -> Option<(f32, f32, (f32, f32))> {
    let Prim::Line { pts, width, id } = prim else {
        return None;
    };
    let owned = id.as_ref().is_some_and(|id| page.staff_ids.contains(id));
    let flat = pts
        .windows(2)
        .all(|seg| (seg[0][1] - seg[1][1]).abs() < 1.0);
    if !owned || pts.len() < 2 || !flat {
        return None;
    }
    let across = pts
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p[0]), hi.max(p[0]))
        });
    Some((pts[0][1], *width, across))
}

/// **One of a staff's own lines**, as `(page y, stroke width)`: a line with
/// the staff's id that runs the length of its system.
fn staff_line(page: &ScoreData, prim: &Prim, space: f32) -> Option<(f32, f32)> {
    let (line, width, (lo, hi)) = staff_owned(page, prim)?;
    (hi - lo >= SYSTEM_LINE * space).then_some((line, width))
}

/// **A ledger line**, as `(page x of its middle, half its length, page y)`: a
/// line with the staff's id -- the engraver files a note's ledger lines under
/// its staff -- as short as a notehead's, not a system's.
fn ledger_line(page: &ScoreData, prim: &Prim, space: f32) -> Option<(f32, f32, f32)> {
    let (line, _, (lo, hi)) = staff_owned(page, prim)?;
    (hi - lo < SYSTEM_LINE * space).then_some(((lo + hi) * 0.5, (hi - lo) * 0.5, line))
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;

    /// One staff -- five lines a staff space apart -- a clef before two
    /// noteheads.
    fn page() -> ScoreData {
        let props: Map<String, Value> = serde_json::from_str(
            r#"{
                "vb": [4000, 1400], "step": 90,
                "glyphs": {"E0A4": "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
                "prims": [
                    {"k": "line", "pts": [[0, 200], [4000, 200]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[0, 380], [4000, 380]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[0, 560], [4000, 560]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[0, 740], [4000, 740]], "w": 13, "id": "staff"},
                    {"k": "line", "pts": [[0, 920], [4000, 920]], "w": 13, "id": "staff"},
                    {"k": "glyph", "cp": "E0A4", "xf": [200, 560, 0.72, -0.72], "id": "clef"},
                    {"k": "glyph", "cp": "E0A4", "xf": [1500, 560, 0.72, -0.72], "id": "n1"},
                    {"k": "glyph", "cp": "E0A4", "xf": [2500, 380, 0.72, -0.72], "id": "n2"}
                ],
                "elements": ["n1", "n2"],
                "kinds": {"n1": "note", "n2": "note", "clef": "clef"}
            }"#,
        )
        .unwrap();
        ScoreData::parse(&props)
    }

    #[test]
    fn a_ledger_line_is_its_notehead_s_and_a_staff_line_runs_the_system() {
        let mut data = page();
        // a ledger line under the first note, under the staff's id as the
        // engraver files it
        data.prims.push(Prim::Line {
            pts: vec![[1440.0, 1100.0], [1720.0, 1100.0]],
            width: 13.0,
            id: Some("staff".into()),
        });
        let space = 2.0 * data.step;
        let lines: Vec<Option<(f32, f32)>> = data
            .prims
            .iter()
            .map(|prim| staff_line(&data, prim, space))
            .collect();
        assert_eq!(
            lines[0],
            Some((200.0, 13.0)),
            "a line the length of the system"
        );
        let ledger = data.prims.last().unwrap();
        assert_eq!(
            staff_line(&data, ledger, space),
            None,
            "a ledger line is no staff line"
        );
        assert_eq!(
            ledger_line(&data, ledger, space),
            Some((1580.0, 140.0, 1100.0))
        );
        // and no glyph, nor a staff's own line, is one
        assert!(
            data.prims[..8]
                .iter()
                .all(|prim| ledger_line(&data, prim, space).is_none())
        );
    }

    const COLORS: TimeColors = TimeColors {
        ink: [1.0; 4],
        backdrop: [0.0, 0.0, 0.0, 1.0],
    };

    #[test]
    fn a_page_is_drawn_where_the_row_can_show_a_staff_and_not_under_it() {
        let data = page();
        let anchors = vec![("n1".to_string(), 0.0), ("n2".to_string(), 48_000.0)];
        let x_of = |start: f64| 300.0 + (start / 48_000.0) as f32 * 400.0;
        let frame = |h: f32| TimeFrame {
            body: Rect::new(300.0, 10.0, 600.0, h),
            row: Rect::new(100.0, 10.0, 900.0, h),
        };
        let mut mesh = Mesh::new();
        assert!(data.render_on_time(&mut mesh, frame(90.0), &anchors, &x_of, COLORS));
        assert!(mesh.vertex_count() > 0);
        assert_eq!(mesh.clip(), None, "the caller's clip is put back");
        // a row a staff cannot be read in draws nothing, and says so
        let mut low = Mesh::new();
        assert!(!data.render_on_time(&mut low, frame(12.0), &anchors, &x_of, COLORS));
        assert_eq!(low.vertex_count(), 0);
        // a page none of whose elements has a time is still a page
        let mut still = Mesh::new();
        assert!(data.render_on_time(&mut still, frame(90.0), &[], &x_of, COLORS));
    }

    #[test]
    fn a_note_is_at_its_time_and_its_zone_is_not_stretched() {
        // two notes 300 page units apart, drawn 600 pixels apart at half a
        // pixel to the unit
        let warp = Warp::new(&[(1000.0, 200.0), (1300.0, 800.0)], 0.5, (100.0, 100.0));
        assert_eq!(warp.x(1000.0), 200.0);
        assert_eq!(warp.x(1300.0), 800.0);
        // inside a zone a page unit is half a pixel, as the staff is drawn
        assert_eq!(warp.x(1050.0), 225.0);
        assert_eq!(warp.x(1250.0), 775.0);
        assert_eq!(warp.x(900.0), 150.0, "and before the first note");
        assert_eq!(warp.x(1500.0), 900.0, "and past the last");
        // between the zones the page is stretched to join them
        assert_eq!(warp.x(1150.0), 500.0);
        assert_eq!(warp.start(), 900.0);
    }

    #[test]
    fn notes_nearer_than_their_zones_stay_in_order() {
        // 300 page units apart and 20 pixels: the zones give way
        let warp = Warp::new(&[(1000.0, 200.0), (1300.0, 220.0)], 0.5, (100.0, 100.0));
        assert_eq!(warp.x(1000.0), 200.0);
        assert_eq!(warp.x(1300.0), 220.0);
        let across: Vec<f32> = (0..=30).map(|i| warp.x(1000.0 + 10.0 * i as f32)).collect();
        assert!(
            across.windows(2).all(|pair| pair[0] <= pair[1]),
            "{across:?}"
        );
        // a column of notes is one anchor
        let chord = Warp::new(&[(1000.0, 200.0), (1000.4, 200.0)], 0.5, (100.0, 100.0));
        assert_eq!(chord.x(1050.0), 225.0);
        // and a page with no note has no time: it is drawn as it is
        assert_eq!(Warp::new(&[], 0.5, (100.0, 100.0)).x(400.0), 200.0);
    }
}
