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
//! the same cursor by construction. A shape the list has no name for (the
//! roll's pencil, eraser, scissors and stretch, each the icon of its tool on
//! the toolbar) is drawn from the host's icon of it and set
//! as a picture ([`Cursors`]), on both fronts too; where a platform takes
//! no picture, its named shape stands in ([`Pointer::named`]).

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
    /// The eraser: a press takes away what it lands on. Drawn from the
    /// host's icon, the arrow where it cannot be.
    Erase,
    /// The scissors: a press cuts what it lands on there. Drawn from the
    /// host's icon, the crosshair where it cannot be.
    Cut,
    /// The pencil: a press draws a note where it lands. Drawn from the
    /// host's icon, the crosshair where it cannot be.
    Draw,
    /// The stretch: a drag stretches what it takes in time. Drawn from the
    /// host's icon, the sideways resize where it cannot be.
    Stretch,
}

/// The shapes drawn from an icon: the icon's name, and where in its box of
/// 24 the point a press lands on is.
const PICTURED: &[(Pointer, &str, (f32, f32))] = &[
    // the corner the eraser rubs with
    (Pointer::Erase, "eraser", (5.0, 19.0)),
    // where the blades cross
    (Pointer::Cut, "scissors", (12.0, 12.0)),
    // the pencil's point
    (Pointer::Draw, "pencil", (2.0, 22.0)),
    // the line the arrows leave
    (Pointer::Stretch, "unfold-horizontal", (12.0, 12.0)),
];

impl Pointer {
    /// **The named shape it is, or stands in for**: itself, or for a shape
    /// drawn from an icon the named one a platform shows in its place.
    pub fn named(self) -> Pointer {
        match self {
            Pointer::Erase => Pointer::Default,
            Pointer::Cut | Pointer::Draw => Pointer::Crosshair,
            Pointer::Stretch => Pointer::EwResize,
            p => p,
        }
    }

    /// Its CSS name, which is the list both fronts name it by (its named
    /// shape's, for one drawn from an icon).
    pub fn css(self) -> &'static str {
        match self.named() {
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
            Pointer::Erase | Pointer::Cut | Pointer::Draw | Pointer::Stretch => {
                unreachable!("a named shape")
            }
        }
    }

    /// **The windowing crate's cursor for it**, the one both fronts set:
    /// the platform's theme natively, the canvas's CSS `cursor` in a page
    /// (which the crate writes by the same name as [`Self::css`]).
    pub fn icon(self) -> winit::window::CursorIcon {
        use winit::window::CursorIcon;
        match self.named() {
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
            Pointer::Erase | Pointer::Cut | Pointer::Draw | Pointer::Stretch => {
                unreachable!("a named shape")
            }
        }
    }
}

/// **The shapes drawn from an icon, made for a window's size**: the
/// platform's picture of each, made once per size from the event loop (the
/// one place a platform makes one), and the cursor a front sets.
#[derive(Default)]
pub struct Cursors {
    made: std::collections::HashMap<(Pointer, u32), winit::window::CustomCursor>,
}

/// How large a pointer is drawn, in a window's logical pixels: the size of a
/// platform's own.
pub const POINTER_PX: f32 = 24.0;

/// How many pixels square a pointer is drawn at a window's `scale`.
pub fn pointer_px(scale: f32) -> u32 {
    (POINTER_PX * scale).round().max(1.0) as u32
}

impl Cursors {
    /// Makes the pictured shapes at `px` pixels square, the ones not made
    /// yet.
    pub fn make(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, px: u32) {
        for &(p, name, (hx, hy)) in PICTURED {
            if self.made.contains_key(&(p, px)) {
                continue;
            }
            let Some(rgba) = clausters_gui_draw::font::outline::icon_pixels(name, px) else {
                continue;
            };
            // the icon's box inside a pixel of margin ([`icon_pixels`])
            let k = (px as f32 - 2.0).max(1.0) / 24.0;
            let hot = |v: f32| ((1.0 + v * k).round() as u16).min(px as u16 - 1);
            let Ok(source) = winit::window::CustomCursor::from_rgba(
                rgba,
                px as u16,
                px as u16,
                hot(hx),
                hot(hy),
            ) else {
                continue;
            };
            self.made
                .insert((p, px), event_loop.create_custom_cursor(source));
        }
    }

    /// **The cursor for `p`** at `px` pixels: its picture when it is drawn
    /// from an icon and was made, else its named shape.
    pub fn cursor(&self, p: Pointer, px: u32) -> winit::window::Cursor {
        match self.made.get(&(p, px)) {
            Some(c) => c.clone().into(),
            None => p.icon().into(),
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
            Pointer::Erase,
            Pointer::Cut,
            Pointer::Draw,
            Pointer::Stretch,
        ] {
            assert_eq!(p.icon().name(), p.css(), "{p:?}");
        }
    }

    /// **A shape drawn from an icon is a picture of it**: black lines on a
    /// white halo, clear around them, for every shape the list pictures.
    /// Stroking an icon is the `notation` feature's (the drawing base's
    /// `path`); a build without it shows each shape's named stand-in.
    #[cfg(feature = "notation")]
    #[test]
    fn a_pictured_shape_is_its_icon_in_black_on_white() {
        for &(_, name, _) in PICTURED {
            let px = clausters_gui_draw::font::outline::icon_pixels(name, 24).expect(name);
            assert_eq!(px.len(), 24 * 24 * 4);
            let texels: Vec<&[u8; 4]> = px.as_chunks::<4>().0.iter().collect();
            assert_eq!(texels[0][3], 0, "{name}: clear in its corner");
            assert!(
                texels.iter().any(|t| **t == [0, 0, 0, 255]),
                "{name}: its lines"
            );
            assert!(
                texels.iter().any(|t| **t == [255, 255, 255, 255]),
                "{name}: its halo"
            );
        }
    }
}
