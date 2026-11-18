//! **What the dock draws of its own**: each panel's bar, the page strip
//! along its bottom, the divider under the pointer or in the hand, where a
//! dragged tab would land, and an open menu. Drawn over the panes, in the
//! dock's surface.
//!
//! A panel's bar reads as an editor's panel does: a grip at its start, a tab
//! for each window, the one shown standing out on the ground of the panel
//! with a line along its top, and its menu button at the end (a floating
//! panel's bar shows its window's title instead). The page strip is a `choice`'s row of tabs
//! ([`choice::draw`]); a menu is the popup layer's list ([`draw_list`]).

use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, symbol};
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::{Color, Theme};

use super::{BarPart, Dock, GroupPlace};
use crate::host::frame::draw_list;
use crate::host::graphics::choice;

impl Dock {
    /// **The dock's chrome**, into `d`.
    pub fn draw(&self, d: &mut Draw) {
        let size = self.text_size();
        for group in self.groups() {
            self.draw_bar(d, group);
        }
        // the pages, along the bottom; a floating panel has none
        if !self.is_floating() {
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
            for (level, placed) in menu.stack.levels.iter().zip(self.menu_placed()) {
                draw_list(mesh, level, &placed, menu.stack.text_size, m, theme);
            }
            mesh.set_clip(None);
        }
    }

    /// **One panel's bar**: its ground and the line under it, the grip, the
    /// title or the tabs, the buttons.
    fn draw_bar(&self, d: &mut Draw, group: &GroupPlace) {
        let size = self.text_size();
        let lit = self.lit_bar(&group.path);
        let focused = self
            .focus()
            .is_some_and(|f| group.panes.get(group.active) == Some(&f));
        let parts = self.bar_parts(group);
        let (mesh, m, theme) = d.parts();
        let bar = group.strip;
        mesh.set_clip(Some(bar));
        mesh.rect(bar, theme.header);
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
                    let close = (r.h * 0.7).round();
                    font::text_ellipsis(
                        mesh,
                        &self.title(group.panes[i]),
                        r.x + m.pad * 1.5,
                        r.y + (r.h - glyph) * 0.5,
                        (r.w - 2.0 * m.pad - close).max(0.0),
                        size,
                        if shown { theme.text } else { theme.text_dim },
                    );
                }
                // a tab's close mark shows on the tab shown and under the hand
                BarPart::TabClose(i) => {
                    let on_tab = i == group.active
                        || lit == Some(BarPart::Tab(i))
                        || lit == Some(BarPart::TabClose(i));
                    if on_tab {
                        button(mesh, r, symbol::CLOSE, hot, size, theme);
                    }
                }
                BarPart::Menu => button(mesh, r, symbol::MENU, hot, size, theme),
                BarPart::Close => button(mesh, r, symbol::CLOSE, hot, size, theme),
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

/// The grip: two columns of three dots, what a handle to drag by looks like.
fn grip(mesh: &mut Mesh, r: Rect, color: Color, unit: f32) {
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
