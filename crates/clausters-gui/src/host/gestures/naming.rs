//! **A marker's name, typed on the ruler.**
//!
//! A marker is a label at a moment, and the label is what its owner is handed:
//! an OSC marker in a roll *is* the address it sends, so one with no name is
//! nothing to write. So a hand that adds a marker names it before the owner
//! hears of it: the Ctrl+click puts it down numbered, with the number selected
//! in a field drawn where its label is, and what is typed replaces it. Enter,
//! Tab or a press anywhere else gives it the name and reports the markers;
//! Escape takes the new marker away again, unreported. A double click on a
//! marker opens its name the same way, and Escape then leaves it as it was.
//!
//! The text and its caret are the widget's view state
//! ([`crate::host::widget::Naming`]), as a field's caret is the field's, and
//! the editing is the field's own ([`crate::host::elements::edit_key`]), so a
//! name types exactly as a text entry does.

use super::super::Host;
use super::super::clipboard::Clip;
use super::super::graphics::textedit::Caret;
use super::super::widget::Naming;
use super::super::widget::element::{Key, KeyInput, Mods};
use super::{GestureCtx, GestureEffect};

/// The widget of window `def_id` whose marker is being named, if one is.
pub(super) fn naming_in(host: &Host, def_id: i32) -> Option<i32> {
    host.window_def(def_id)?
        .descendants()
        .find(|w| w.kind.editor().is_some_and(|e| e.naming.is_some()))?
        .id
}

/// **Opens the name of marker `index`** of widget `id`, its label selected
/// whole so the first key typed replaces it. `added` is a marker the gesture
/// just put down, which an Escape takes away.
pub(super) fn open(host: &mut Host, def_id: i32, id: i32, index: usize, added: bool) -> bool {
    let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    else {
        return false;
    };
    let Some(marker) = editor.markers.get(index) else {
        return false;
    };
    let text = marker.label.clone();
    editor.naming = Some(Naming {
        index,
        caret: Caret {
            pos: text.len(),
            anchor: Some(0),
        },
        text,
        added,
    });
    true
}

/// **Gives the marker being named in window `def_id` its name**, and reports
/// the markers. A name left empty says nothing: a new marker goes, unreported,
/// and one being renamed keeps the name it had.
pub(super) fn commit(host: &mut Host, out: &mut Vec<GestureEffect>, def_id: i32) {
    let Some((id, naming, mut markers)) = take(host, def_id) else {
        return;
    };
    let name = naming.text.trim();
    out.push(GestureEffect::Redraw(def_id));
    if name.is_empty() {
        if naming.added {
            markers.remove(naming.index);
            write(host, def_id, id, markers);
        }
        return;
    }
    if !naming.added && markers[naming.index].label == name {
        return;
    }
    markers[naming.index].label = name.to_string();
    super::nav::set_markers(host, out, def_id, id, markers);
}

/// **Leaves the name unwritten**: a new marker goes, unreported, and one
/// being renamed keeps the name it had.
pub(super) fn cancel(host: &mut Host, out: &mut Vec<GestureEffect>, def_id: i32) {
    let Some((id, naming, mut markers)) = take(host, def_id) else {
        return;
    };
    if naming.added {
        markers.remove(naming.index);
        write(host, def_id, id, markers);
    }
    out.push(GestureEffect::Redraw(def_id));
}

/// **A key while a name is open in this window**: the name's, whatever it is.
/// Enter and Tab give the name, Escape leaves it, and anything else edits the
/// text as a single-line field does -- a chord the field does not take is
/// swallowed rather than handed to the window, which would act behind a name
/// half typed.
pub(super) fn key(
    host: &mut Host,
    ctx: &GestureCtx,
    key: &Key,
    clipboard: &mut Clip,
) -> Option<Vec<GestureEffect>> {
    let id = naming_in(host, ctx.def_id)?;
    let mut out = Vec::new();
    match key {
        Key::Enter | Key::Tab => commit(host, &mut out, ctx.def_id),
        Key::Escape => cancel(host, &mut out, ctx.def_id),
        _ => {
            let naming = host
                .window_def_mut(ctx.def_id)
                .and_then(|t| t.find_mut(id))
                .and_then(|w| w.kind.editor_mut())
                .and_then(|e| e.naming.as_mut())?;
            let mut input = KeyInput {
                mods: Mods {
                    shift: ctx.shift,
                    ctrl: ctx.ctrl,
                    alt: ctx.alt,
                },
                clipboard,
                cursor: None,
            };
            crate::host::elements::edit_key(
                &mut naming.text,
                &mut naming.caret,
                false,
                key,
                &mut input,
            );
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
    }
    Some(out)
}

/// The name open in window `def_id`, closed: the widget, the name as it was
/// typed and the markers it was over -- `None` where none was open, or where
/// the marker it named is no longer there.
fn take(host: &mut Host, def_id: i32) -> Option<(i32, Naming, Vec<crate::host::widget::Marker>)> {
    let id = naming_in(host, def_id)?;
    let editor = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())?;
    let naming = editor.naming.take()?;
    (naming.index < editor.markers.len()).then(|| (id, naming, editor.markers.clone()))
}

/// The widget's markers, written without being reported: what its owner
/// never heard of is taken back the same way.
fn write(host: &mut Host, def_id: i32, id: i32, markers: Vec<crate::host::widget::Marker>) {
    if let Some(editor) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(id))
        .and_then(|w| w.kind.editor_mut())
    {
        editor.markers = markers;
    }
}
