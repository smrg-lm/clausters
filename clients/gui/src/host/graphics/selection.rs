//! **The sweep**: what a selection looks like, wherever a hand draws one.
//!
//! Four views let a hand sweep one — a lane of clips, a piano roll, a waveform,
//! a spectrogram — and a patcher sweeps one over its canvas. They had four
//! spellings of the drawing between them, and the four disagreed: the signal
//! views drew the band with the half-sample rule and the value restriction the
//! sweep had set, the roll drew a full-height band from the raw sample and threw
//! the pitch restriction it had just swept away, the lane drew **nothing at
//! all**, and the patcher drew its own rectangle in its own alpha. A reader who
//! learns what a selection looks like on one of them learns nothing about the
//! next.
//!
//! So the drawing is one function and the *difference* between the views is a
//! value: [`Vertical`] says what the second axis measures, which is the only
//! question a sweep asks that a view answers differently. What it decides is
//! whether the sweep is a **stripe** — the whole lane, because nothing
//! restricts it vertically — or a **rectangle** the hand cut out of it.
//!
//! The edges follow from the same answer rather than from a flag: a band the
//! full height of the lane owns only its two vertical edges (its top and bottom
//! are the lane's own), and a restricted one owns all four, because every one
//! of them is a value the hand chose.

use crate::host::layout::Rect;
use crate::host::paint::Draw;
use crate::host::theme::with_alpha;
use crate::viewport::View;

/// The wash a selection is filled with, over whatever it covers.
const FILL: f32 = 0.18;
/// The opacity of an edge the *hand* decided (the lane's own edges are the
/// lane's, and are not redrawn here).
const EDGE: f32 = 0.75;

/// What a view's **second axis** measures, which is what decides whether a
/// swept selection is a stripe or a rectangle.
///
/// Not "which widget is this": a spectrogram and a lane of clips give the same
/// answer for different reasons — the one measures bins and the other measures
/// nothing — and a rule that asked which widget it was would have to be told
/// about the next one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Vertical {
    /// Nothing a selection can be restricted on: a lane of clips (whose second
    /// axis is the stack of lanes itself) and a spectrogram (whose second axis
    /// is bins, not a value). The band is the whole height it is given.
    Whole,
    /// A trace over its value `domain`, seen through the vertical `window`
    /// (`(start, len)` in display units) the picture was drawn with — the pair
    /// that makes the band's edges land on the values the ruler beside it
    /// labels.
    Value {
        domain: (f32, f32),
        window: (f64, f64),
    },
    /// A roll's pitch window: the `lo`/`hi` its rows are drawn over.
    Pitch { lo: f32, hi: f32 },
}

/// The x range a sample span covers in `body`, or `None` when it covers no
/// pixels of it.
///
/// **The band runs from halfway to halfway.** `(start, len)` is a count of
/// samples — indices `start .. start + len` — and each of them owns the half
/// sample-width on either side of it, so the edges fall midway between the last
/// selected sample and the first one left out. Drawn edge-to-edge instead, the
/// band would end *on* the last selected sample and read as excluding it, which
/// is exactly the ambiguity a sample-level zoom exposes. Invisible at a musical
/// zoom, which is why the view that had its own arithmetic never showed it.
pub fn span_x(body: Rect, nav: &View, start: f64, len: f64) -> Option<(f32, f32)> {
    let to_x = |s: f64| {
        (body.x as f64 + (s - nav.start) / nav.len.max(f64::MIN_POSITIVE) * body.w as f64) as f32
    };
    let x0 = to_x(start - 0.5).clamp(body.x, body.x + body.w);
    let x1 = to_x(start + len - 0.5).clamp(body.x, body.x + body.w);
    (x1 > x0).then_some((x0, x1))
}

/// The vertical extents a selection covers, as `(y, height)` **per lane**: the
/// whole of each lane for a selection nothing restricts, and the restriction's
/// own slice of it otherwise.
///
/// The range is mapped through the same pair the picture was drawn with, so the
/// band's edges land on the values the picture puts there. A range that
/// survives a zoom out of the visible window is clipped to the lane rather than
/// dropped: the selection still holds those values, they are simply off screen.
///
/// The restriction is drawn in *every* lane, for the reason a value zoom is
/// centred in every lane: one vertical window serves them all and a value says
/// the same thing in each, so a range of values is a range in each of them.
pub fn bands(
    body: Rect,
    lanes: usize,
    restriction: Option<(f64, f64)>,
    vertical: Vertical,
) -> Vec<(f32, f32)> {
    let lanes = lanes.max(1);
    // Nothing restricting it is **one** stripe over the whole height, not one
    // per lane: the two look the same and only the first is honest about what
    // the hand did, which is what decides the edges below.
    let whole = vec![(body.y, body.h)];
    let Some((min, max)) = restriction.filter(|(a, b)| b > a) else {
        return whole;
    };
    match vertical {
        Vertical::Whole => whole,
        Vertical::Pitch { lo, hi } => {
            // A pitch axis is discrete: the band covers the *rows* it holds, so
            // it runs from the top edge of the highest to the bottom edge of the
            // lowest — half a row past each centre, which is where the row is
            // drawn from.
            let y = |p: f64| crate::host::graphics::pianoroll::pitch_to_y(p as f32, lo, hi, body);
            clipped(body, y(max + 0.5), y(min - 0.5))
                .map(|band| vec![band])
                .unwrap_or_default()
        }
        Vertical::Value { domain, window } => {
            let (y0, y_len) = window;
            let mut out = Vec::with_capacity(lanes);
            for ch in 0..lanes {
                let lane = lane_of(body, lanes, ch);
                let lane = Rect::new(body.x, lane.0, body.w, lane.1);
                // Value -> display -> the lane's own height, the inverse of the
                // read the sweep made.
                let y_of = |v: f64| {
                    let d = crate::waveform::value_to_display(v as f32, domain.0, domain.1);
                    let rel = 1.0 - ((d - y0) / y_len.max(f64::MIN_POSITIVE));
                    lane.y + (rel as f32) * lane.h
                };
                out.extend(clipped(lane, y_of(max), y_of(min)));
            }
            out
        }
    }
}

/// One lane's `(y, height)` in a stack of `lanes`.
fn lane_of(body: Rect, lanes: usize, ch: usize) -> (f32, f32) {
    let r = crate::waveform::lane_rect(body, lanes, ch);
    (r.y, r.h)
}

/// `(y, height)` for `top..bottom` clipped into `r`, or nothing when the two
/// edges close up inside it.
fn clipped(r: Rect, top: f32, bottom: f32) -> Option<(f32, f32)> {
    let (top, bottom) = (top.clamp(r.y, r.y + r.h), bottom.clamp(r.y, r.y + r.h));
    (bottom > top).then_some((top, bottom - top))
}

/// **Draws the sweep**: the wash between `x0` and `x1` over each of `bands`,
/// with the edges the hand decided.
///
/// `full` is the height a band has when nothing restricts it — the lane's own —
/// and it is what tells the two cases apart without a second argument saying so.
pub fn draw(d: &mut Draw, x0: f32, x1: f32, bands: &[(f32, f32)], full: (f32, f32)) {
    let (mesh, m, theme) = d.parts();
    let (fill, edge) = (
        with_alpha(theme.selection, FILL),
        with_alpha(theme.selection, EDGE),
    );
    let w = m.divider_w;
    for &(y, h) in bands {
        mesh.rect(Rect::new(x0, y, x1 - x0, h), fill);
        mesh.rect(Rect::new(x0, y, w, h), edge);
        mesh.rect(Rect::new(x1 - w, y, w, h), edge);
        // The horizontal edges are the hand's only where the band is not the
        // whole lane: a stripe's top and bottom are the lane's own, and drawing
        // over them would claim the sweep put them there.
        if (y - full.0).abs() > 0.01 || (h - full.1).abs() > 0.01 {
            mesh.rect(Rect::new(x0, y, x1 - x0, w), edge);
            mesh.rect(Rect::new(x0, y + h - w, x1 - x0, w), edge);
        }
    }
}

/// The whole of it for a timeline view: the sample span `sel` mapped into
/// `body` through `nav`, over the extents its second axis leaves it.
///
/// **This is the one call a view makes.** A view that computed its own x range
/// or its own bands would be the fifth spelling, and the four that came before
/// are why this exists.
pub fn draw_span(
    d: &mut Draw,
    body: Rect,
    nav: &View,
    sel: Option<(f64, f64)>,
    lanes: usize,
    restriction: Option<(f64, f64)>,
    vertical: Vertical,
) {
    let Some((start, len)) = sel else { return };
    let Some((x0, x1)) = span_x(body, nav, start, len) else {
        return;
    };
    let bands = bands(body, lanes, restriction, vertical);
    draw(d, x0, x1, &bands, (body.y, body.h));
}

/// The sweep over a **plane** — a patcher's canvas, where the rectangle is the
/// selection itself and both of its axes are the hand's.
///
/// The same wash and the same edges as a timeline's: one hand sweeping one
/// rectangle looks like one thing, whatever it is sweeping over.
pub fn draw_rect(d: &mut Draw, r: Rect) {
    // Restricted on both axes by construction, which is what `full` says here:
    // no band of this rectangle is the whole of anything, so all four edges are
    // drawn.
    draw(d, r.x, r.x + r.w, &[(r.y, r.h)], (f32::NAN, f32::NAN));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::layout::Rect;

    const BODY: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 200.0,
        h: 100.0,
    };

    /// The half-sample rule, which one of the four call sites did not have: the
    /// band's edges fall midway between the last selected sample and the first
    /// one left out, so a one-sample selection is one sample wide and not zero.
    #[test]
    fn a_span_runs_from_halfway_to_halfway() {
        let nav = View {
            start: 0.0,
            len: 200.0,
        };
        let (x0, x1) = span_x(BODY, &nav, 10.0, 1.0).expect("a sample is a width");
        assert!((x1 - x0 - 1.0).abs() < 0.01, "one sample, one pixel here");
        assert!((x0 - 9.5).abs() < 0.01, "half a sample before it");
        // A span the window has scrolled past covers no pixels at all.
        let past = View {
            start: 1000.0,
            len: 200.0,
        };
        assert_eq!(span_x(BODY, &past, 10.0, 1.0), None);
    }

    /// Nothing to restrict on — a lane of clips, a spectrogram — is the whole
    /// lane, however many lanes there are and whatever a stale range says.
    #[test]
    fn an_unrestricted_sweep_is_the_whole_lane() {
        assert_eq!(bands(BODY, 1, None, Vertical::Whole), vec![(0.0, 100.0)]);
        assert_eq!(
            bands(BODY, 1, Some((0.0, 1.0)), Vertical::Whole),
            vec![(0.0, 100.0)],
            "a value range restricts nothing where nothing measures a value",
        );
        assert_eq!(
            bands(BODY, 2, None, Vertical::Whole).len(),
            1,
            "one stripe over the stack, not one per lane",
        );
    }

    /// A pitch axis is discrete, so the band covers the rows it holds — from the
    /// top of the highest to the bottom of the lowest, not their centres.
    #[test]
    fn a_pitch_restriction_covers_the_rows_it_holds() {
        let vertical = Vertical::Pitch { lo: 60.0, hi: 71.0 };
        let whole = bands(BODY, 1, None, vertical);
        let band = bands(BODY, 1, Some((62.0, 63.0)), vertical);
        assert_eq!(whole, vec![(0.0, 100.0)]);
        assert_eq!(band.len(), 1);
        let (y, h) = band[0];
        let row = 100.0 / 12.0;
        assert!(
            (h - 2.0 * row).abs() < 0.5,
            "two semitones, two rows: {h} against {row}",
        );
        assert!(
            y > 0.0 && y + h < 100.0,
            "inside the grid, not the whole of it"
        );
    }

    /// The stripe owns its two vertical edges; a restricted band owns all four,
    /// because every one of them is a value the hand chose.
    #[test]
    fn a_stripe_draws_two_edges_and_a_rectangle_draws_four() {
        // A rectangle is two triangles, so the count is in vertices: what the
        // test is after is how many boxes went down, and six of them are one.
        let boxes = |bands: &[(f32, f32)], full: (f32, f32)| {
            let m = crate::host::metrics::Metrics::default();
            let theme = crate::host::theme::Theme::default();
            let mut mesh = crate::host::paint::Mesh::new();
            draw(
                &mut Draw::new(&mut mesh, &m, &theme),
                10.0,
                90.0,
                bands,
                full,
            );
            mesh.vertex_count() / 6
        };
        // fill + two edges
        assert_eq!(boxes(&[(0.0, 100.0)], (0.0, 100.0)), 3);
        // fill + four edges
        assert_eq!(boxes(&[(20.0, 40.0)], (0.0, 100.0)), 5);
    }
}
