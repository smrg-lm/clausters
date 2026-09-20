//! What a gesture *delivers*: turning a widget's new value or edited payload
//! into the effects the front carries out.
//!
//! Every interaction that produces something the outside world should see ends
//! here, so the bound-vs-event decision (`/gui_bind` to the audio server or
//! another widget, else a `/gui_event` to the script) is made in one place
//! rather than at each gesture.

use crate::host::diag;
use clausters_core::osc::OscType;

use super::super::status;
use super::super::{Host, HostEffect};
use super::GestureEffect;
use super::nav::group_view;

/// Emits `/gui_event widget_id seq <args…>` (as an effect for the front to
/// send), **stamping it** so the owner's acknowledgement can name it.
///
/// The stamp is issued here rather than at either front because a gesture is
/// implemented once: two fronts numbering their own edits would be two
/// sequences, and an acknowledgement would not know which it answered. The
/// counter is behind a `RefCell` for the same reason it is here at all -- this
/// is where an edit is produced, and the tree is borrowed immutably at that
/// point.
pub(super) fn emit(
    host: &Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    args: Vec<OscType>,
) {
    // **The window says what it just did.** Here rather than at each gesture
    // for the reason the stamp is here: this is the one place an edit is
    // produced, so the status bar reads every verb the host emits -- a refusal
    // included, since the host refuses by emitting one -- without a line of it
    // crossing the wire (see `crate::host::status`).
    host.say(def_id, status::Line::of_event(widget_id, &args));
    let seq = host.outbox.borrow_mut().stamp(def_id, widget_id);
    out.push(GestureEffect::Emit {
        def_id,
        widget_id,
        seq,
        args,
    });
}

/// **Refuses the gesture this arm resolved to**, out loud, and consumes the
/// press.
///
/// The other half of the plan's fall-through rule. `GestureMap::plan` resolves
/// `ctrl -> alt -> shift -> plain`, and an arm that returns `false` is saying
/// *this press was not mine* -- the press walks on down the chain and the plain
/// arm sweeps a selection. That is right for a press outside the surface an arm
/// acts on, and wrong for every other way an arm gives up: a pencil that
/// resolved and then could not act would become a selection tool, silently.
///
/// So the rule this expresses is **the press is mine once the plan named my
/// gesture and the pointer is inside the surface I act on** -- after that, every
/// failure is mine, is said, and is consumed. `docs/gui-protocol.md` has
/// specified it since the pencil's zoom gate was written; this is the door that
/// makes it cheap enough to hold everywhere rather than in the one arm that
/// spelled it out by hand.
pub(super) fn refuse(
    host: &Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    verb: &str,
    why: String,
) -> bool {
    emit(
        host,
        out,
        def_id,
        widget_id,
        super::super::widget::element::refusal(verb, &why),
    );
    true // consumed: a plan that resolved does not fall through to a sweep
}

/// Routes a widget's new `value` where it is bound (`/gui_bind`: the audio
/// server on the low-latency path, or another widget's prop), or to the script
/// as a `/gui_event` otherwise. Every interaction that produces a value goes
/// through here, so a single binding check covers them all.
pub(super) fn deliver(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    value: OscType,
) {
    let mut effects = Vec::new();
    if host.forward(widget_id, value.clone(), &mut effects) {
        // Bound: the value went straight to its destination, and whatever the
        // apply behind a widget binding touched has to repaint.
        //
        // **And the window says it anyway.** A binding is the one road a value
        // takes that never passes through `emit`, so the bar went quiet for
        // exactly the controls that act most directly -- a knob wired to the
        // server has a history like every other widget's, and it is the same
        // act whichever road the value took.
        host.say(def_id, status::Line::of_bound(widget_id, &value));
        return redraws(out, effects);
    }
    emit(host, out, def_id, widget_id, vec![value]);
}

/// Turns the host effects an apply produced into gesture effects. A binding's
/// apply is a `/gui_set` without the wire, so the only thing it can ask for is
/// a repaint; anything else would be a window opening behind a knob turn, which
/// a binding has no business doing.
pub(super) fn redraws(out: &mut Vec<GestureEffect>, effects: Vec<HostEffect>) {
    for effect in effects {
        match effect {
            HostEffect::Redraw(root) => out.push(GestureEffect::Redraw(root)),
            other => diag::warn!("a binding's apply asked for {other:?}, which it cannot do"),
        }
    }
}

/// Delivers an edited flat structure -- the edit-back pattern: a **bound**
/// widget forwards `args[1..]` (without the leading tag, which names the event
/// payload, not a server argument) straight to the audio server; an unbound one
/// emits the whole tagged list as a `/gui_event`.
pub(super) fn deliver_args(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    args: Option<Vec<OscType>>,
) {
    let Some(args) = args else {
        return;
    };
    if host.is_bound(widget_id) {
        let mut effects = Vec::new();
        host.forward_args(widget_id, args[1..].to_vec(), &mut effects);
        return redraws(out, effects);
    }
    emit(host, out, def_id, widget_id, args);
}

/// Repaints every window in `roots` (the windows a group mutation touched).
pub(super) fn redraw_all(out: &mut Vec<GestureEffect>, roots: &[i32]) {
    for root in roots {
        out.push(GestureEffect::Redraw(*root));
    }
}

/// Emits a timeline view's visible range as a `/gui_event id "view" start len`
/// -- once per gesture step, carrying the interacted member's id (linked
/// members repaint but do not re-emit).
pub(super) fn emit_view(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, id: i32) {
    if let Some((start, len, _)) = group_view(host, id) {
        emit(
            host,
            out,
            def_id,
            id,
            vec![
                OscType::String("view".into()),
                OscType::Float(start as f32),
                OscType::Float(len as f32),
            ],
        );
    }
}
