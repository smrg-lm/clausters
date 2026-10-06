//! **Closing a window that may hold unsaved work**: one rule, for every
//! application.
//!
//! A window is closed by the close mark on its frame, by a desktop window's
//! Escape, or by a File menu's Close. Whether anything is lost depends on who
//! holds the work, and only that:
//!
//! - **A client holds it** in its own objects -- the score, the take, the
//!   sequence the editor edits -- and the handle keeps it: the client can
//!   save it, read it, or open the window again, with its chrome or without.
//!   Closing loses nothing, so nothing is asked, and what becomes of the
//!   edited data is the client's to decide.
//! - **The window is the only holder** in a standalone host, which has no
//!   client beside it. There an editor with changes its file does not hold
//!   asks first -- Don't save, Cancel, Save -- and the window carries
//!   `ask_close`, so the host holds its close mark for the editor's answer.
//!
//! The holder says which when it opens the editor
//! ([`Converse::asks_to_close`]); what each application adds is whether it
//! has something to lose ([`Converse::unsaved`]) and the form it asks in
//! ([`Converse::ask_to_close`]), which it builds with [`dialog`]. The verb and
//! the turn that reads it are here, so every editor closes by the same rule.

use serde_json::{Value, json};

use clausters_editing::conversation::Correction;

use crate::turn::{Converse, Turned};

/// **The window's close**: what a window carrying `ask_close` reports when
/// its close mark is pressed, and the File menu's Close.
pub const VERB: &str = "close";

/// The close form's three buttons, by the name each is numbered under after
/// the form's own prefix (`close:ok`).
pub const SAVE: &str = "ok";
/// Close without saving.
pub const DISCARD: &str = "discard";
/// Keep the window.
pub const CANCEL: &str = "cancel";

/// **What the close form was answered with.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Write the work, and close once it is written.
    Save,
    /// Let the work go and close.
    Discard,
    /// Keep the window, and the work in it.
    Cancel,
}

/// The answer a button of the close form is, by its name after the prefix.
#[must_use]
pub fn choice(button: &str) -> Option<Choice> {
    match button {
        SAVE => Some(Choice::Save),
        DISCARD => Some(Choice::Discard),
        CANCEL => Some(Choice::Cancel),
        _ => None,
    }
}

/// **The close form as a dialog**: the question, and `Don't save` apart from
/// `Cancel` and `Save`. `id` numbers a button by its name ([`SAVE`],
/// [`DISCARD`], [`CANCEL`]); the dialog itself is `dialog_id`.
#[must_use]
pub fn dialog(dialog_id: Option<i32>, question: &str, id: impl Fn(&str) -> Option<i32>) -> Value {
    let spring = json!({"type": "separator", "weight": 1, "line": false});
    let buttons = json!({
        "type": "layout",
        "flow": "row",
        "hug": true,
        "pack": true,
        "margin": 0,
        "children": [
            {"type": "button", "id": id(DISCARD), "label": "Don't save"},
            spring,
            {"type": "button", "id": id(CANCEL), "label": "Cancel"},
            {"type": "button", "id": id(SAVE), "label": "Save"},
        ],
    });
    json!({
        "type": "layout",
        "id": dialog_id,
        "modal": true,
        "frame": true,
        "hug": true,
        "title": "Close",
        "flow": "col",
        "children": [{"type": "label", "text": question}, buttons],
    })
}

/// **The close form's widgets, numbered by the window's holder**: the
/// stack it is a page of, the dialog, and its three buttons. An application
/// with no dialogs of its own composes the form with these ([`stack`]); one
/// that has a stack of forms already puts [`dialog`] on a page of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    /// The stack: page 0 holds nothing, page 1 the form.
    pub stack: i32,
    /// The dialog.
    pub dialog: i32,
    /// `Save`.
    pub save: i32,
    /// `Don't save`.
    pub discard: i32,
    /// `Cancel`.
    pub cancel: i32,
}

impl Ids {
    /// Whether `widget` is one of the form's.
    #[must_use]
    pub fn contains(&self, widget: i64) -> bool {
        [
            self.stack,
            self.dialog,
            self.save,
            self.discard,
            self.cancel,
        ]
        .iter()
        .any(|id| i64::from(*id) == widget)
    }

    /// **What a report from the form answers**: a button's click, or the
    /// dialog's own `cancel` (its close mark, Escape).
    #[must_use]
    pub fn read(&self, widget: i64, tag: &str) -> Option<Choice> {
        let is = |id: i32| i64::from(id) == widget;
        if is(self.dialog) {
            return (tag == "cancel").then_some(Choice::Cancel);
        }
        if tag != "click" {
            return None;
        }
        if is(self.save) {
            Some(Choice::Save)
        } else if is(self.discard) {
            Some(Choice::Discard)
        } else if is(self.cancel) {
            Some(Choice::Cancel)
        } else {
            None
        }
    }
}

/// **The close form as a window holds it**: a stack that takes no room, page
/// 0 empty and page 1 the form -- in the window from the start, so asking is
/// a correction of the stack's `index` ([`shown`]) and nothing is defined.
#[must_use]
pub fn stack(ids: &Ids, question: &str) -> Value {
    let form = dialog(Some(ids.dialog), question, |name| match name {
        SAVE => Some(ids.save),
        DISCARD => Some(ids.discard),
        CANCEL => Some(ids.cancel),
        _ => None,
    });
    json!({
        "type": "layout",
        "id": ids.stack,
        "flow": "stack",
        "index": 0,
        "h": 0,
        "margin": 0,
        "children": [{"type": "layout"}, {"type": "layout", "children": [form]}],
    })
}

/// The correction that puts the close form up, or takes it down.
#[must_use]
pub fn shown(ids: &Ids, up: bool) -> Vec<Correction> {
    vec![Correction {
        widget: i64::from(ids.stack),
        props: json!({"index": i32::from(up)}),
    }]
}

/// **Whether a window asks before it closes** (its `ask_close` prop): only
/// when its holder said the window is the work's only one, and it has a form
/// to ask in.
#[must_use]
pub fn asks(holder_asks: bool, has_form: bool) -> bool {
    holder_asks && has_form
}

/// **A close asked of `editor`'s window**: at once, unless the window is the
/// work's only holder and has something to lose -- then the editor's form
/// asks. The reason it could not ask, or the corrections that show the form.
pub fn close<E: Converse>(
    editor: &mut E,
    out: &mut E::Outcome,
) -> (Option<String>, Vec<Correction>) {
    if editor.asks_to_close() && editor.unsaved() {
        return editor.ask_to_close(out);
    }
    out.set_close(true);
    (None, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::{self, Event, Kind};
    use clausters_editing::conversation::{Answer, Conversation};

    #[derive(Debug, Default)]
    struct Out {
        turn: Kind,
        answer: Option<Answer>,
        seq: i64,
        redo: bool,
        version: i64,
        close: bool,
    }
    turn::turned!(Out);

    /// An editor with nothing of its own but what the rule asks of one.
    struct Any {
        conversation: Conversation,
        unsaved: bool,
        alone: bool,
    }

    impl Converse for Any {
        type Outcome = Out;
        fn conversation(&mut self) -> &mut Conversation {
            &mut self.conversation
        }
        fn window_id(&self) -> Option<i32> {
            Some(1)
        }
        fn closed(&mut self) {}
        fn owns(&self, _widget: i64, _tag: &str) -> bool {
            false
        }
        fn resync(&mut self, _widget: i64) -> Vec<Correction> {
            Vec::new()
        }
        fn route(
            &mut self,
            _widget: i64,
            _tag: &str,
            _values: &[Value],
            _out: &mut Out,
        ) -> (Option<String>, Vec<Correction>) {
            (None, Vec::new())
        }
        fn asks_to_close(&self) -> bool {
            self.alone
        }
        fn unsaved(&self) -> bool {
            self.unsaved
        }
    }

    fn closed(unsaved: bool, alone: bool) -> Out {
        let mut editor = Any {
            conversation: Conversation::new(0),
            unsaved,
            alone,
        };
        let event = Event {
            addr: "/gui_event".into(),
            args: vec![json!(1), json!(7), json!(0), json!(VERB)],
        };
        turn::turn(&mut editor, &event, 0)
    }

    /// **Any editor closes by the rule**: at once, unless its window is the
    /// work's only holder and has something to lose -- and one with no form
    /// to ask in then says so and stays.
    #[test]
    fn every_editor_closes_by_one_rule() {
        for (unsaved, alone) in [(false, false), (true, false), (false, true)] {
            let out = closed(unsaved, alone);
            assert!(out.close, "unsaved {unsaved}, alone {alone}");
            assert_eq!(out.turn, Kind::Route);
        }
        let out = closed(true, true);
        assert!(!out.close, "the only holder, with something to lose");
        let why = match out.answer {
            Some(Answer::Push { reason, .. }) | Some(Answer::Ack { reason, .. }) => reason,
            _ => None,
        }
        .expect("a reason");
        assert!(why.contains("not saved"), "{why}");
    }

    #[test]
    fn the_form_numbers_its_three_buttons_and_reads_them_back() {
        let ids = |name: &str| {
            Some(match name {
                SAVE => 1,
                DISCARD => 2,
                _ => 3,
            })
        };
        let form = dialog(Some(9), "Unsaved.", ids);
        assert_eq!(form["id"], 9);
        assert_eq!(form["modal"], true);
        let buttons = form["children"][1]["children"].as_array().unwrap();
        let labels: Vec<&str> = buttons.iter().filter_map(|b| b["label"].as_str()).collect();
        assert_eq!(labels, ["Don't save", "Cancel", "Save"]);
        assert_eq!(choice("ok"), Some(Choice::Save));
        assert_eq!(choice("discard"), Some(Choice::Discard));
        assert_eq!(choice("cancel"), Some(Choice::Cancel));
        assert_eq!(choice("title"), None);
        assert!(asks(true, true) && !asks(false, true) && !asks(true, false));
    }

    /// The form a window holds is a page of a stack, up by a correction, and
    /// its buttons and its own close are read back as the three answers.
    #[test]
    fn a_held_form_is_a_page_and_reads_its_answers() {
        let ids = Ids {
            stack: 20,
            dialog: 21,
            save: 22,
            discard: 23,
            cancel: 24,
        };
        let held = stack(&ids, "Unsaved.");
        assert_eq!(
            (held["id"].clone(), held["index"].clone()),
            (json!(20), json!(0))
        );
        assert_eq!(held["children"][1]["children"][0]["id"], 21);
        assert_eq!(shown(&ids, true)[0].props, json!({"index": 1}));
        assert_eq!(ids.read(22, "click"), Some(Choice::Save));
        assert_eq!(ids.read(23, "click"), Some(Choice::Discard));
        assert_eq!(ids.read(24, "click"), Some(Choice::Cancel));
        assert_eq!(ids.read(21, "cancel"), Some(Choice::Cancel));
        assert_eq!(ids.read(22, "focus"), None);
        assert!(ids.contains(20) && !ids.contains(25));
    }
}
