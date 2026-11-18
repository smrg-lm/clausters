//! **A window's header**: the one strip along a window's top that holds its
//! tools, which in a dock is its panel's bar.
//!
//! A panel has one header and never two: an editor's panel is its tabs, a
//! tool's panel its tools, both on one strip with the grip at its start and
//! the panel's buttons at its end. An element that carries a strip of tools
//! of its own (the documentation's way back and where the page is, the post
//! window's clear and autoscroll: [`Element::strip`]) draws it along its own
//! top -- unless its window names it as its **header** (`header`), and then
//! the strip is drawn in a band along the window's top instead, the element
//! laid out under it. In a dock that band is under the panel's bar
//! ([`crate::host::dock::Dock::band`]): the dock draws the grip, the tabs if
//! any and its buttons, and leaves the band's middle -- the **slot** -- to
//! the element. Outside a dock the band is the window's own, the panel
//! header's height, and the slot is all of it.
//!
//! [`Element::strip`]: crate::host::widget::Element::strip

use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

use super::dock::Band;
use super::widget::{Widget, WidgetKind};

/// Where a window's header stands, in its own pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    /// The element whose strip it is.
    pub id: i32,
    /// The whole band along the window's top.
    pub band: Rect,
    /// The band's middle, which the element draws its strip in.
    pub slot: Rect,
    /// Whether the band is the window's own rather than under a dock's bar,
    /// which draws the line under it.
    pub own: bool,
}

/// **The element window `tree` names as its header**, when it names one
/// that carries a strip.
pub fn element(tree: &Widget) -> Option<i32> {
    let WidgetKind::Window {
        header: Some(id), ..
    } = tree.kind
    else {
        return None;
    };
    tree.find(id)?
        .kind
        .as_element()
        .is_some_and(|el| el.strip())
        .then_some(id)
}

/// **Where window `tree`'s header goes in `area`** (its framebuffer): along
/// the top, as tall as `band` says when a dock measured it, else the panel
/// header's height, its slot between the room `band` keeps at each end.
pub fn place(tree: &Widget, band: Option<Band>, area: Rect, m: &Metrics) -> Option<Place> {
    let id = element(tree)?;
    let own = band.is_none();
    let band = band.unwrap_or(Band {
        h: m.panel_h,
        left: 0.0,
        right: 0.0,
    });
    let h = band.h.min(area.h).max(0.0);
    let left = band.left.clamp(0.0, area.w);
    let right = band.right.clamp(0.0, area.w - left);
    Some(Place {
        id,
        band: Rect::new(area.x, area.y, area.w, h),
        slot: Rect::new(area.x + left, area.y, area.w - left - right, h),
        own,
    })
}

/// `area` with the header's band taken off its top.
pub fn content(place: Option<&Place>, area: Rect) -> Rect {
    match place {
        Some(p) => Rect::new(
            area.x,
            area.y + p.band.h,
            area.w,
            (area.h - p.band.h).max(0.0),
        ),
        None => area,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::guidef::GuiNode;

    fn window(json: &str) -> Widget {
        let node = GuiNode::parse(json.as_bytes()).expect("a def");
        Widget::from_node(1, &node, &[]).expect("a tree")
    }

    /// **A window names an element whose strip is its header**: the band is
    /// the panel header's height along its top, all of it the slot outside a
    /// dock, and the slot between the room a dock's bar keeps inside one; a
    /// window naming nothing, or an element with no strip, has none.
    #[test]
    fn a_header_is_a_band_along_the_top_with_a_slot() {
        let m = Metrics::default();
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let tree = window(
            r#"{"type":"window","header":5,"children":[
                {"id":5,"type":"post","tools":1}]}"#,
        );
        let own = place(&tree, None, area, &m).expect("a header");
        assert_eq!(own.id, 5);
        assert_eq!(own.band, Rect::new(0.0, 0.0, 400.0, m.panel_h));
        assert_eq!(own.slot, own.band);
        assert_eq!(
            content(Some(&own), area),
            Rect::new(0.0, m.panel_h, 400.0, 300.0 - m.panel_h)
        );
        let docked = Band {
            h: 30.0,
            left: 20.0,
            right: 50.0,
        };
        let in_dock = place(&tree, Some(docked), area, &m).expect("a header");
        assert_eq!(in_dock.slot, Rect::new(20.0, 0.0, 330.0, 30.0));
        let plain = window(r#"{"type":"window","header":5,"children":[{"id":5,"type":"post"}]}"#);
        assert!(
            place(&plain, None, area, &m).is_none(),
            "a post with no tools"
        );
        let none = window(r#"{"type":"window","children":[{"id":5,"type":"post"}]}"#);
        assert!(place(&none, None, area, &m).is_none());
    }
}
