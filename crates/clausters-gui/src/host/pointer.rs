//! **The pointer's shape: what a press here would do, said before it.**
//!
//! A desktop application says what a press will start before the hand
//! presses: an I-beam over text a press puts a caret in, a hand over a link,
//! a resize arrow over an edge a drag pulls. The question is the hit test's
//! -- what lies under the pointer, and what a press there would start -- so
//! it is asked where the press is resolved (`gestures::pointer`): the dock's
//! chrome over its own parts ([`crate::host::dock::Dock::pointer_shape`]),
//! a container's plan over its axis, and an element over the parts its own
//! gestures tell apart ([`Element::pointer`](crate::host::widget::element::Element::pointer)).
//!
//! The front only sets what it is told, on its window: natively the
//! platform's cursor theme, in a page the canvas's CSS `cursor`. Both are
//! named by the one list below, by the CSS names, so the two fronts ask for
//! the same cursor by construction, and the core names no windowing type.

/// **A pointer shape**, by its CSS name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pointer {
    /// The platform's arrow: a press here is a click, or nothing.
    #[default]
    Default,
    /// The hand (`pointer`): a link a press follows. Desktop buttons keep
    /// the arrow.
    Hand,
    /// The I-beam (`text`): a press puts a caret here, or sweeps a
    /// selection over what is under it.
    Text,
    /// Precise placement (`crosshair`): a press that draws -- a note made
    /// where it lands, a point added, a pencil's stroke.
    Crosshair,
    /// A thing a press takes and a drag carries (`grab`): a clip, a note.
    Grab,
    /// The thing taken (`grabbing`), while the drag carries it, and a view
    /// panned under the hand.
    Grabbing,
    /// A thing a drag moves in both directions (`move`): a break-point.
    Move,
    /// A divider between columns, dragged sideways (`col-resize`).
    ColResize,
    /// A divider between rows, dragged up and down (`row-resize`).
    RowResize,
    /// An edge dragged sideways (`ew-resize`): a clip's or a note's ends.
    EwResize,
    /// An edge or a value dragged up and down (`ns-resize`): a velocity,
    /// a segment's bend.
    NsResize,
}

impl Pointer {
    /// Its CSS name, which is the list both fronts name it by.
    pub fn css(self) -> &'static str {
        match self {
            Pointer::Default => "default",
            Pointer::Hand => "pointer",
            Pointer::Text => "text",
            Pointer::Crosshair => "crosshair",
            Pointer::Grab => "grab",
            Pointer::Grabbing => "grabbing",
            Pointer::Move => "move",
            Pointer::ColResize => "col-resize",
            Pointer::RowResize => "row-resize",
            Pointer::EwResize => "ew-resize",
            Pointer::NsResize => "ns-resize",
        }
    }

    /// **The windowing crate's cursor for it**, the one both fronts set:
    /// the platform's theme natively, the canvas's CSS `cursor` in a page
    /// (which the crate writes by the same name as [`Self::css`]).
    pub fn icon(self) -> winit::window::CursorIcon {
        use winit::window::CursorIcon;
        match self {
            Pointer::Default => CursorIcon::Default,
            Pointer::Hand => CursorIcon::Pointer,
            Pointer::Text => CursorIcon::Text,
            Pointer::Crosshair => CursorIcon::Crosshair,
            Pointer::Grab => CursorIcon::Grab,
            Pointer::Grabbing => CursorIcon::Grabbing,
            Pointer::Move => CursorIcon::Move,
            Pointer::ColResize => CursorIcon::ColResize,
            Pointer::RowResize => CursorIcon::RowResize,
            Pointer::EwResize => CursorIcon::EwResize,
            Pointer::NsResize => CursorIcon::NsResize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_shape_is_the_cursor_of_its_css_name() {
        for p in [
            Pointer::Default,
            Pointer::Hand,
            Pointer::Text,
            Pointer::Crosshair,
            Pointer::Grab,
            Pointer::Grabbing,
            Pointer::Move,
            Pointer::ColResize,
            Pointer::RowResize,
            Pointer::EwResize,
            Pointer::NsResize,
        ] {
            assert_eq!(p.icon().name(), p.css(), "{p:?}");
        }
    }
}
