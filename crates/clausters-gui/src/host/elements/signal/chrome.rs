//! **The chrome a signal element carries around its picture**: the band it
//! reserves left of its body for a value ruler.
//!
//! A navigable element sits on a *shared* time axis, and every member of that
//! axis draws its body at the same x -- the widest gutter any of them asked for
//! -- so the same sample sits at the same pixel in a lane, a roll and a view
//! stacked on one axis. What this file answers is this element's own wish;
//! reconciling the wishes is the group's ([`crate::host::timeline`]).
//!
//! The wish is the role's width and never the labels': a band that grew with
//! the digits a zoom writes moved the picture beside it, so the labels shrink
//! to fit the band instead (`font::fit_scale`, in the ruler's drawing).

use super::SignalElement;
use clausters_gui_draw::metrics::Metrics;

impl SignalElement {
    /// The band this element reserves left of its body: a value ruler's width,
    /// or nothing when it draws no ruler.
    pub fn gutter(&self, m: &Metrics) -> f32 {
        if self.editor.y_strip() {
            m.ruler_w
        } else {
            0.0
        }
    }
}
