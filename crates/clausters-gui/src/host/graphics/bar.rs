//! **A panel's bar**: the strip along a panel's top, one configured piece
//! wherever a panel is -- a dock's pane, or a section in an application's
//! column.
//!
//! What differs between bars is data, never drawing: which parts a bar
//! shows (a grip, a fold mark, a title or tabs, a menu button, a close
//! mark), described by a [`Bar`]. One function places the parts it shows
//! ([`parts`]), one draws them ([`draw`]), and the hit test reads the same
//! placement, so a part is pressed where it is drawn. The dock describes
//! each panel's bar from what its windows say (a document's tabs, a window
//! that may not be closed, one that may fold), and a titled group in a
//! window describes a bar with its title alone, and its fold mark when it
//! is a section. How thick a bar is, is the rectangle its owner gives it.

use clausters_gui_draw::font::{self, Family, symbol};
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::{Color, Theme};

use super::choice;
use super::controls::icon;

/// One thing a hand can land on in a bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The handle the panel is dragged by.
    Grip,
    /// The mark that folds the panel to its bar and opens it again.
    Fold,
    /// The panel's title, when it shows no tabs.
    Title,
    /// The tab at this index.
    Tab(usize),
    /// That tab's close mark.
    TabClose(usize),
    /// The button that opens the panel's menu.
    Menu,
    /// The panel's close mark.
    Close,
}

/// **A bar's fold mark**: whether the panel is folded now, and whether a
/// press may fold it (a mark that may not is drawn quiet, and keeps its
/// place so nothing beside it moves).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fold {
    pub folded: bool,
    pub enabled: bool,
}

/// One tab of a bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tab {
    pub name: String,
    /// It has a close mark.
    pub closable: bool,
    /// Its work has changes not kept: a dot in its close mark's place.
    pub modified: bool,
}

/// What the middle of a bar shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Middle {
    /// A title, cut to an ellipsis where it runs out of room.
    Title(String),
    /// A tab per window, `active` the one shown; with `half`, the tabs keep
    /// to half the room and the rest is the pane's (its tools).
    Tabs {
        tabs: Vec<Tab>,
        active: usize,
        half: bool,
    },
    /// Nothing of the bar's own: a pane draws its tools there.
    Pane,
}

/// **A bar, configured**: which parts it shows. Every part is an option.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub grip: bool,
    pub fold: Option<Fold>,
    pub middle: Middle,
    pub menu: bool,
    pub close: bool,
    /// The keyboard is in the panel: the tab shown is marked in the ring's
    /// role.
    pub focused: bool,
    /// A line along the bar's top too: what parts a section from the one
    /// above it in a column whose sections stand together, where a dock has
    /// its divider.
    pub line_top: bool,
}

impl Bar {
    /// **A bar with `title` and nothing else**: a titled group's.
    pub fn titled(title: &str) -> Self {
        Self {
            grip: false,
            fold: None,
            middle: Middle::Title(title.to_string()),
            menu: false,
            close: false,
            focused: false,
            line_top: false,
        }
    }
}

/// The text size of a bar's title and tabs: the interface's size of the type
/// scale.
pub fn text_size(m: &Metrics) -> f32 {
    font::em_scale(Family::Ui, m.type_sm)
}

/// **The parts `bar` shows in `r`**, each with the rectangle it is drawn and
/// pressed in: the grip, the fold mark, the title or the tabs (each tab's
/// close mark before the tab, so it is hit first), the menu button and the
/// close mark. Tabs that do not fit are cut to an ellipsis down to a least,
/// and those past the bar's end are left out.
pub fn parts(bar: &Bar, r: Rect, m: &Metrics) -> Vec<(Part, Rect)> {
    let h = r.h;
    let size = text_size(m);
    let mut out = Vec::new();
    let mut x = r.x;
    if bar.grip {
        out.push((Part::Grip, Rect::new(x, r.y, m.panel_grip, h)));
        x += m.panel_grip;
    }
    if bar.fold.is_some() {
        out.push((Part::Fold, Rect::new(x, r.y, h, h)));
        x += h;
    }
    let tabs = matches!(bar.middle, Middle::Tabs { .. });
    let buttons = f32::from(u8::from(bar.menu) + u8::from(bar.close));
    // a title keeps the room of both buttons whenever it has one, so it
    // ends at the same place on every bar of a dock
    let reserved = if tabs || buttons == 0.0 { buttons } else { 2.0 };
    let end = (r.x + r.w - reserved * h).max(x);
    match &bar.middle {
        Middle::Title(_) => out.push((Part::Title, Rect::new(x, r.y, end - x, h))),
        Middle::Pane => {}
        Middle::Tabs { tabs, half, .. } => {
            // a tab: its inset, its name, a gap, its close mark and a gap --
            // a long name is cut, not followed, at the widest a tab grows
            let close = m.tab_close;
            let natural: Vec<f32> = tabs
                .iter()
                .map(|t| tab_width(&t.name, size, m, t.closable))
                .collect();
            let room = (end - x).max(0.0);
            let room = if *half { room * 0.5 } else { room };
            let least = tab_width("\u{2026}", size, m, true);
            let total: f32 = natural.iter().sum();
            let each = (room / natural.len().max(1) as f32).max(least);
            let start = x;
            let mut tx = x;
            for (i, w) in natural.iter().enumerate() {
                let w = if total <= room { *w } else { w.min(each) };
                if tx + w > start + room + 0.5 {
                    break;
                }
                if tabs[i].closable {
                    out.push((
                        Part::TabClose(i),
                        Rect::new(
                            tx + w - close - m.tab_end,
                            r.y + (h - close) * 0.5,
                            close,
                            close,
                        ),
                    ));
                }
                out.push((Part::Tab(i), Rect::new(tx, r.y, w, h)));
                tx += w;
            }
        }
    }
    // the buttons from the bar's end: the close mark last
    let mut bx = r.x + r.w - buttons * h;
    if bar.menu {
        out.push((Part::Menu, Rect::new(bx, r.y, h, h)));
        bx += h;
    }
    if bar.close {
        out.push((Part::Close, Rect::new(bx, r.y, h, h)));
    }
    out
}

/// **The middle of a bar placed as `parts`**, from where its start's parts
/// end (the grip, the fold mark, the tabs) to where its end's begin (the
/// buttons), as x: what a pane filling it draws its tools in.
pub fn slot(parts: &[(Part, Rect)], r: Rect) -> (f32, f32) {
    let to = parts
        .iter()
        .filter(|(p, _)| matches!(p, Part::Menu | Part::Close))
        .map(|(_, r)| r.x)
        .fold(r.x + r.w, f32::min);
    let from = parts
        .iter()
        .filter(|(p, _)| matches!(p, Part::Grip | Part::Fold | Part::Tab(_)))
        .map(|(_, r)| r.x + r.w)
        .fold(r.x, f32::max);
    (from, to.max(from))
}

/// **Draws `bar` in `r`**, its parts placed as [`parts`] places them and
/// `lit` the one under the hand: the ground (at the ends only, around a
/// middle that is a pane's), each part, the close marks over the tabs they
/// stand on, and the line along the foot.
pub fn draw(mesh: &mut Mesh, bar: &Bar, r: Rect, lit: Option<Part>, m: &Metrics, theme: &Theme) {
    let size = text_size(m);
    let placed = parts(bar, r, m);
    if bar.middle == Middle::Pane {
        let (from, to) = slot(&placed, r);
        mesh.rect(Rect::new(r.x, r.y, from - r.x, r.h), theme.header);
        mesh.rect(Rect::new(to, r.y, r.x + r.w - to, r.h), theme.header);
    } else {
        mesh.rect(r, theme.header);
    }
    let (marks, placed): (Vec<_>, Vec<_>) = placed
        .into_iter()
        .partition(|(part, _)| matches!(part, Part::TabClose(_)));
    for (part, at) in placed.into_iter().chain(marks) {
        let hot = lit == Some(part);
        match part {
            Part::Grip => grip(
                mesh,
                at,
                size,
                if hot { theme.text } else { theme.text_dim },
                m.divider_w,
            ),
            Part::Fold => match bar.fold {
                Some(f) if f.enabled => fold(mesh, at, f.folded, hot, size, theme),
                _ => fold(mesh, at, false, false, size, &theme.disabled()),
            },
            Part::Title => {
                if let Middle::Title(text) = &bar.middle {
                    title(mesh, at, text, size, m, theme);
                }
            }
            Part::Tab(i) => {
                let Middle::Tabs { tabs, active, .. } = &bar.middle else {
                    continue;
                };
                // the tab shown stands on the panel's own ground, its line
                // in the ring's role while its panel has the keys
                let mark = (i == *active).then_some(if bar.focused {
                    theme.focus
                } else {
                    theme.separator
                });
                let hot = hot || lit == Some(Part::TabClose(i));
                tab(
                    mesh,
                    m,
                    theme,
                    at,
                    &tabs[i].name,
                    mark,
                    hot,
                    tabs[i].closable,
                    choice::TabEdge::Top,
                    size,
                );
            }
            // a tab's close mark shows on the tab shown and under the hand;
            // a tab whose work has changes not kept shows a dot in its
            // place, which turns into the mark under the hand
            Part::TabClose(i) => {
                let Middle::Tabs { tabs, active, .. } = &bar.middle else {
                    continue;
                };
                let under = lit == Some(Part::Tab(i)) || hot;
                if !under && tabs[i].modified {
                    let dot = (at.h * 0.18).max(2.0 * m.divider_w);
                    mesh.disc(at.x + at.w * 0.5, at.y + at.h * 0.5, dot, theme.text);
                } else if under || i == *active {
                    tab_cross(mesh, at, hot, size, theme);
                }
            }
            Part::Menu => button(mesh, at, icon("menu", symbol::MENU), hot, size, theme),
            Part::Close => button(mesh, at, icon("x", symbol::CLOSE), hot, size, theme),
        }
    }
    line(mesh, r, m, theme);
    if bar.line_top {
        mesh.rect(Rect::new(r.x, r.y, r.w, m.divider_w), theme.separator);
    }
}

/// The line along the foot of `bar`, as under every panel's bar.
pub fn line(mesh: &mut Mesh, bar: Rect, m: &Metrics, theme: &Theme) {
    mesh.rect(
        Rect::new(bar.x, bar.y + bar.h - m.divider_w, bar.w, m.divider_w),
        theme.separator,
    );
}

/// **A bar's title** in `r`, from the pad, cut to an ellipsis where it would
/// run past `r`.
fn title(mesh: &mut Mesh, r: Rect, text: &str, size: f32, m: &Metrics, theme: &Theme) {
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

/// **What a tab takes beside its name**, before it and after it: its inset,
/// then the gaps around its cross when it closes (`closes`), else the inset
/// again. The one geometry a row of tabs is measured by ([`tab_width`]) and
/// a tab's name is drawn by ([`tab`]), so a name measured to fit is drawn
/// whole.
pub fn tab_ends(m: &Metrics, closes: bool) -> (f32, f32) {
    let after = if closes {
        2.0 * m.tab_end + m.tab_close
    } else {
        m.tab_inset
    };
    (m.tab_inset, after)
}

/// **The width a tab of `name` takes**: the name and its ends
/// ([`tab_ends`]), up to the widest a tab grows, past which the name is cut.
pub fn tab_width(name: &str, size: f32, m: &Metrics, closes: bool) -> f32 {
    let (before, after) = tab_ends(m, closes);
    (font::width(name, size) + before + after).min(m.tab_w)
}

/// **A tab**, as a bar and the dock's page strip draw one: its ground,
/// marked when `mark` says it is the one shown, and its name from its inset,
/// cut to leave room for its cross when it has one (`closes`).
#[allow(clippy::too_many_arguments)] // one tab: its place, its name, its state
pub fn tab(
    mesh: &mut Mesh,
    m: &Metrics,
    theme: &Theme,
    r: Rect,
    name: &str,
    mark: Option<Color>,
    hot: bool,
    closes: bool,
    edge: choice::TabEdge,
    size: f32,
) {
    choice::tab_ground(mesh, m, theme, r, mark, hot, edge);
    let (before, after) = tab_ends(m, closes);
    let glyph = font::height(size);
    font::text_ellipsis(
        mesh,
        name,
        r.x + before,
        r.y + (r.h - glyph) * 0.5,
        (r.w - before - after).max(0.0),
        size,
        if mark.is_some() {
            theme.text
        } else {
            theme.text_dim
        },
    );
}

/// **A tab's cross**: the specification's 12 of it, where a tool's icon is
/// 16.
pub fn tab_cross(mesh: &mut Mesh, r: Rect, hot: bool, size: f32, theme: &Theme) {
    button(mesh, r, icon("x", symbol::CLOSE), hot, size * 0.75, theme);
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
fn fold(mesh: &mut Mesh, r: Rect, folded: bool, hot: bool, size: f32, theme: &Theme) {
    let glyph = if folded {
        icon("chevron-right", symbol::POINT_RIGHT)
    } else {
        icon("chevron-down", symbol::POINT_DOWN)
    };
    button(mesh, r, glyph, hot, size, theme);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(middle: Middle) -> Bar {
        Bar {
            middle,
            ..Bar::titled("")
        }
    }

    /// **Every part is an option**: a bar shows what it is configured with
    /// and nothing else, in the same places whichever other parts it has.
    #[test]
    fn a_bar_shows_the_parts_it_is_configured_with() {
        let m = Metrics::default();
        let r = Rect::new(0.0, 0.0, 400.0, 24.0);
        let names = |b: &Bar| {
            parts(b, r, &m)
                .into_iter()
                .map(|(p, _)| p)
                .collect::<Vec<_>>()
        };
        let section = Bar {
            fold: Some(Fold::default()),
            ..Bar::titled("Display")
        };
        assert_eq!(names(&section), [Part::Fold, Part::Title]);
        let placed = parts(&section, r, &m);
        assert_eq!(placed[0].1, Rect::new(0.0, 0.0, 24.0, 24.0), "at the start");
        assert_eq!(placed[1].1.x + placed[1].1.w, 400.0, "the title to the end");
        let panel = Bar {
            grip: true,
            menu: true,
            close: true,
            ..Bar::titled("Post")
        };
        assert_eq!(
            names(&panel),
            [Part::Grip, Part::Title, Part::Menu, Part::Close]
        );
        let placed = parts(&panel, r, &m);
        assert_eq!(placed[3].1.x, 376.0, "the close mark at the end");
        assert_eq!(placed[2].1.x, 352.0, "the menu before it");
        let tabs = bar(Middle::Tabs {
            tabs: vec![
                Tab {
                    name: "a.py".into(),
                    closable: true,
                    modified: false,
                },
                Tab {
                    name: "b.py".into(),
                    closable: false,
                    modified: false,
                },
            ],
            active: 0,
            half: false,
        });
        assert_eq!(
            names(&tabs),
            [Part::TabClose(0), Part::Tab(0), Part::Tab(1)],
            "a close mark only where the tab has one"
        );
        assert!(names(&bar(Middle::Pane)).is_empty());
    }

    /// **A name measured to fit is drawn whole**: a tab's width is its name
    /// and its ends, and its name is drawn in that width minus them, so in
    /// the face the host draws with, at every size, closing or not, no name
    /// is cut. Compared exactly, "Score" lost a millionth of a pixel and was
    /// drawn as "Sco...".
    #[test]
    fn a_tab_measured_to_fit_draws_its_name_whole() {
        font::atlas::set_embedded();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let m = Metrics::default().resolved(scale);
            for size in [1.0, 1.25, 1.5, 2.0, 2.5, 3.0] {
                for name in ["Score", "Audio", "Multitrack", "Multitrack 2", "Notes"] {
                    for closes in [true, false] {
                        let w = tab_width(name, size, &m, closes);
                        let (before, after) = tab_ends(&m, closes);
                        assert!(
                            font::fits(name, w - before - after, size),
                            "{name} at {size}, scale {scale}, closes {closes}"
                        );
                    }
                }
            }
        }
    }
}
