//! **Where the keyboard goes as a dialog comes and goes.** A dialog takes
//! the keys as it comes up -- to its first field, else to its default button
//! -- and gives them back to what had them when it goes, as the design
//! system's dialog does. Enter in one of its fields presses that default
//! button, as a form submits ([`default_button`]). Tab staying inside it is the ring's
//! ([`gestures::focus`](super::gestures)), and Escape and a press on the
//! scrim asking it to go are the gesture machine's.
//!
//! A dialog comes up however a window says so -- defined, its stack's page
//! turned, freed, its `modal` set -- so the host looks after each change that
//! can do it ([`Host::settle_dialog`]) rather than in any one of them.

use super::widget::{Widget, WidgetKind};
use super::{Host, HostEffect};

/// A dialog up in a window, and what had the keyboard before it came up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Up {
    /// The dialog.
    pub dialog: i32,
    /// The widget of the same window that had the keys, if one did.
    pub opener: Option<i32>,
}

/// **Where a dialog's keyboard starts**: its first field that takes typed
/// text, in the order it is read, else its default button.
fn first_stop(dialog: &Widget) -> Option<i32> {
    let mut field = None;
    let mut default = None;
    walk(dialog, &mut |w| {
        let Some(el) = w.kind.as_element().filter(|el| el.accepts_focus()) else {
            return;
        };
        if field.is_none() && el.takes_text() {
            field = w.id;
        }
        if default.is_none() && el.is_default() {
            default = w.id;
        }
    });
    field.or(default)
}

/// **The dialog's default button**, the one Enter in a field presses: the
/// first a reader sees that names itself `default` and can be pressed.
pub(crate) fn default_button(dialog: &Widget) -> Option<i32> {
    let mut found = None;
    walk(dialog, &mut |w| {
        if found.is_none() && w.enabled && w.kind.as_element().is_some_and(|el| el.is_default()) {
            found = w.id;
        }
    });
    found
}

/// Every widget of `w` a reader sees, in order: visible, and on the shown
/// page of a stack.
fn walk(w: &Widget, each: &mut impl FnMut(&Widget)) {
    if !w.visible {
        return;
    }
    each(w);
    if let WidgetKind::Stack { index, .. } = &w.kind {
        if let Some(page) = usize::try_from(*index).ok().and_then(|i| w.children.get(i)) {
            walk(page, each);
        }
        return;
    }
    for child in &w.children {
        walk(child, each);
    }
}

/// Whether widget `id` is one of `tree`'s a reader sees.
fn shown(tree: &Widget, id: i32) -> bool {
    let mut found = false;
    walk(tree, &mut |w| found |= w.id == Some(id));
    found
}

impl Host {
    /// The keys back to `opener`, when it is still there to take them.
    fn give_back(&mut self, root: i32, opener: Option<i32>, effects: &mut Vec<HostEffect>) {
        let Some(opener) = opener.filter(|id| self.window_def(root).is_some_and(|t| shown(t, *id)))
        else {
            return;
        };
        if let Some(other) = self.focus(root, opener) {
            effects.push(HostEffect::Redraw(other));
        }
        effects.push(HostEffect::Redraw(root));
    }

    /// **A change that may have brought a dialog up in window `root`, or
    /// taken one down**: one that comes up takes the keys to its first stop
    /// ([`first_stop`]), remembering what had them; one that goes gives them
    /// back, if what had them is still there and nothing else took them.
    pub(crate) fn settle_dialog(&mut self, root: i32, effects: &mut Vec<HostEffect>) {
        let now = self.dialog(root).map(|d| (d.id, first_stop(d)));
        let was = self.dialogs_up.get(&root).copied();
        match (was, now) {
            (None, None) => {}
            (Some(up), Some((Some(id), _))) if up.dialog == id => {}
            (was, Some((id, stop))) => {
                let opener = was.map_or_else(
                    || {
                        self.focused()
                            .filter(|(def, _)| *def == root)
                            .map(|(_, id)| id)
                    },
                    |up| up.opener,
                );
                let Some(dialog) = id else {
                    self.dialogs_up.remove(&root);
                    return;
                };
                self.dialogs_up.insert(root, Up { dialog, opener });
                // with nowhere to start in it, the keys reach nothing: what
                // had them is behind it
                match stop {
                    Some(stop) => {
                        if let Some(other) = self.focus(root, stop) {
                            effects.push(HostEffect::Redraw(other));
                        }
                    }
                    None if opener.is_some() => {
                        self.clear_focus();
                    }
                    None => return,
                }
                effects.push(HostEffect::Redraw(root));
            }
            (Some(up), None) => {
                self.dialogs_up.remove(&root);
                // the keys are given back while they are in this window; on
                // the dialog's hidden page they would type into nothing seen
                let Some((def, held)) = self.focused() else {
                    return self.give_back(root, up.opener, effects);
                };
                let seen = |id: i32| self.window_def(root).is_some_and(|t| shown(t, id));
                if def != root || seen(held) {
                    return;
                }
                if up.opener.is_some_and(seen) {
                    self.give_back(root, up.opener, effects);
                } else {
                    self.clear_focus();
                    effects.push(HostEffect::Redraw(root));
                }
            }
        }
    }
}
