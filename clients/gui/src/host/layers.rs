//! **Edit layers**: which of a container's layered contents a hand is editing.
//!
//! A container that layers elements draws several things on one rectangle, and
//! each of them is editable in its own vocabulary: the container's *placement*
//! (where it sits, how long it is), the material under it, the events over
//! that, an automation over both. They share every pixel, so a press is claimed
//! by four things at once, and the order used to be decided ad hoc in three
//! different places.
//!
//! **This is not a clip's idea.** A clip is the first container here that
//! layers editable contents and it is deliberately not the last: an audio
//! editor's view is the same picture — material, a selection over it, an
//! automation over both, later a spectral layer — and a container that grows
//! contents grows layers with them. So the active layer is a field of the
//! [`Widget`] node, the stack is read off whatever children fill a role, and
//! nothing in this module names a widget type.
//!
//! The rule is one sentence: **one layer is active at a time, and it is the
//! only one that acts or offers an affordance.** A curve being edited shows its
//! points and its lit segment and the clip shows no grips; the clip being moved
//! shows its grips and the curve is a picture. Nothing else changes when a new
//! kind of layer arrives, which is the point of stating it here rather than in
//! each pass.
//!
//! **The stack is general and the elements in it are of any nature.** A layer
//! is addressed by its position in the container's own declaration order
//! (back to front), never by a fixed list of kinds: [`stack`] reads the
//! children that fill a [`BodyRole`], so a container that grows a fourth kind
//! of content grows a fourth layer with no change here. What an element
//! contributes is only two answers — the role it fills
//! ([`Element::body_role`](crate::host::widget::Element::body_role)) and
//! whether the pointer is on its own material
//! ([`Element::layer_hit`](crate::host::widget::Element::layer_hit)).
//!
//! **Selection is not the mouse.** [`Selection`] is the whole of what selecting
//! a layer is, and the pointer rule ([`under_pointer`]) is one caller of it;
//! `/gui_set layer` is another, and a key binding or a menu would be a third.
//! The wire name a script uses is the role's ([`Layer::name`]), resolved
//! against the container that actually holds the layers, so a script names a
//! layer by what it is rather than by a number.

use crate::host::graphics::track;
use crate::host::layout::Rect;
use crate::host::widget::element::{BodyRole, Input, TimeSpace};
use crate::host::widget::{Widget, WidgetKind};

/// The **active edit layer** of a container: what a press on it means.
///
/// [`Placement`](Layer::Placement) is the container itself — the rectangle's
/// move and its edges. [`Content`](Layer::Content) is the nth element layered
/// inside it, counted in the container's own declaration order (the order they
/// are drawn in, back to front), which is what keeps the address independent of
/// what kinds of element the container turns out to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layer {
    /// The container's own placement: where it sits and how long it is.
    #[default]
    Placement,
    /// The nth layered content element.
    Content(usize),
}

/// The wire name of a container's own placement layer.
pub const PLACEMENT: &str = "placement";

/// What a **clip**'s placement layer is also called, since a clip is what a
/// script means by the thing being placed. One alias, accepted on the way in
/// and never produced on the way out, so a query answers one word.
const PLACEMENT_ALIAS: &str = "clip";

impl Layer {
    /// The wire name of this layer inside `widget`: `"placement"` for the
    /// container itself (a `clip` also answers to `"clip"` on the way in),
    /// otherwise the role the element fills (`"take"`, `"notes"`, `"points"`),
    /// suffixed `":n"` when the container carries more than one layer of that
    /// role — a clip with two automations names them `points` and `points:1`.
    ///
    /// A stale address (a layer index the container no longer has) names the
    /// placement, which is what [`Selection::layer`] resolves it to as well.
    pub fn name(&self, widget: &Widget) -> String {
        let Layer::Content(n) = *self else {
            return PLACEMENT.into();
        };
        let stack = stack(widget);
        let Some(role) = stack.get(n).copied() else {
            return PLACEMENT.into();
        };
        let ordinal = stack[..n].iter().filter(|r| **r == role).count();
        match ordinal {
            0 => role_name(role).into(),
            k => format!("{}:{k}", role_name(role)),
        }
    }
}

/// The wire name of a body role — the same word the prop that fills it is
/// called by, so a script names the layer it already knows how to write.
pub fn role_name(role: BodyRole) -> &'static str {
    match role {
        BodyRole::Take => "take",
        BodyRole::Notes => "notes",
        BodyRole::Curve => "points",
    }
}

/// The **layer stack** of a container: the role of each layered content
/// element, back to front, in the container's own declaration order.
///
/// It is derived from the children rather than declared, so it is exactly what
/// is drawn — a container that grew a body through a `/gui_set` has grown a
/// layer, and one whose body was never built has not.
pub fn stack(widget: &Widget) -> Vec<BodyRole> {
    content(widget)
        .filter_map(|(_, c)| c.kind.body_role())
        .collect()
}

/// Selecting a layer, as an operation of its own — the door every caller goes
/// through, whatever put it in motion (a press, a `/gui_set`, a shortcut).
///
/// It is a thin type rather than three free functions because the two halves
/// belong together: resolving a *name* against the container that holds the
/// layers, and writing the choice where the passes read it. A caller that has
/// neither (a test, a browser front) still reaches both.
pub struct Selection<'a> {
    widget: &'a mut Widget,
}

impl<'a> Selection<'a> {
    /// The selection over `widget`, or `None` when it **layers nothing** — a
    /// leaf, or a container whose children are laid out beside each other
    /// rather than over each other. The test is the stack itself, so a
    /// container qualifies by holding layered contents and not by being of any
    /// particular type; a `clip` qualifies even while empty, because its
    /// placement is a layer a script may name before its bodies arrive.
    pub fn of(widget: &'a mut Widget) -> Option<Self> {
        (matches!(widget.kind, WidgetKind::Clip { .. }) || !stack(widget).is_empty())
            .then_some(Self { widget })
    }

    /// The layer a **wire name** means on this container, or `None` when it
    /// names none — an unknown word is refused rather than silently taken as
    /// the placement, so a typo in a `/gui_set` fails the way every other bad
    /// value does.
    pub fn parse(&self, name: &str) -> Option<Layer> {
        if name.is_empty() || name == PLACEMENT || name == PLACEMENT_ALIAS {
            return Some(Layer::Placement);
        }
        let (role, ordinal) = match name.split_once(':') {
            Some((role, n)) => (role, n.parse::<usize>().ok()?),
            None => (name, 0),
        };
        let stack = stack(self.widget);
        stack
            .iter()
            .enumerate()
            .filter(|(_, r)| role_name(**r) == role)
            .nth(ordinal)
            .map(|(n, _)| Layer::Content(n))
    }

    /// The layer that is active now, **resolved against the stack as it
    /// stands**: a content layer whose element is gone (a body freed, a def
    /// redrawn shorter) reads as the placement rather than as a layer nobody
    /// can see.
    pub fn layer(&self) -> Layer {
        active(self.widget)
    }

    /// **Which layers are drawn**, named the same way the active one is: every
    /// layer in `names` is hidden and every other one is shown, so one
    /// statement is the whole picture rather than a pile of toggles whose order
    /// matters. An empty list shows everything, which is the default a clip is
    /// built with.
    ///
    /// A name this container has no layer for is ignored rather than refused:
    /// the list is a description of a stack that may still be growing (a body
    /// arrives with a later `/gui_set`), and the layer it names will be hidden
    /// the moment it exists.
    pub fn set_hidden(&mut self, names: &str) -> bool {
        let wanted: Vec<Layer> = names
            .split_whitespace()
            .filter_map(|n| self.parse(n))
            .collect();
        let mut changed = false;
        let indices: Vec<usize> = content(self.widget).map(|(index, _)| index).collect();
        for (n, index) in indices.into_iter().enumerate() {
            let visible = !wanted.contains(&Layer::Content(n));
            changed |= self.widget.children[index].visible != visible;
            self.widget.children[index].visible = visible;
        }
        // A hidden layer is not the one being edited (see `active`), so the
        // read settles itself; nothing to unwind here.
        changed
    }

    /// The hidden layers' names, space-separated — what a query answers and
    /// what [`set_hidden`](Self::set_hidden) would take back.
    pub fn hidden(&self) -> String {
        content(self.widget)
            .enumerate()
            .filter(|(_, (_, child))| !child.visible)
            .map(|(n, _)| Layer::Content(n).name(self.widget))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Makes `layer` the active one, answering whether that **changed**
    /// anything — the caller emits the `"layer"` payload on a change and stays
    /// quiet otherwise, so pressing twice on the same curve says it once.
    pub fn set(&mut self, layer: Layer) -> bool {
        let changed = self.widget.layer != layer;
        self.widget.layer = layer;
        changed
    }
}

/// **The rectangle and the axis one layer stands on**, given the container's.
///
/// A layer that names no stretch fills the container, which is what a layered
/// body has always been. One that names a stretch (`Widget::span`) is placed on
/// it exactly as a clip is placed on its lane — the same mapping, one level
/// down — and reads its own window onto its own material, which is what lets a
/// clip hold three segments of three different files and still be one clip.
///
/// The **one** derivation, so the layout, the drawing, the hit-test and the
/// press cannot disagree about where a layer is.
pub fn layer_input<'a>(container: &Input<'a>, child: &Widget) -> Input<'a> {
    let Some(time) = container.time else {
        return *container;
    };
    let window = child.window.or(Some(time.window));
    let Some((at, len)) = child.span else {
        return Input {
            time: Some(TimeSpace {
                window: window.unwrap_or_default(),
                ..time
            }),
            ..*container
        };
    };
    let rect = match track::clip_x_range(
        container.rect,
        &time.view,
        at,
        len,
        container.metrics.divider_w,
    ) {
        Some((x0, x1)) => Rect::new(x0, container.rect.y, x1 - x0, container.rect.h),
        None => Rect::new(container.rect.x, container.rect.y, 0.0, 0.0),
    };
    let local = track::clip_local_view(container.rect, &time.view, at, len, rect);
    Input {
        rect,
        time: Some(TimeSpace {
            view: local,
            span: len,
            window: window.unwrap_or_default(),
            ..time
        }),
        ..*container
    }
}

/// The layer `widget` is being edited on, resolved against the stack as it
/// stands — the read every pass makes, and the one a caller with no
/// [`Selection`] in hand (a draw pass, a hit-test) uses.
///
/// Two things resolve to the placement rather than to a layer nobody can act
/// on: an address the container no longer has (a body freed, a def redrawn
/// shorter), and a layer that is **not drawn**. The second is the visualization
/// half meeting the editing half: what is hidden is not being edited, or the
/// window would be taking presses for a picture it is not showing.
pub fn active(widget: &Widget) -> Layer {
    let Layer::Content(n) = widget.layer else {
        return Layer::Placement;
    };
    match content(widget).nth(n) {
        Some((_, child)) if child.visible => widget.layer,
        _ => Layer::Placement,
    }
}

/// The container's layered contents, in declaration order — the children that
/// fill a role, each with its index among *all* the children. The one walk the
/// stack, the addresses and the drawing gate are read through, so a layer's
/// address never depends on which pass is asking.
fn content(widget: &Widget) -> impl DoubleEndedIterator<Item = (usize, &Widget)> {
    widget
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind.body_role().is_some())
}

/// Whether the child at `index` in `widget`'s children is its **active** layer
/// — what a body is told when it is drawn or handed a press, so an element
/// knows whether the affordances it draws are promises it can keep.
pub fn child_is_active(widget: &Widget, index: usize) -> bool {
    let Layer::Content(n) = active(widget) else {
        return false;
    };
    content(widget)
        .nth(n)
        .is_some_and(|(child_index, _)| child_index == index)
}

/// The layer a press at `(x, y)` belongs to: the **topmost content layer whose
/// own material is under the pointer**, and the placement everywhere else.
///
/// Topmost first, because that is the order they are drawn in and the reader
/// aims at what they can see. A layer answers for its *material* — a curve's
/// points and segments, a note's rectangle — and never for the rectangle it
/// shares with the container, which is what leaves the clip's background and
/// its grips to the placement: "drag the background to move the clip" is a rule
/// about what the background is *not* claimed by.
///
/// A **hidden** layer is skipped for the same reason: it draws nothing there,
/// so nothing there is its.
///
/// **A layer that cannot be edited is not selected by pointing at it.** An
/// element whose contents are read-only answers `false` here (the same `false`
/// as empty space), so a press on the notes of a pattern falls through to the
/// placement and the clip moves — which is what the refusal that used to
/// consume it was standing in the way of. A script that wants such a layer
/// active anyway says so with `/gui_set layer`, and the element still refuses
/// the edit itself.
pub fn under_pointer(widget: &Widget, at: (f64, f64), input: &Input) -> Layer {
    // **The active layer is asked first.** Layers overlap, and what a hand is
    // already editing keeps the pixels it draws on: an automation being edited
    // does not lose a point because a note sits under it. Only where the active
    // layer draws nothing does the question become which of the others is
    // there.
    let on = |child: &Widget| {
        // A layer that is not drawn is not pointed at: the pixels belong to
        // whatever *is* drawn there. And it is asked on **its own** rectangle
        // and axis, which is the container's unless it names a stretch of it.
        child.visible
            && child
                .kind
                .as_element()
                .is_some_and(|el| el.layer_hit(at, &layer_input(input, child)))
    };
    if let Layer::Content(n) = active(widget)
        && let Some((_, child)) = content(widget).nth(n)
        && on(child)
    {
        return Layer::Content(n);
    }
    content(widget)
        .enumerate()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|(n, (_, child))| on(child).then_some(Layer::Content(n)))
        .unwrap_or(Layer::Placement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::widget::Widget;

    fn clip(bodies: &str) -> Widget {
        let json = format!(r#"{{"type":"field","id":1,"offset":0.0,"dur":100.0,{bodies}}}"#);
        Widget::from_node(
            1,
            &crate::host::guidef::GuiNode::parse(json.as_bytes()).unwrap(),
            &[],
        )
        .unwrap()
    }

    /// The stack is the children that fill a role, in declaration order — and
    /// the names are the roles', which is what a script already calls them.
    #[test]
    fn a_stack_is_read_off_the_children_and_named_by_their_roles() {
        let w = clip(r#""data":[0.0,1.0],"notes":[0,10,60,100,0],"points":[0,0,0,0]"#);
        assert_eq!(
            stack(&w),
            vec![BodyRole::Take, BodyRole::Notes, BodyRole::Curve]
        );
        assert_eq!(Layer::Content(0).name(&w), "take");
        assert_eq!(Layer::Content(2).name(&w), "points");
        assert_eq!(Layer::Placement.name(&w), PLACEMENT);
        // A layer index the container does not have is the placement, not a
        // panic and not a layer nobody can see.
        assert_eq!(Layer::Content(9).name(&w), PLACEMENT);
    }

    /// A name resolves against the stack that is there; an unknown one is
    /// refused rather than quietly meaning the placement.
    #[test]
    fn a_name_resolves_against_the_container_that_holds_the_layers() {
        let mut w = clip(r#""notes":[0,10,60,100,0],"points":[0,0,0,0]"#);
        let sel = Selection::of(&mut w).unwrap();
        assert_eq!(sel.parse("clip"), Some(Layer::Placement));
        assert_eq!(sel.parse("notes"), Some(Layer::Content(0)));
        assert_eq!(sel.parse("points"), Some(Layer::Content(1)));
        assert_eq!(sel.parse("take"), None, "no take on this clip");
        assert_eq!(sel.parse("points:1"), None, "only one automation");
        assert_eq!(sel.parse("nonsense"), None);
    }

    /// **The visualization half**: what is drawn is a separate statement from
    /// what is edited, named the same way — and what is not drawn is not
    /// edited either, because a window taking presses for a picture it is not
    /// showing is the one combination that cannot be read off the screen.
    #[test]
    fn hiding_a_layer_takes_it_out_of_the_picture_and_out_of_the_hand() {
        let mut w = clip(r#""notes":[0.0,10.0,60.0,100,0],"points":[0.0,0.0,1,0.0]"#);
        let mut sel = Selection::of(&mut w).unwrap();
        sel.set(Layer::Content(1));
        assert_eq!(sel.layer(), Layer::Content(1));
        assert!(sel.set_hidden("points"));
        assert_eq!(sel.hidden(), "points");
        assert_eq!(
            sel.layer(),
            Layer::Placement,
            "a hidden layer is not the one in hand"
        );
        // The address is untouched by hiding: showing it again is all it takes.
        assert!(sel.set_hidden(""));
        assert_eq!(sel.hidden(), "");
        assert_eq!(sel.layer(), Layer::Content(1), "and the hand is back on it");
        // A name for a layer this clip has not got is kept for the day it does.
        assert!(!sel.set_hidden("take"));
    }

    /// **Two automations over one clip**, which is what the stack being the
    /// container's contents buys: they are two layers, named apart, and one of
    /// them is edited at a time. Nothing here counts kinds — the second curve
    /// is the second curve because it is declared second.
    #[test]
    fn a_container_may_hold_two_layers_of_one_role() {
        let w = clip(concat!(
            r#""data":[0.0,1.0],"children":["#,
            r#"{"type":"curve","points":[0.0,0.0,1,0.0,100.0,1.0,1,0.0],"#,
            r##""min":0.0,"max":1.0,"color":"#ff6666"},"##,
            r#"{"type":"curve","points":[0.0,1.0,1,0.0,100.0,0.0,1,0.0],"#,
            r##""min":0.0,"max":1.0,"color":"#66aaff"}]"##,
        ));
        assert_eq!(
            stack(&w),
            vec![BodyRole::Take, BodyRole::Curve, BodyRole::Curve]
        );
        assert_eq!(Layer::Content(1).name(&w), "points");
        assert_eq!(Layer::Content(2).name(&w), "points:1");
        // ...and each carries its own colour, since a layer is a node.
        assert!(w.children[1].color.is_some() && w.children[2].color.is_some());
        assert_ne!(w.children[1].color, w.children[2].color);
    }

    /// Setting reports whether it changed, and a stale index reads back as the
    /// placement.
    #[test]
    fn a_selection_says_whether_it_changed_and_never_points_at_nothing() {
        let mut w = clip(r#""points":[0,0,0,0]"#);
        let mut sel = Selection::of(&mut w).unwrap();
        assert_eq!(sel.layer(), Layer::Placement);
        assert!(sel.set(Layer::Content(0)));
        assert!(
            !sel.set(Layer::Content(0)),
            "the same layer twice is silent"
        );
        assert_eq!(sel.layer(), Layer::Content(0));
        // The body goes away: what is active is the placement again.
        w.children.clear();
        assert_eq!(Selection::of(&mut w).unwrap().layer(), Layer::Placement);
    }
}
