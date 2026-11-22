//! **What the dock draws of its own**: each panel's bar, the page strip
//! along its bottom, the divider under the pointer or in the hand, where a
//! dragged tab would land, and an open menu. Drawn over the panes, in the
//! dock's surface.
//!
//! A panel's bar reads as an editor's panel does: a grip at its start, the
//! window's title -- or, for a panel of several windows or of a document,
//! their tabs, the one shown standing out on the ground of the panel with a
//! line along its top -- and its buttons at the end. The page strip is a row
//! of the same tabs, mirrored, each page's with its cross but the first's; a
//! menu is the popup layer's list ([`draw_stack`]).

use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, symbol};
use clausters_gui_draw::rect::Rect;

use super::{Dock, GroupPlace, Landing, PagePart, Side};
use crate::host::frame::{draw_stack, draw_tip};
use crate::host::graphics::bar;
use crate::host::graphics::choice;
use crate::host::popup::Tip;

impl Dock {
    /// **The dock's chrome**, into `d`.
    pub fn draw(&self, d: &mut Draw) {
        // an application's page has no panel's bar
        for group in self.groups().iter().filter(|g| g.strip.h > 0.0) {
            self.draw_bar(d, group);
        }
        // the pages, along the bottom; a floating panel has none
        if self.page_strip_h() > 0 {
            self.draw_pages(d);
        }
        // the divider under the hand, in the ring's role
        if let Some(divider) = self.lit_divider() {
            d.mesh.rect(divider.rect, d.theme.focus);
        }
        if let Some((landing, place)) = self.landing() {
            self.draw_drop_places(d);
            self.draw_drop_zone(d, landing, place);
        }
        if let Some(menu) = self.menu() {
            let (mesh, m, theme) = d.parts();
            let placed = self.menu_placed();
            let owner = match menu.stack.anchor {
                crate::host::popup::Anchor::Below(a) => Some(a),
                _ => None,
            };
            draw_stack(
                mesh,
                &menu.stack,
                &placed,
                owner,
                Some(&self.entrance),
                m,
                theme,
            );
        }
        // a tab's tip, at the tab it was rested on
        if let Some((pane, text)) = self.shown_tip()
            && let Some(tab) = self.tab_rect(pane)
        {
            let tip = Tip {
                widget: pane,
                text: text.to_string(),
                key: None,
                anchor: tab,
            };
            let work = Rect::new(0.0, 0.0, self.size.0 as f32, self.size.1 as f32);
            let (mesh, m, theme) = d.parts();
            draw_tip(mesh, &tip, work, Some(&self.entrance), m, theme);
        }
    }

    /// **Where a dragged panel would land**, as the design system's drop
    /// zone: inset a little from the place, a dashed edge with the medium
    /// corner in the ring's role over the ring at a fifth, and the zone's
    /// name in the medium weight in its middle.
    fn draw_drop_zone(&self, d: &mut Draw, landing: Landing, place: Rect) {
        let (mesh, m, theme) = d.parts();
        let zone = inset(place, m.zone_inset);
        let w = 2.0 * m.divider_w;
        let [r, g, b, _] = theme.focus;
        mesh.round_rect(zone, m.radius_md, [r, g, b, 0.2]);
        mesh.dashed_round_border(zone, w, m.radius_md, 3.0 * w, theme.focus);
        let name = self.landing_name(landing);
        let scale = font::em_scale(font::Family::UiMedium, m.type_sm);
        let tw = font::width_in(font::Family::UiMedium, &name, scale);
        let x = zone.x + ((zone.w - tw) * 0.5).max(0.0);
        let y = zone.y + (zone.h - font::height(scale)) * 0.5;
        font::text_ellipsis_in(
            mesh,
            font::Family::UiMedium,
            &name,
            x,
            y,
            zone.w,
            scale,
            theme.text,
        );
    }

    /// **Every place a dragged panel may land**, faintly, while it is
    /// dragged: each panel's body under a dashed edge of the ring's role at
    /// two fifths, so the hand sees where it can go before it is there; the
    /// zone under it is drawn over them ([`draw_drop_zone`](Self::draw_drop_zone)).
    fn draw_drop_places(&self, d: &mut Draw) {
        let (mesh, m, theme) = d.parts();
        let w = 2.0 * m.divider_w;
        let [r, g, b, _] = theme.focus;
        for group in &self.groups {
            let zone = inset(group.body.rect(), m.zone_inset);
            mesh.dashed_round_border(zone, w, m.radius_md, 3.0 * w, [r, g, b, 0.4]);
        }
    }

    /// What a drop zone is called: the side a panel would stand on, the
    /// tabs it would join, the page it would go to, or a window of its own.
    fn landing_name(&self, landing: Landing) -> String {
        match landing {
            Landing::Split { side, .. } => match side {
                Side::Left => "Left",
                Side::Right => "Right",
                Side::Top => "Top",
                Side::Bottom => "Bottom",
            }
            .to_string(),
            Landing::Join { .. } => "Tabs".to_string(),
            Landing::Out => "New window".to_string(),
            Landing::Page(i) => self.page_names().get(i).cloned().unwrap_or_default(),
        }
    }

    /// **One panel's bar**: the bar its windows describe
    /// ([`Dock::bar_of`]), drawn by the one drawing of a bar. Over a pane
    /// that fills the header the middle is the pane's: its ground and its
    /// tools are drawn by the pane, under this.
    fn draw_bar(&self, d: &mut Draw, group: &GroupPlace) {
        let lit = self.lit_bar(&group.path);
        let described = self.bar_of(group);
        let (mesh, m, theme) = d.parts();
        mesh.set_clip(Some(group.strip));
        bar::draw(mesh, &described, group.strip, lit, m, theme);
        mesh.set_clip(None);
    }
}

impl Dock {
    /// **The page strip**: a panel's tabs, mirrored, since the strip is
    /// under the pages -- the line along its top, the page shown on the
    /// panel's ground with its line along the bottom, and each tab's cross
    /// on the page shown and under the hand.
    fn draw_pages(&self, d: &mut Draw) {
        let size = self.text_size();
        let strip = self.page_strip();
        let lit = self.lit_page();
        let names = self.page_names();
        let shown = self.page_index();
        let (mesh, m, theme) = d.parts();
        mesh.rect(strip, theme.status_bar);
        mesh.rect(
            Rect::new(strip.x, strip.y, strip.w, m.divider_w),
            theme.separator,
        );
        let parts = self.page_parts();
        let (marks, tabs): (Vec<_>, Vec<_>) = parts
            .into_iter()
            .partition(|(part, _)| matches!(part, PagePart::Close(_)));
        for (part, r) in tabs.into_iter().chain(marks) {
            let hot = lit == Some(part);
            match part {
                PagePart::Tab(i) => {
                    let mark = (i == shown).then_some(theme.separator);
                    let hot = hot || lit == Some(PagePart::Close(i));
                    let closes = self.page_closable(i);
                    bar::tab(
                        mesh,
                        m,
                        theme,
                        r,
                        &names[i],
                        mark,
                        hot,
                        closes,
                        choice::TabEdge::Bottom,
                        size,
                    );
                }
                PagePart::Close(i) => {
                    let under = lit == Some(PagePart::Tab(i)) || hot;
                    if under || i == shown {
                        bar::tab_cross(mesh, r, hot, size, theme);
                    }
                }
                PagePart::More(from) => {
                    let mark = (shown >= from).then_some(theme.separator);
                    choice::tab_ground(mesh, m, theme, r, mark, hot, choice::TabEdge::Bottom);
                    font::text_centered(mesh, &symbol::MENU.to_string(), r, size, theme.text_dim);
                }
            }
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
