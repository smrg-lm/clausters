//! **What the dock draws of its own**: a tab strip over each group, the
//! divider under the pointer or in the hand, and where a dragged tab would
//! land. Drawn over the panes, in the dock's surface, with the drawing the
//! widgets use -- a group's strip is a `choice`'s row of tabs
//! ([`choice::draw`]), so a tab looks and is hit where a tab of a window is.

use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::rect::Rect;

use super::Dock;
use crate::host::graphics::choice;

impl Dock {
    /// **The dock's chrome**, into `d`: the strips, the lit divider, the
    /// landing.
    pub fn draw(&self, d: &mut Draw) {
        let size = self.text_size();
        for group in self.groups() {
            let titles: Vec<String> = group.panes.iter().map(|&p| self.title(p)).collect();
            d.mesh.rect(group.strip, d.theme.background);
            choice::draw(
                d,
                choice::View::Tabs,
                &titles,
                group.active,
                None,
                group.strip,
                size,
                self.lit_tab(&group.path),
            );
        }
        if let Some(divider) = self.lit_divider() {
            d.mesh.rect(divider.rect, d.theme.accent);
        }
        if let Some((_, place)) = self.landing() {
            let [r, g, b, _] = d.theme.accent;
            d.mesh.rect(place, [r, g, b, 0.25]);
            let w = (2.0 * d.m.divider_w).max(2.0);
            d.mesh.border(inset(place, w * 0.5), w, d.theme.accent);
        }
    }
}

/// `r` with `by` taken off every side.
fn inset(r: Rect, by: f32) -> Rect {
    Rect::new(
        r.x + by,
        r.y + by,
        (r.w - 2.0 * by).max(0.0),
        (r.h - 2.0 * by).max(0.0),
    )
}
