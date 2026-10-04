//! **The menu bar**: a window's `menu`, drawn by the host as a band along the
//! top edge.
//!
//! It is chrome, exactly as the status bar along the bottom is
//! ([`super::status`]): not a widget, not in the tree, and taken out of the
//! area the tree is laid out in -- by one function ([`bar`]) that the renderer
//! and the hit test both call, so a title is pressed on the pixels it was
//! drawn on and nothing under the band is.
//!
//! The bar holds the **titles** of the menu's first entries; a press on one
//! opens its list under it in the popup layer ([`super::popup`]). Titles that
//! do not fit the window gather under a last one, whose list holds them as
//! submenus -- so a narrow window loses no entry.

use super::font::{self, symbol};
use super::layout::Rect;
use super::menu::{Entry, EntryKind};
use super::metrics::Metrics;
use super::popup;
use super::widget::{Widget, WidgetKind};

/// The window's menu, when it carries one with anything in it.
pub fn entries(tree: &Widget) -> Option<&[Entry]> {
    match tree.kind {
        WidgetKind::Window { .. } => tree.menu.as_deref().filter(|m| !m.is_empty()),
        _ => None,
    }
}

/// The height the bar costs a window -- zero when it has none.
pub fn bar_h(tree: &Widget, m: &Metrics) -> f32 {
    if entries(tree).is_some() {
        popup::row_h(m.text_scale, m)
    } else {
        0.0
    }
}

/// The band the bar occupies inside `area`, or `None` for a window with no
/// menu.
pub fn bar(tree: &Widget, area: Rect, m: &Metrics) -> Option<Rect> {
    let h = bar_h(tree, m).min(area.h);
    (h > 0.0).then(|| Rect::new(area.x, area.y, area.w, h))
}

/// `area` with the bar's band taken off its top -- where the tree is laid out.
pub fn content(tree: &Widget, area: Rect, m: &Metrics) -> Rect {
    match bar(tree, area, m) {
        Some(band) => Rect::new(area.x, area.y + band.h, area.w, (area.h - band.h).max(0.0)),
        None => area,
    }
}

/// One title on the bar: which of the menu's first entries it stands for, or
/// the gathered remainder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Title {
    /// The entry at this index of the menu.
    Entry(usize),
    /// The titles from this index on, which did not fit: one title whose list
    /// holds them.
    More(usize),
}

/// The titles as they stand on the bar, left to right, each with its
/// rectangle.
pub fn titles(entries: &[Entry], band: Rect, m: &Metrics) -> Vec<(Title, Rect)> {
    let size = m.text_scale;
    let width = |label: &str| font::width(label, size) + 4.0 * m.pad;
    let more_w = width(&symbol::MENU.to_string());
    let widths: Vec<f32> = entries.iter().map(|e| width(&e.label)).collect();
    // How many fit: all of them, or as many as leave room for the title that
    // holds the rest.
    let total: f32 = widths.iter().sum();
    let fit = if total <= band.w {
        entries.len()
    } else {
        let mut used = 0.0;
        widths
            .iter()
            .take_while(|w| {
                used += **w;
                used + more_w <= band.w
            })
            .count()
    };
    let mut x = band.x;
    let mut out: Vec<(Title, Rect)> = widths[..fit]
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let r = Rect::new(x, band.y, *w, band.h);
            x += w;
            (Title::Entry(i), r)
        })
        .collect();
    if fit < entries.len() {
        out.push((
            Title::More(fit),
            Rect::new(
                x,
                band.y,
                more_w.min((band.x + band.w - x).max(0.0)),
                band.h,
            ),
        ));
    }
    out
}

/// The text a title is drawn with.
pub fn label(entries: &[Entry], title: Title) -> String {
    match title {
        Title::Entry(i) => entries[i].label.clone(),
        Title::More(_) => symbol::MENU.to_string(),
    }
}

/// The list a title opens: the entry's submenu, or -- for the gathered
/// remainder -- the entries that did not fit, each still opening its own.
///
/// `None` for a title that is a plain entry: a press on it is a pick, with no
/// list to open.
pub fn list(entries: &[Entry], title: Title) -> Option<Vec<Entry>> {
    match title {
        Title::Entry(i) => entries.get(i)?.submenu().map(<[Entry]>::to_vec),
        Title::More(from) => Some(entries.get(from..)?.to_vec()),
    }
}

/// The path, from the menu's root, of a row picked in a title's list.
pub fn path(title: Title, in_list: &[usize]) -> Vec<usize> {
    match title {
        Title::Entry(i) => std::iter::once(i).chain(in_list.iter().copied()).collect(),
        Title::More(from) => match in_list.split_first() {
            Some((first, tail)) => std::iter::once(from + first)
                .chain(tail.iter().copied())
                .collect(),
            None => Vec::new(),
        },
    }
}

/// Whether a title can be pressed: an enabled entry that opens a list or names
/// a verb.
pub fn live(entries: &[Entry], title: Title) -> bool {
    match title {
        Title::Entry(i) => entries
            .get(i)
            .is_some_and(|e| e.enabled && !matches!(e.kind, EntryKind::Separator)),
        Title::More(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::menu;
    use serde_json::json;

    fn bar_menu() -> Vec<Entry> {
        menu::parse(&json!([
            {"label": "File", "menu": ["New", "Open"]},
            {"label": "Edit", "menu": ["Undo", "Redo"]},
            {"label": "View", "menu": ["Zoom"]},
            {"label": "Help", "verb": "help"},
        ]))
    }

    #[test]
    fn the_titles_stand_left_to_right_inside_the_band() {
        let m = Metrics::default();
        let band = Rect::new(0.0, 0.0, 800.0, 24.0);
        let t = titles(&bar_menu(), band, &m);
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].0, Title::Entry(0));
        assert!(t.windows(2).all(|w| w[0].1.x + w[0].1.w == w[1].1.x));
    }

    #[test]
    fn titles_that_do_not_fit_gather_under_a_last_one() {
        let m = Metrics::default();
        let all = titles(&bar_menu(), Rect::new(0.0, 0.0, 800.0, 24.0), &m);
        // Room for two titles and the gathering one, not for three.
        let w = all[1].1.x + all[1].1.w + all[2].1.w * 0.9;
        let t = titles(&bar_menu(), Rect::new(0.0, 0.0, w, 24.0), &m);
        assert_eq!(t.last().unwrap().0, Title::More(2));
        assert!(t.iter().all(|(_, r)| r.x + r.w <= w + 0.01));
        // Its list holds what did not fit, and a pick there maps to the root.
        let rest = list(&bar_menu(), Title::More(2)).unwrap();
        assert_eq!(rest.len(), 2);
        assert_eq!(path(Title::More(2), &[0, 0]), vec![2, 0]);
        assert_eq!(path(Title::Entry(1), &[1]), vec![1, 1]);
    }

    #[test]
    fn a_title_that_is_a_plain_entry_opens_no_list() {
        assert_eq!(list(&bar_menu(), Title::Entry(3)), None);
        assert!(live(&bar_menu(), Title::Entry(3)));
    }
}
