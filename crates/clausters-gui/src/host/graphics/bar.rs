//! **A panel's bar**: the strip along a panel's top, drawn one way wherever
//! a panel is -- a dock's pane, or a section in an application's column.
//!
//! What differs between the two is which parts the bar shows (a grip, a fold
//! mark, a title, tabs, a menu, a close mark) and how thick it is, never how
//! a part looks: the ground and the line under it, a button's glyph and its
//! hover, the grip's dots and the fold mark are drawn here, once, and the
//! dock's chrome and a group's strip both call them. A section's bar is the
//! dock's with every part but the fold mark and the title turned off, on a
//! strip a line of control tall.

use clausters_gui_draw::font::{self, Family, symbol};
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::{Color, Theme};

use super::controls::icon;

/// The text size of a bar's title and tabs: the interface's size of the type
/// scale.
pub fn text_size(m: &Metrics) -> f32 {
    font::em_scale(Family::Ui, m.type_sm)
}

/// **A bar's ground**: the header's role over `bar`, and the separator's line
/// along its foot, as under every panel's bar.
pub fn ground(mesh: &mut Mesh, bar: Rect, m: &Metrics, theme: &Theme) {
    mesh.rect(bar, theme.header);
    line(mesh, bar, m, theme);
}

/// The line along the foot of `bar`, alone: what a bar whose middle another
/// drew (a pane's own tools) still ends in.
pub fn line(mesh: &mut Mesh, bar: Rect, m: &Metrics, theme: &Theme) {
    mesh.rect(
        Rect::new(bar.x, bar.y + bar.h - m.divider_w, bar.w, m.divider_w),
        theme.separator,
    );
}

/// **A bar's title** in `r`, from the pad, cut to an ellipsis where it would
/// run past `r`.
pub fn title(mesh: &mut Mesh, r: Rect, text: &str, size: f32, m: &Metrics, theme: &Theme) {
    font::text_ellipsis(
        mesh,
        text,
        r.x + m.pad,
        r.y + (r.h - font::height(size)) * 0.5,
        (r.w - 2.0 * m.pad).max(0.0),
        size,
        theme.text,
    );
}

/// A button of a bar: its glyph, on the hover ground under the hand.
pub fn button(mesh: &mut Mesh, r: Rect, glyph: char, hot: bool, size: f32, theme: &Theme) {
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

/// **The fold mark**: a button whose chevron points down while the panel is
/// open and to the side while it is folded to its bar.
pub fn fold(mesh: &mut Mesh, r: Rect, folded: bool, hot: bool, size: f32, theme: &Theme) {
    let glyph = if folded {
        icon("chevron-right", symbol::POINT_RIGHT)
    } else {
        icon("chevron-down", symbol::POINT_DOWN)
    };
    button(mesh, r, glyph, hot, size, theme);
}

/// The grip: two columns of three dots, what a handle to drag by looks like
/// -- the icon, or the dots drawn where the host carries no icons.
pub fn grip(mesh: &mut Mesh, r: Rect, size: f32, color: Color, unit: f32) {
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
