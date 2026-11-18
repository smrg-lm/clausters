//! **What the dock draws of its own**: each panel's bar, the page strip
//! along its bottom, the divider under the pointer or in the hand, where a
//! dragged tab would land, and an open menu. Drawn over the panes, in the
//! dock's surface.
//!
//! A panel's bar reads as an editor's panel does: a grip at its start, the
//! window's title -- or, for a panel of several windows or of a document,
//! their tabs, the one shown standing out on the ground of the panel with a
//! line along its top -- and its buttons at the end. The page strip is a `choice`'s row of tabs
//! ([`choice::draw`]); a menu is the popup layer's list ([`draw_list`]).

use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, symbol};
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::{Color, Theme};

use super::{BarPart, Dock, GroupPlace};
use crate::host::frame::{draw_stack, draw_tip};
use crate::host::graphics::choice;
use crate::host::graphics::controls::icon;
use crate::host::popup::Tip;

impl Dock {
    /// **The dock's chrome**, into `d`.
    pub fn draw(&self, d: &mut Draw) {
        let size = self.text_size();
        for group in self.groups() {
            self.draw_bar(d, group);
        }
        // the pages, along the bottom; a floating panel has none
        if self.page_strip_h() > 0 {
            let strip = self.page_strip();
            d.mesh.rect(strip, d.theme.status_bar);
            choice::draw(
                d,
                choice::View::Tabs,
                &self.page_options(),
                self.page_index(),
                None,
                strip,
                size,
                self.lit_page(),
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
        if let Some(menu) = self.menu() {
            let (mesh, m, theme) = d.parts();
            draw_stack(mesh, &menu.stack, &self.menu_placed(), m, theme);
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
            draw_tip(mesh, &tip, work, m, theme);
        }
    }

    /// **One panel's bar**: its ground and the line under it, the grip, the
    /// title or the tabs, the buttons. Over a pane that fills the header
    /// ([`Dock::set_header`]) the middle is the pane's: its ground and its
    /// tools are drawn by the pane, under this.
    fn draw_bar(&self, d: &mut Draw, group: &GroupPlace) {
        let size = self.text_size();
        let lit = self.lit_bar(&group.path);
        let shown = group.panes.get(group.active).copied();
        let focused = self.focus().is_some_and(|f| shown == Some(f));
        let headed = shown.is_some_and(|p| self.is_headed(p));
        let (from, to) = if headed {
            self.slot(group)
        } else {
            (group.strip.x, group.strip.x)
        };
        let parts = self.bar_parts(group);
        let (mesh, m, theme) = d.parts();
        let bar = group.strip;
        mesh.set_clip(Some(bar));
        // the ground at the bar's ends, and under a middle that is not a pane's
        if headed {
            mesh.rect(Rect::new(bar.x, bar.y, from - bar.x, bar.h), theme.header);
            mesh.rect(
                Rect::new(to, bar.y, bar.x + bar.w - to, bar.h),
                theme.header,
            );
        } else {
            mesh.rect(bar, theme.header);
        }
        let glyph = font::height(size);
        // the close marks last, over the tabs they stand on
        let (marks, parts): (Vec<_>, Vec<_>) = parts
            .into_iter()
            .partition(|(part, _)| matches!(part, BarPart::TabClose(_)));
        for (part, r) in parts.into_iter().chain(marks) {
            let hot = lit == Some(part);
            match part {
                BarPart::Grip => grip(
                    mesh,
                    r,
                    size,
                    if hot { theme.text } else { theme.text_dim },
                    m.divider_w,
                ),
                BarPart::Title => {
                    let title = self.title(group.panes[group.active]);
                    font::text_ellipsis(
                        mesh,
                        &title,
                        r.x + m.pad,
                        r.y + (r.h - glyph) * 0.5,
                        (r.w - 2.0 * m.pad).max(0.0),
                        size,
                        theme.text,
                    );
                }
                BarPart::Tab(i) => {
                    let shown = i == group.active;
                    if shown {
                        // the tab shown stands on the panel's own ground
                        mesh.rect(r, theme.background);
                        let line = (2.0 * m.divider_w).max(2.0);
                        mesh.rect(
                            Rect::new(r.x, r.y, r.w, line),
                            if focused {
                                theme.focus
                            } else {
                                theme.separator
                            },
                        );
                    } else if hot || lit == Some(BarPart::TabClose(i)) {
                        mesh.rect(r, theme.hover);
                    }
                    mesh.rect(
                        Rect::new(r.x + r.w - m.divider_w, r.y, m.divider_w, r.h),
                        theme.separator,
                    );
                    let after = 2.0 * m.tab_end + m.tab_close;
                    font::text_ellipsis(
                        mesh,
                        &self.title(group.panes[i]),
                        r.x + m.tab_inset,
                        r.y + (r.h - glyph) * 0.5,
                        (r.w - m.tab_inset - after).max(0.0),
                        size,
                        if shown { theme.text } else { theme.text_dim },
                    );
                }
                // a tab's close mark shows on the tab shown and under the
                // hand; a tab whose work has changes not kept shows a dot in
                // its place, which turns into the mark under the hand
                BarPart::TabClose(i) => {
                    let under = lit == Some(BarPart::Tab(i)) || lit == Some(BarPart::TabClose(i));
                    if !under && self.is_modified(group.panes[i]) {
                        let dot = (r.h * 0.18).max(2.0 * m.divider_w);
                        mesh.disc(r.x + r.w * 0.5, r.y + r.h * 0.5, dot, theme.text);
                    } else if under || i == group.active {
                        // the specification's 12 of a tab's cross, where a tool's icon is 16
                        button(mesh, r, icon("x", symbol::CLOSE), hot, size * 0.75, theme);
                    }
                }
                BarPart::Menu => button(mesh, r, icon("menu", symbol::MENU), hot, size, theme),
                BarPart::Close => button(mesh, r, icon("x", symbol::CLOSE), hot, size, theme),
            }
        }
        mesh.set_clip(None);
        // the line under the bar, as under every panel's
        mesh.rect(
            Rect::new(bar.x, bar.y + bar.h - m.divider_w, bar.w, m.divider_w),
            theme.separator,
        );
    }
}

/// A button of a bar: its glyph, on the hover ground under the hand.
fn button(mesh: &mut Mesh, r: Rect, glyph: char, hot: bool, size: f32, theme: &Theme) {
    if hot {
        mesh.rect(r, theme.hover);
    }
    font::text_centered(
        mesh,
        &glyph.to_string(),
        r,
        size,
        if hot { theme.text } else { theme.text_dim },
    );
}

/// The grip: two columns of three dots, what a handle to drag by looks like
/// -- the icon, or the dots drawn where the host carries no icons.
fn grip(mesh: &mut Mesh, r: Rect, size: f32, color: Color, unit: f32) {
    if let Some(icon) = font::outline::icon("grip-vertical") {
        return font::text_centered(mesh, &icon.to_string(), r, size, color);
    }
    let dot = (1.5 * unit).max(1.0);
    let step = 4.0 * unit.max(1.0);
    let (cx, cy) = (r.x + r.w * 0.5, r.y + r.h * 0.5);
    for col in [-0.5, 0.5] {
        for row in [-1.0, 0.0, 1.0] {
            mesh.disc(cx + col * step, cy + row * step, dot, color);
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
