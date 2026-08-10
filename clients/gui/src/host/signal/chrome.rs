//! **The chrome a signal element carries around its picture**: the band it
//! reserves left of its body for a value ruler.
//!
//! A navigable element sits on a *shared* time axis, and every member of that
//! axis draws its body at the same x — the widest gutter any of them asked for
//! — so the same sample sits at the same pixel in a lane, a roll and a view
//! stacked on one axis. What this file answers is this element's own wish;
//! reconciling the wishes is the group's ([`super::super::timeline`]).
//!
//! The wish is asked **twice**, which is the only subtlety here. Once from the
//! props alone, before anything is placed, because the layout needs it to place
//! a lane's clips at all; and once from the placement, because an amplitude
//! ruler's width is a property of the data — the same axis formats `-1.0`
//! unzoomed and `-0.0625` zoomed in, and the step it labels at depends on how
//! tall the element ended up.

use super::super::layout::Rect;
use super::super::metrics::Metrics;
use super::super::widget::RulerY;
use super::SignalElement;

impl SignalElement {
    /// The band this element reserves left of its body: a value ruler's width,
    /// or nothing when it draws no ruler.
    pub fn gutter(&self, m: &Metrics) -> f32 {
        if self.editor.ruler_y != RulerY::Off {
            m.ruler_w
        } else {
            0.0
        }
    }

    /// The band it wants once placed, when the measure is wider than the
    /// role-sized one — an **amplitude** ruler, whose labels are the data's.
    ///
    /// Two things it deliberately does not measure. A **hertz** axis stays on
    /// the role: its labels are short and bounded (`20K`, `1.5k`, `440`) and
    /// the frequency they run to is the analysis', not the tree's. And the
    /// element is measured as **one lane**: a stacked view's lanes are shorter
    /// than its body and so step more coarsely, so this asks for at most what a
    /// multichannel element needs and never for less — a gutter is a
    /// reservation, and reserving a character wide costs pixels where reserving
    /// short clamps a label.
    pub fn measured_gutter(&self, rect: Rect, m: &Metrics) -> Option<f32> {
        if !self.navigates_time() || matches!(self.editor.ruler_y, RulerY::Off | RulerY::Hz) {
            return None;
        }
        // The gutter is what we are solving for, so it is left out of the body:
        // it moves the body's x, never its height, which is all the measure
        // reads.
        let body = super::super::frame::timeline_body(rect, &self.editor, 0.0, m);
        if body.h <= 0.0 {
            return None;
        }
        let (y_start, y_len) = self.editor.y_view();
        Some(super::super::ruler::amp_strip_w(
            self.editor.ruler_y,
            body.h,
            self.editor.bit_depth,
            y_start,
            y_len,
            m,
        ))
    }
}
