//! **What an action reads as in a window, and what it addresses**: the one
//! reading a menu entry, a tool, a key and a client's `/gui_verb` all consult
//! before the verb is performed.
//!
//! The owner of a window publishes its actions' readings as the window's
//! `actions` prop ([`clausters_editing::verbs::ActionState`], one per verb),
//! computed from what it holds each time something changes. The host keeps
//! that object as it was sent and resolves two things the owner cannot know:
//!
//! - **the text field holding the focus answers first.** A field's own
//!   editing (its copy, its paste, an undo it keeps no history for) is read
//!   before any document's, so Ctrl+Z while typing never reaches the take
//!   behind the field ([`Element::action_state`]).
//! - **the selection as it is now.** A sweep or a pick is drawn here first
//!   and its owner hears of it after, so an action whose only objection is an
//!   empty selection (`unselected`) reads the selection of the view it
//!   addresses -- the window's `main`, else its one take: the notes or clips
//!   the element holds, or the span of its axis -- and is never refused on
//!   the owner's reading alone, which may predate the gesture.
//! - **the clipboard as it is now.** A paste is offered only where what is on
//!   the clipboard fits the destination the owner named: as many channels, the
//!   same rate for a mix, and a server copy made on the same connection.
//!
//! - **a dialog that is up.** Nothing behind a dialog is reached by the
//!   pointer, and the same holds for an action: while one is up, a verb is
//!   performed only by the element holding the focus inside it (a field's
//!   undo, its paste), and every reading of the window behind it is refused
//!   ([`Host::dialog`]). That holds in every window, with readings or none.
//!
//! A verb with no reading is not an action this window's owner describes, and
//! is performed as it always was.
//!
//! [`Element::action_state`]: super::widget::Element::action_state

use clausters_document::clipboard::Content;
use clausters_editing::verbs::{self, ActionKind, ActionState, ActionTarget, PasteTarget};

use super::Host;
use super::clipboard::Clip;
use super::menu::{Entry, EntryKind};
use super::widget::Widget;

impl Host {
    /// **Action `name` in window `def`, as it reads now**, or `None` when
    /// nothing in the window describes it.
    pub(crate) fn action_state(&self, def: i32, name: &str, clip: &Clip) -> Option<ActionState> {
        let focused = self
            .focused()
            .filter(|(window, _)| *window == def)
            .map(|(_, id)| id);
        resolve(
            self.window_def(def)?,
            focused,
            self.window_selected(def),
            name,
            clip,
        )
    }

    /// **Whether the view window `def`'s actions address holds a
    /// selection**: its `main` element, else its one take -- what the element
    /// says it holds (notes, clips), else the span of its navigation group.
    /// `None` when the window names neither, and the owner's reading stands.
    pub(crate) fn window_selected(&self, def: i32) -> Option<bool> {
        let view = self
            .window_main(def)
            .or_else(|| super::gestures::sole_take(self, def))?;
        let held = self
            .window_def(def)?
            .find(view)
            .and_then(|w| w.kind.as_element())
            .and_then(|el| el.holds_selection());
        held.or_else(|| {
            let group = self.timelines().state(self.timeline_key(view)?)?;
            Some(group.selection().is_some())
        })
    }

    /// **Whether window `def` offers verb `name`** of the host's own table.
    /// A window whose owner publishes readings describes its actions whole,
    /// so a verb of the table it gives none is not that window's: its key
    /// does nothing and the key sheet leaves it out (the audio editor's has no
    /// `split`, `join`, `quantize` or `snap`). A window with no readings
    /// offers every one, a verb the focused element answers itself is
    /// offered, and a verb outside the table is its owner's.
    pub(crate) fn offers(&self, def: i32, name: &str) -> bool {
        self.window_def(def).is_none_or(|tree| {
            tree.actions.is_empty()
                || verbs::find(verbs::WINDOW, name).is_none()
                || self.action_state(def, name, &self.clipboard).is_some()
        })
    }

    /// **The dialog up in window `def`**, if one is: the modal panel a
    /// reader sees -- not one in a hidden page of a stack, nor in an invisible
    /// subtree.
    pub(crate) fn dialog(&self, def: i32) -> Option<&Widget> {
        dialog_of(self.window_def(def)?)
    }

    /// The element holding window `def`'s focus, when it is inside the
    /// dialog up there -- the only one a verb reaches while it is up.
    pub(crate) fn focused_in_dialog(&self, def: i32) -> Option<i32> {
        let dialog = self.dialog(def)?;
        self.focused()
            .filter(|(window, id)| *window == def && dialog.find(*id).is_some())
            .map(|(_, id)| id)
    }

    /// **Marks a menu's entries with their actions' state**, at every depth: an
    /// action that cannot be taken is disabled, a switch is a check and a
    /// choice is one of a group. What a list shows when it opens -- the
    /// window's own menu is left as its owner sent it.
    pub(crate) fn action_entries(&self, def: i32, entries: &mut [Entry]) {
        let dialog = self.dialog(def).is_some();
        for entry in entries {
            if let EntryKind::Submenu(children) = &mut entry.kind {
                self.action_entries(def, children);
                continue;
            }
            if dialog && entry.verb.is_some() {
                // what a menu does is behind the dialog
                entry.enabled = false;
                continue;
            }
            let Some(state) = entry
                .verb
                .as_deref()
                .and_then(|name| self.action_state(def, name, &self.clipboard))
            else {
                continue;
            };
            entry.enabled &= state.enabled;
            if let Some(on) = state.checked {
                entry.kind = match (state.kind, state.group) {
                    (ActionKind::Choice, Some(group)) => EntryKind::Radio { group, on },
                    _ => EntryKind::Check(on),
                };
            }
        }
    }
}

/// **Action `name` over window `tree`**, with `focused` the widget holding its
/// keyboard focus: the focused element's own reading first, then the one the
/// window's owner published, checked against what the clipboard holds.
/// `selected` is whether the view the window's actions address holds a
/// selection, where the host can tell.
pub(crate) fn resolve(
    tree: &Widget,
    focused: Option<i32>,
    selected: Option<bool>,
    name: &str,
    clip: &Clip,
) -> Option<ActionState> {
    if let Some(widget) = focused
        && let Some(el) = tree.find(widget).and_then(|w| w.kind.as_element())
        && let Some(verb) = verbs::find(verbs::WINDOW, name)
        && let Some(state) = el.action_state(&verb, ActionTarget::Text { widget }, clip)
    {
        return Some(state);
    }
    let mut state = tree.actions.get(name)?.clone();
    if dialog_of(tree).is_some() {
        state.refuse("a dialog is open");
        return Some(state);
    }
    if state.unselected && selected == Some(true) {
        state.enabled = true;
        state.unselected = false;
        state.reason = None;
    }
    if let Some(target) = state.paste {
        paste_fits(&mut state, target, name == "mix", clip);
    }
    Some(state)
}

/// The dialog up in `widget`'s subtree: a modal panel on the path a layout
/// places -- visible, and the shown page of a stack.
fn dialog_of(widget: &Widget) -> Option<&Widget> {
    if !widget.visible {
        return None;
    }
    if widget.kind.is_modal() {
        return Some(widget);
    }
    if let super::widget::WidgetKind::Stack { index, .. } = &widget.kind {
        return usize::try_from(*index)
            .ok()
            .and_then(|i| widget.children.get(i))
            .and_then(dialog_of);
    }
    widget.children.iter().find_map(dialog_of)
}

/// **Refuses a paste the clipboard does not fit** into `target`: another kind
/// of contents, or samples of another shape -- `mix` needing the same rate as
/// well -- or a server copy made on another connection.
fn paste_fits(state: &mut ActionState, target: PasteTarget, mix: bool, clip: &Clip) {
    let PasteTarget::Samples {
        channels: wanted,
        rate,
        connection,
        generation,
    } = target;
    match clip.doc().map(|doc| &doc.content) {
        Some(Content::Buffer { location, .. })
            if !location
                .as_ref()
                .is_some_and(|at| at.connection == connection && at.generation == generation) =>
        {
            state.refuse("the clipboard's copy is on another server connection");
        }
        Some(
            Content::Samples {
                channels,
                frames,
                sample_rate,
                ..
            }
            | Content::Buffer {
                channels,
                frames,
                sample_rate,
                ..
            },
        ) => {
            if *frames == 0 {
                state.refuse("the clipboard is empty");
            } else if *channels != wanted {
                state.refuse("the clipboard holds another number of channels");
            } else if mix && *sample_rate != rate {
                state.refuse("a mix needs the clipboard at the same rate");
            }
        }
        _ => state.refuse("the clipboard holds no samples"),
    }
}
