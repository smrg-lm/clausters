//! **The ruler**: a scale drawn on a side, written once for every view that
//! has an axis.
//!
//! A ruler is two independent choices, and this module keeps them apart:
//!
//! - **what it shows**, a [`Scale`] ([`scale`]): time, a full-scale
//!   amplitude, a plain value, a frequency, a meter's decibels. A scale knows
//!   its ladder of steps and its labels over the visible window of its axis,
//!   and nothing about where it is drawn.
//! - **how it is drawn**, a [`RulerDir`] ([`draw`]): the edge of its strip
//!   that faces what it measures. Over a view, under it, on its left or on its
//!   right -- the same marks and labels, the strip on one side the mirror of
//!   the strip on the other.
//!
//! A [`Ruler`] is the pair, and it is what every caller asks: a view's own
//! strips, the free-standing rulers of an application's window (the time
//! ruler over or under its views, the value ruler beside one), a plot's
//! axes, a scope's and a meter's ladder. None of them formats a tick or
//! draws a mark of its own, so a scale added here is a scale every one of
//! them can show and a side added here is a side every one can stand on.

mod draw;
mod scale;

#[cfg(test)]
mod tests;

pub(crate) use draw::{Look, Shared, TICK_LABEL_TOP, ticks_width};
pub(crate) use scale::{
    Scale, Tick, TimeUnit, axis_floor, bars_in, display_to_hz, fmt_decimal, hz_to_display,
    note_readout, readout_amp, readout_beats, readout_samples, readout_time, readout_value,
    scale_tag, tuning_of,
};

use crate::host::widget::RulerDir;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

/// **The axis a ruler's labels are laid along**: side by side over or under a
/// view, one over the other beside it. It is the one thing a scale is told
/// about its drawing, since it decides how a label is measured against the
/// next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Along {
    Horizontal,
    Vertical,
}

/// **One ruler**: what it shows, which way its strip faces, and the ink it
/// is drawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ruler<'a> {
    pub scale: Scale<'a>,
    pub dir: RulerDir,
    pub look: Look,
}

impl<'a> Ruler<'a> {
    /// A ruler in the theme's ruler ink.
    pub(crate) fn new(scale: Scale<'a>, dir: RulerDir) -> Ruler<'a> {
        Ruler {
            scale,
            dir,
            look: Look::default(),
        }
    }

    /// The same ruler drawn as `look` says.
    pub(crate) fn with_look(self, look: Look) -> Ruler<'a> {
        Ruler { look, ..self }
    }

    /// The axis it rules.
    pub(crate) fn along(&self) -> Along {
        self.dir.along()
    }

    /// Its ticks over `window` of its axis, `length_px` device pixels long.
    pub(crate) fn ticks(&self, window: (f64, f64), length_px: f64, m: &Metrics) -> Vec<Tick> {
        self.scale.ticks(self.along(), window, length_px, m)
    }

    /// **How thick its strip has to be** across its axis, ruling `window` of
    /// an axis `length_px` long: the strip's role over or under a view, and
    /// beside one the role or its widest label, whichever is more
    /// ([`ticks_width`]) -- a label's width is the data's, so a zoomed axis
    /// asks for the room its labels take.
    pub(crate) fn thickness(&self, window: (f64, f64), length_px: f64, m: &Metrics) -> f32 {
        match self.along() {
            Along::Horizontal => m.ruler_h,
            Along::Vertical => m
                .ruler_w
                .max(ticks_width(&self.ticks(window, length_px, m), m)),
        }
    }

    /// **Takes its strip off `body`** on the side it faces from, `thickness`
    /// across, and returns the strip: off the bottom for a strip under its
    /// content, the top for one over it, the left for one on its left and the
    /// right for one on its right.
    pub(crate) fn reserve(&self, body: &mut Rect, thickness: f32) -> Rect {
        let t = thickness.max(0.0);
        match self.dir {
            RulerDir::Up => {
                let t = t.min(body.h);
                body.h -= t;
                Rect::new(body.x, body.y + body.h, body.w, t)
            }
            RulerDir::Down => {
                let t = t.min(body.h);
                body.y += t;
                body.h -= t;
                Rect::new(body.x, body.y - t, body.w, t)
            }
            RulerDir::Right => {
                let t = t.min(body.w);
                body.x += t;
                body.w -= t;
                Rect::new(body.x - t, body.y, t, body.h)
            }
            RulerDir::Left => {
                let t = t.min(body.w);
                body.w -= t;
                Rect::new(body.x + body.w, body.y, t, body.h)
            }
        }
    }

    /// **Draws it into `strip`**, ruling `window` of `ruled` -- the content
    /// rectangle (a view's body, one channel's row of it) whose width or
    /// height its ticks divide. `shared` names the edges a stacked row shares
    /// with its neighbours ([`Shared::of`]).
    pub(crate) fn draw(
        &self,
        d: &mut Draw,
        strip: Rect,
        ruled: Rect,
        window: (f64, f64),
        shared: Shared,
    ) {
        let length = match self.along() {
            Along::Horizontal => ruled.w,
            Along::Vertical => ruled.h,
        };
        let ticks = self.ticks(window, length as f64, d.m);
        self.draw_ticks(d, strip, ruled, &ticks, shared);
    }

    /// [`draw`](Self::draw) with the ticks already in hand -- for a caller
    /// that read them for something else as well.
    pub(crate) fn draw_ticks(
        &self,
        d: &mut Draw,
        strip: Rect,
        ruled: Rect,
        ticks: &[Tick],
        shared: Shared,
    ) {
        draw::ticks(d, strip, ruled, ticks, self.dir, shared, self.look);
    }
}
