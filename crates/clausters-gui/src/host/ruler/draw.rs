//! **How a ruler is drawn**: one strip of ticks against the edge that faces
//! what it measures, on either axis and on either side.
//!
//! A ruler is drawn the same way whatever it counts: a mark at each tick,
//! grown from the strip's edge that touches the content ([`RulerDir`]), and
//! the tick's label beyond the mark. Over or under a view that is a row of
//! marks along the strip's bottom or top edge with the labels centred on them;
//! beside a view it is the same thing turned on its side, the labels stacked
//! against the marks. The strip on a view's right is the mirror of the one on
//! its left, as the one under a view is the mirror of the one over it, so
//! which side a ruler stands on is a choice of the layout and never a second
//! drawing.

use super::Tick;
use crate::host::widget::RulerDir;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font;
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::Color;

/// **The ink a ruler is drawn in**: the theme's ruler roles by default. A
/// ruler that stands over a picture instead of in a strip of its own -- a
/// layer's scale written inside the body it measures -- draws its labels on
/// the text plate and in the layer's own colour, so they read over whatever
/// is under them and say whose scale they are.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Look {
    /// The colour of its marks and labels, `None` for the theme's ruler ink.
    pub ink: Option<Color>,
    /// Whether each label stands on the text plate.
    pub plated: bool,
}

/// **How far down the strip a tick's label sits**, and how tall the tick
/// itself is -- named because a marker's arrow and its text stand on this same
/// row, and two literals in two files is how they come to disagree the first
/// time either moves.
pub(crate) const TICK_H: f32 = 6.0;
pub(crate) const TICK_LABEL_TOP: f32 = 7.0;

/// How long a labelled tick is across a **vertical** strip (a minor one is
/// half of it). Longer than [`TICK_H`] because the labels beside it are read
/// across, against the mark, where the horizontal ones are read under it.
const TICK_V: f32 = 8.0;

/// The clear space a vertical strip leaves between a label and the edge it
/// labels -- so the width a strip must reserve is its widest label plus this.
pub(crate) const LABEL_GAP: f32 = 10.0;

/// **Which edges of a row it shares with a row ruled beside it**: the rows
/// of a stack of channels, each ruled on its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Shared {
    /// The row above meets this one's top.
    pub top: bool,
    /// The row below meets this one's bottom.
    pub bottom: bool,
}

impl Shared {
    /// Row `index` of `rows` stacked edge to edge: every edge but the
    /// stack's own two is shared.
    pub(crate) fn of(index: usize, rows: usize) -> Shared {
        Shared {
            top: index > 0,
            bottom: index + 1 < rows,
        }
    }
}

/// **Draws `ticks` into `strip`**, measuring `ruled`: the content rectangle
/// whose axis the ticks are fractions of -- its width when the strip faces up
/// or down, its height (`frac` 0 at the bottom) when it faces left or right.
/// `shared` names the edges of a stacked row a label must keep clear of, so
/// two rows ruled one under the other rule the edge they share once.
///
/// The one drawing of every ruler: a view's own strips, a free-standing
/// ruler, a plot's, a scope's and a meter's ladder all come here.
pub(crate) fn ticks(
    d: &mut Draw,
    strip: Rect,
    ruled: Rect,
    ticks: &[Tick],
    dir: RulerDir,
    shared: Shared,
    look: Look,
) {
    match dir {
        RulerDir::Up | RulerDir::Down => horizontal(d, strip, ruled, ticks, dir, look),
        RulerDir::Left | RulerDir::Right => vertical(d, strip, ruled, ticks, dir, shared, look),
    }
}

/// Writes a label at `(x, y)` as `look` says: plain, or on the text plate.
fn label(d: &mut Draw, text: &str, x: f32, y: f32, color: Color, look: Look) {
    let scale = d.m.caption_scale;
    if look.plated {
        let w = font::width(text, scale);
        crate::host::graphics::plate_text(d, text, x, y, w, scale, color);
    } else {
        font::text(d.mesh, text, x, y, scale, color);
    }
}

/// A strip over or under its content: a mark against the edge `dir` names
/// (taller when labelled), the label centred on it and edge-clamped into the
/// strip.
fn horizontal(d: &mut Draw, strip: Rect, ruled: Rect, ticks: &[Tick], dir: RulerDir, look: Look) {
    let scale = d.m.caption_scale;
    let text_h = font::height(scale);
    let (line, text) = (
        look.ink.unwrap_or(d.theme.ruler_line),
        look.ink.unwrap_or(d.theme.ruler_text),
    );
    for tick in ticks {
        let x = ruled.x + ruled.w * tick.frac as f32;
        let h = if tick.label.is_some() {
            TICK_H
        } else {
            TICK_H * 0.5
        };
        // The tick grows from the strip's near edge inward, and the label
        // stands on the far side of it -- mirrored, so the row reads the same
        // either way up.
        let (ty, ly) = if dir == RulerDir::Up {
            (strip.y, strip.y + TICK_LABEL_TOP)
        } else {
            (
                strip.y + strip.h - h,
                strip.y + strip.h - TICK_LABEL_TOP - text_h,
            )
        };
        d.mesh.rect(Rect::new(x, ty, d.m.divider_w, h), line);
        if let Some(text_of) = &tick.label {
            let w = font::width(text_of, scale);
            let lx = (x - w * 0.5).clamp(strip.x, (strip.x + strip.w - w).max(strip.x));
            label(d, text_of, lx, ly, text, look);
        }
    }
}

/// A strip beside its content: a mark against the edge facing it (longer
/// when labelled), the label beyond the mark and kept inside the strip.
fn vertical(
    d: &mut Draw,
    strip: Rect,
    row: Rect,
    ticks: &[Tick],
    dir: RulerDir,
    shared: Shared,
    look: Look,
) {
    if row.h <= 4.0 {
        return;
    }
    let scale = d.m.caption_scale;
    let line = font::height(scale);
    let tops = label_tops(row, ticks, line, d.m.gap, shared);
    let (ink, text) = (
        look.ink.unwrap_or(d.theme.ruler_line),
        look.ink.unwrap_or(d.theme.ruler_text),
    );
    // The edge that faces the content: the strip's right one when the content
    // is on its right, its left one when the strip stands on the content's
    // right.
    let edge = if dir == RulerDir::Right {
        strip.x + strip.w
    } else {
        strip.x
    };
    for (tick, top) in ticks.iter().zip(tops) {
        let y = row.y + row.h * (1.0 - tick.frac as f32);
        let w = if tick.label.is_some() {
            TICK_V
        } else {
            TICK_V * 0.5
        };
        let tick_x = if dir == RulerDir::Right {
            edge - w
        } else {
            edge
        };
        d.mesh.rect(Rect::new(tick_x, y, w, d.m.divider_w), ink);
        if let (Some(text_of), Some(ty)) = (tick.label.as_ref(), top) {
            let lw = font::width(text_of, scale);
            let lx = if dir == RulerDir::Right {
                (edge - LABEL_GAP - lw).max(strip.x)
            } else {
                (edge + LABEL_GAP).min(strip.x + strip.w - lw)
            };
            label(d, text_of, lx, ty, text, look);
        }
    }
}

/// **The top of each label `ticks` draws beside `row`**, in order, `None`
/// for a tick drawn bare. A label is placed where its tick puts it and kept
/// inside the row; one a **shared** edge would have to push, or that comes
/// closer to it than half the clear space between two labels, is dropped
/// and its tick stays -- so the labels of two stacked rows keep the space
/// the labels of one row keep, and the edge they share is ruled once, by
/// the row whose label clears it.
pub(super) fn label_tops(
    row: Rect,
    ticks: &[Tick],
    line: f32,
    gap: f32,
    shared: Shared,
) -> Vec<Option<f32>> {
    // **A row shorter than one caption keeps its ticks and drops their
    // labels** -- the rule a squeezed control already follows
    // ([`crate::host::graphics::controls::label_height`]): a line that does not
    // fit is a drawing that lies, while a tick with no number is merely terser.
    // It is also what keeps the placement below well-formed: the label's band
    // is `row.y ..= row.y + row.h - line`, which inverts exactly here.
    let labelled = row.h >= line;
    let clear = gap * 0.5;
    ticks
        .iter()
        .map(|tick| {
            tick.label.as_ref().filter(|_| labelled)?;
            let y = row.y + row.h * (1.0 - tick.frac as f32);
            let top = y - 3.0;
            if (shared.top && top < row.y + clear)
                || (shared.bottom && top + line > row.y + row.h - clear)
            {
                return None;
            }
            Some(top.clamp(row.y, row.y + row.h - line))
        })
        .collect()
}

/// **The width a vertical strip has to be to draw `ticks` in full**: its
/// widest label plus [`LABEL_GAP`], or `0` when nothing is labelled.
///
/// This is what a fixed size role could not express. A label's width is a
/// property of the *data*: an axis over `[-1, 1]` formats `-1.0` and asks for
/// no more room than the role always gave it, while one over `[-0.1, 0.1]`
/// formats `-0.0625` and is clamped against the strip's edge unless it can ask
/// for the room to draw it. Callers take the role as the **floor**, so an axis
/// whose labels are narrow looks exactly as it always did.
pub(crate) fn ticks_width(ticks: &[Tick], metrics: &Metrics) -> f32 {
    let scale = metrics.caption_scale;
    let widest = ticks
        .iter()
        .filter_map(|t| t.label.as_deref())
        .map(|l| font::width(l, scale))
        .fold(0.0f32, f32::max);
    if widest > 0.0 {
        widest + LABEL_GAP
    } else {
        0.0
    }
}
