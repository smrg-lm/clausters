//! **One choice, several presentations**: the geometry and the drawing of a
//! `choice` in each of its views.
//!
//! A combobox, a radio group, a segmented control, a row of tabs, a pager and
//! a list all hold the same thing -- some options and which one is current --
//! and differ only in the picture. So they are one element
//! ([`crate::host::elements`]'s `choice`) and this module is the pictures: for
//! each view, the **parts** it is made of ([`parts`]), which is the one
//! function the drawing and the press both read. A part is hit where it is
//! drawn because there is no second derivation of where it is.

use crate::host::font::{self, symbol};
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::size::{control_box, text_box};

use super::controls;

/// How a choice is presented.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    /// A field showing the chosen option, with a list that opens under it.
    #[default]
    Combo,
    /// Every option on a row of its own, with a mark beside the chosen one.
    Radio,
    /// The options side by side in one bar of equal segments.
    Segmented,
    /// The options as tabs: side by side, each as wide as its name.
    Tabs,
    /// Two arrows and the position between them: one step at a time.
    Pager,
    /// Every option on a row of its own, the chosen one lit.
    List,
}

impl View {
    pub(crate) fn from_str(s: &str) -> Option<View> {
        Some(match s {
            "combo" => View::Combo,
            "radio" => View::Radio,
            "segmented" => View::Segmented,
            "tabs" => View::Tabs,
            "pager" => View::Pager,
            "list" => View::List,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            View::Combo => "combo",
            View::Radio => "radio",
            View::Segmented => "segmented",
            View::Tabs => "tabs",
            View::Pager => "pager",
            View::List => "list",
        }
    }

    /// Whether the options stand on rows of their own, one under another.
    fn stacked(self) -> bool {
        matches!(self, View::Radio | View::List)
    }
}

/// One thing a hand can land on in a choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The option at this index.
    Option(usize),
    /// A combobox's field: a press opens the list.
    Field,
    /// A pager's arrows.
    Prev,
    Next,
    /// The tabs from this index on, which did not fit: a press lists them.
    More(usize),
}

/// The width one tab takes: its name with a pad and a half on either side.
fn tab_w(option: &str, size: f32, m: &Metrics) -> f32 {
    font::width(option, size) + 3.0 * m.pad
}

/// The side of a pager's arrow cell.
fn arrow_w(size: f32, m: &Metrics) -> f32 {
    font::height(size) + 2.0 * m.pad
}

/// The text a pager shows between its arrows.
pub fn pager_text(options: &[String], index: usize) -> String {
    if options.is_empty() {
        return String::new();
    }
    format!("{} / {}", index + 1, options.len())
}

/// **The parts of a choice**, each with the rectangle it is drawn in, inside
/// `body` -- the control's body, its label strip already taken off.
pub fn parts(
    view: View,
    options: &[String],
    body: Rect,
    size: f32,
    m: &Metrics,
) -> Vec<(Part, Rect)> {
    let n = options.len();
    match view {
        View::Combo => vec![(Part::Field, body)],
        View::Radio | View::List => {
            let row = row_h(size, m);
            (0..n)
                .map(|i| {
                    (
                        Part::Option(i),
                        Rect::new(body.x, body.y + i as f32 * row, body.w, row),
                    )
                })
                .collect()
        }
        View::Segmented => {
            if n == 0 {
                return Vec::new();
            }
            let w = body.w / n as f32;
            (0..n)
                .map(|i| {
                    (
                        Part::Option(i),
                        Rect::new(body.x + i as f32 * w, body.y, w, body.h),
                    )
                })
                .collect()
        }
        View::Tabs => {
            let widths: Vec<f32> = options.iter().map(|o| tab_w(o, size, m)).collect();
            let more = tab_w(&symbol::MENU.to_string(), size, m);
            let total: f32 = widths.iter().sum();
            let fit = if total <= body.w {
                n
            } else {
                let mut used = 0.0;
                widths
                    .iter()
                    .take_while(|w| {
                        used += **w;
                        used + more <= body.w
                    })
                    .count()
            };
            let mut x = body.x;
            let mut out: Vec<(Part, Rect)> = widths[..fit]
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let r = Rect::new(x, body.y, *w, body.h);
                    x += w;
                    (Part::Option(i), r)
                })
                .collect();
            if fit < n {
                let w = more.min((body.x + body.w - x).max(0.0));
                out.push((Part::More(fit), Rect::new(x, body.y, w, body.h)));
            }
            out
        }
        View::Pager => {
            let a = arrow_w(size, m).min(body.w * 0.5);
            vec![
                (Part::Prev, Rect::new(body.x, body.y, a, body.h)),
                (
                    Part::Next,
                    Rect::new(body.x + body.w - a, body.y, a, body.h),
                ),
            ]
        }
    }
}

/// The part under `(x, y)`.
pub fn part_at(parts: &[(Part, Rect)], x: f64, y: f64) -> Option<Part> {
    parts
        .iter()
        .find(|(_, r)| r.contains(x, y))
        .map(|(p, _)| *p)
}

/// The height of one row of a stacked view.
pub fn row_h(size: f32, m: &Metrics) -> f32 {
    control_box(size, m).max(m.box_side)
}

/// The height a choice's **body** wants in this view: one line of control, or
/// a row per option for the views that stack them.
///
/// The options are a prop, so a size that counts them is still a size read out
/// of props -- no value can move it.
pub fn body_h(view: View, options: usize, size: f32, m: &Metrics) -> f32 {
    if view.stacked() {
        row_h(size, m) * options.max(1) as f32
    } else {
        control_box(size, m)
    }
}

/// The width a choice's body wants when its container is fitted to it.
pub fn body_w(view: View, options: &[String], size: f32, m: &Metrics) -> f32 {
    let widest = options
        .iter()
        .map(|o| font::width(o, size))
        .fold(0.0f32, f32::max);
    match view {
        View::Combo => widest + 2.0 * m.pad + font::height(size) + m.pad,
        View::Radio => m.box_side + widest + 2.0 * m.pad,
        View::List => widest + 2.0 * m.pad,
        View::Segmented => (widest + 3.0 * m.pad) * options.len().max(1) as f32,
        View::Tabs => options.iter().map(|o| tab_w(o, size, m)).sum(),
        View::Pager => {
            2.0 * arrow_w(size, m)
                + text_box(
                    &pager_text(options, options.len().saturating_sub(1)),
                    size,
                    m,
                )
        }
    }
}

/// Draws a choice in `view` over `rect`: its label strip, then its parts.
/// `hover` is the part under the pointer, if any.
#[allow(clippy::too_many_arguments)] // one control: its data, its place, its state
pub fn draw(
    d: &mut Draw,
    view: View,
    options: &[String],
    index: usize,
    label: Option<&str>,
    rect: Rect,
    size: f32,
    hover: Option<Part>,
) {
    if view == View::Combo {
        let current = options.get(index).map_or("", String::as_str);
        return controls::choice(d, current, label, rect, size);
    }
    controls::label_strip(d, label, rect, size);
    let body = controls::body_rect_at(rect, label.is_some(), size, d.m);
    let parts = parts(view, options, body, size, d.m);
    let (mesh, m, theme) = d.parts();
    let glyph = font::height(size);
    let ty = |r: Rect| r.y + (r.h - glyph) * 0.5;
    match view {
        View::Combo => {}
        View::Radio => {
            for (part, r) in &parts {
                let Part::Option(i) = *part else { continue };
                let side = m.box_side.min(r.h);
                let (cx, cy) = (r.x + side * 0.5, r.y + r.h * 0.5);
                mesh.disc(cx, cy, side * 0.5, theme.track);
                if i == index {
                    mesh.disc(cx, cy, side * 0.26, theme.accent);
                } else if hover == Some(*part) {
                    mesh.disc(cx, cy, side * 0.26, theme.hover);
                }
                font::text_ellipsis(
                    mesh,
                    &options[i],
                    r.x + side + m.pad,
                    ty(*r),
                    (r.w - side - m.pad).max(0.0),
                    size,
                    theme.text,
                );
            }
        }
        View::List => {
            mesh.rect(body, theme.field);
            for (part, r) in &parts {
                let Part::Option(i) = *part else { continue };
                if i == index {
                    mesh.rect(*r, theme.accent_dim);
                } else if hover == Some(*part) {
                    mesh.rect(*r, theme.hover);
                }
                font::text_ellipsis(
                    mesh,
                    &options[i],
                    r.x + m.pad,
                    ty(*r),
                    (r.w - 2.0 * m.pad).max(0.0),
                    size,
                    theme.text,
                );
            }
        }
        View::Segmented => {
            mesh.rect(body, theme.field);
            for (part, r) in &parts {
                let Part::Option(i) = *part else { continue };
                if i == index {
                    mesh.rect(*r, theme.accent_dim);
                } else if hover == Some(*part) {
                    mesh.rect(*r, theme.hover);
                }
                if i > 0 {
                    mesh.rect(Rect::new(r.x, r.y, m.divider_w, r.h), theme.separator);
                }
                font::text_ellipsis(
                    mesh,
                    &options[i],
                    (r.x + (r.w - font::width(&options[i], size)) * 0.5).max(r.x + m.pad),
                    ty(*r),
                    (r.w - 2.0 * m.pad).max(0.0),
                    size,
                    theme.text,
                );
            }
            mesh.border(body, m.divider_w, theme.separator);
        }
        View::Tabs => {
            // The line every tab stands on, and the chosen one standing out of
            // it: its ground is the field's and its foot is the accent.
            let foot = (2.0 * m.divider_w).max(2.0);
            mesh.rect(
                Rect::new(body.x, body.y + body.h - m.divider_w, body.w, m.divider_w),
                theme.separator,
            );
            for (part, r) in &parts {
                let (text, chosen) = match *part {
                    Part::Option(i) => (options[i].clone(), i == index),
                    // The gathered tabs light when the chosen one is among them.
                    Part::More(from) => (symbol::MENU.to_string(), index >= from),
                    _ => continue,
                };
                if chosen {
                    mesh.rect(*r, theme.field);
                    mesh.rect(Rect::new(r.x, r.y + r.h - foot, r.w, foot), theme.accent);
                } else if hover == Some(*part) {
                    mesh.rect(*r, theme.hover);
                }
                font::text_centered(
                    mesh,
                    &text,
                    *r,
                    size,
                    if chosen { theme.text } else { theme.text_dim },
                );
            }
        }
        View::Pager => {
            let last = options.len().saturating_sub(1);
            for (part, r) in &parts {
                let (glyph_of, live) = match part {
                    Part::Prev => (symbol::POINT_LEFT, index > 0),
                    Part::Next => (symbol::POINT_RIGHT, index < last),
                    _ => continue,
                };
                mesh.rect(
                    *r,
                    if live && hover == Some(*part) {
                        theme.hover
                    } else {
                        theme.field
                    },
                );
                font::text_centered(
                    mesh,
                    &glyph_of.to_string(),
                    *r,
                    size,
                    if live {
                        theme.accent
                    } else {
                        theme.text_disabled
                    },
                );
            }
            font::text_centered(mesh, &pager_text(options, index), body, size, theme.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn body() -> Rect {
        Rect::new(10.0, 20.0, 300.0, 24.0)
    }

    #[test]
    fn the_view_names_round_trip() {
        for name in ["combo", "radio", "segmented", "tabs", "pager", "list"] {
            assert_eq!(View::from_str(name).unwrap().name(), name);
        }
        assert_eq!(View::from_str("carousel"), None);
    }

    #[test]
    fn a_segmented_control_splits_its_body_evenly_and_covers_it() {
        let m = Metrics::default();
        let p = parts(View::Segmented, &options(&["a", "b", "c"]), body(), 2.0, &m);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].1.w, 100.0);
        assert_eq!(part_at(&p, 15.0, 30.0), Some(Part::Option(0)));
        assert_eq!(part_at(&p, 305.0, 30.0), Some(Part::Option(2)));
    }

    #[test]
    fn a_radio_group_is_a_row_per_option() {
        let m = Metrics::default();
        let opts = options(&["one", "two", "three"]);
        let tall = Rect::new(0.0, 0.0, 200.0, body_h(View::Radio, 3, 2.0, &m));
        let p = parts(View::Radio, &opts, tall, 2.0, &m);
        assert_eq!(p.len(), 3);
        assert!(p.windows(2).all(|w| w[0].1.y + w[0].1.h == w[1].1.y));
        assert_eq!(
            p[2].1.y + p[2].1.h,
            tall.h,
            "the rows fill the body it asked for"
        );
        assert_eq!(
            body_h(View::Radio, 3, 2.0, &m),
            body_h(View::List, 3, 2.0, &m)
        );
        assert!(body_h(View::Radio, 3, 2.0, &m) > body_h(View::Tabs, 3, 2.0, &m));
    }

    #[test]
    fn tabs_are_as_wide_as_their_names_and_the_rest_gather_when_they_do_not_fit() {
        let m = Metrics::default();
        let opts = options(&["one", "a longer name", "three", "four"]);
        let roomy = parts(
            View::Tabs,
            &opts,
            Rect::new(0.0, 0.0, 2000.0, 24.0),
            2.0,
            &m,
        );
        assert_eq!(roomy.len(), 4);
        assert!(roomy[1].1.w > roomy[0].1.w, "a tab follows its own name");
        // Room for two tabs and the gathering one.
        let w = roomy[1].1.x + roomy[1].1.w + roomy[2].1.w * 0.8;
        let tight = parts(View::Tabs, &opts, Rect::new(0.0, 0.0, w, 24.0), 2.0, &m);
        assert_eq!(tight.last().unwrap().0, Part::More(2));
        assert!(tight.iter().all(|(_, r)| r.x + r.w <= w + 0.01));
    }

    #[test]
    fn a_pager_is_two_arrows_at_the_ends_of_its_body() {
        let m = Metrics::default();
        let p = parts(View::Pager, &options(&["a", "b"]), body(), 2.0, &m);
        assert_eq!(p[0].0, Part::Prev);
        assert_eq!(p[1].0, Part::Next);
        assert_eq!(p[0].1.x, 10.0);
        assert_eq!(p[1].1.x + p[1].1.w, 310.0);
        assert_eq!(pager_text(&options(&["a", "b", "c"]), 1), "2 / 3");
    }
}
