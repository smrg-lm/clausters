//! **What a turn of any application's editor is**: the message a host sends,
//! what kind of turn it came to, and the entry a gesture leaves for the history.
//!
//! Every application answers a host the same way -- a message read by the
//! conversation, a gesture read in the structure's own vocabulary, an entry for
//! the history the caller keeps -- so the words for those are here once rather
//! than once per application.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message, Turn};

/// What kind of turn a message came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Not this editor's, or not an event at all.
    #[default]
    Nothing,
    /// This editor's window closed.
    Closed,
    /// A walk through the history, which the caller takes and then answers.
    Step,
    /// An edit made against a picture that is gone: refused, and the picture
    /// handed back.
    Stale,
    /// A gesture, read and answered.
    Route,
}

/// One structure's share of an entry to record: how to redo it, how to put it
/// back, and what makes two of them the same thing done the same way.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Leg {
    /// `{"edit": <payload>}`.
    pub forward: Value,
    /// The payload that puts the structure back, read before the edit landed.
    pub backward: Value,
    /// The coalesce key, empty for an edit that never coalesces.
    pub key: String,
    /// **The sources redoing this leg reads** -- the takes a list of parts
    /// names -- which the history holds for as long as the entry can be
    /// walked. Empty for a leg over no sources.
    #[serde(rename = "holdsForward", skip_serializing_if = "Vec::is_empty")]
    pub holds_forward: Vec<u64>,
    /// The sources undoing it reads.
    #[serde(rename = "holdsBackward", skip_serializing_if = "Vec::is_empty")]
    pub holds_backward: Vec<u64>,
}

/// An entry for the history the caller keeps: one gesture, however many edits
/// it took.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Record {
    /// What an undo menu calls it.
    pub label: String,
    /// The structure's legs, in the order they were applied.
    pub legs: Vec<Leg>,
}

/// One message from the host: its address and its arguments,
/// `<id> <seq> <version> <tag> <payload...>`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Event {
    /// `"/gui_event"` or `"/gui_closed"`.
    pub addr: String,
    /// The arguments, in order.
    #[serde(default)]
    pub args: Vec<Value>,
}

/// A report's number as an integer, whichever way JSON spelled it.
pub(crate) fn int(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|f| f as i64))
        .unwrap_or(0)
}

/// A report's number.
pub(crate) fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

/// A report's word.
pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The fields every editor's outcome shares, so [`turn`] can fill them whatever
/// else an editor's own outcome carries.
pub trait Turned: Default {
    /// Starts an outcome at `version`.
    fn at(version: i64) -> Self;
    /// What kind of turn it was.
    fn set_turn(&mut self, turn: Kind);
    /// What the host is sent.
    fn set_answer(&mut self, answer: Answer);
    /// The stamp and the direction of a walk through the history.
    fn set_step(&mut self, seq: i64, redo: bool);
    /// The version after the turn -- moved by a route that edited.
    fn version(&self) -> i64;
    /// Whether its holder closes the window ([`crate::closing`]).
    fn set_close(&mut self, close: bool);
}

/// Implements [`Turned`] for an outcome with the usual field names.
macro_rules! turned {
    ($outcome:ty) => {
        impl crate::turn::Turned for $outcome {
            fn at(version: i64) -> Self {
                Self {
                    version,
                    ..Self::default()
                }
            }
            fn set_turn(&mut self, turn: crate::turn::Kind) {
                self.turn = turn;
            }
            fn set_answer(&mut self, answer: clausters_editing::conversation::Answer) {
                self.answer = Some(answer);
            }
            fn set_step(&mut self, seq: i64, redo: bool) {
                self.seq = seq;
                self.redo = redo;
            }
            fn version(&self) -> i64 {
                self.version
            }
            fn set_close(&mut self, close: bool) {
                self.close = close;
            }
        }
    };
}
pub(crate) use turned;

/// **What an editor answers for, in one conversation turn.** Everything a turn
/// does that is not the editor's own -- reading the message, the history walk,
/// the refusal of a stale gesture, the acknowledgement -- is [`turn`]'s; these
/// are the parts that differ from one application to the next.
pub trait Converse {
    /// The editor's outcome.
    type Outcome: Turned;
    /// The conversation it keeps.
    fn conversation(&mut self) -> &mut Conversation;
    /// Its window, when it has one open.
    fn window_id(&self) -> Option<i32>;
    /// Its window closed.
    fn closed(&mut self);
    /// Whether `widget` is one of its own for a gesture with this tag.
    fn owns(&self, widget: i64, tag: &str) -> bool;
    /// The corrections that put `widget` back as the structure has it.
    fn resync(&mut self, widget: i64) -> Vec<Correction>;
    /// One gesture read in the structure's own vocabulary: the reason it was
    /// refused, if it was, and the corrections the acknowledgement carries.
    /// What else it did goes on `out`.
    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Self::Outcome,
    ) -> (Option<String>, Vec<Correction>);
    /// **Whether the window is the work's only holder**, so a close with
    /// something unsaved asks first ([`crate::closing`]). A client holds its
    /// own structure and loses nothing by a close, so by default no.
    fn asks_to_close(&self) -> bool {
        false
    }
    /// Whether the structure holds changes its file does not.
    fn unsaved(&self) -> bool {
        false
    }
    /// **Ask before closing**: the editor's close form shown, or why it
    /// could not be. An editor with no form to ask in says so, and the
    /// window stays.
    fn ask_to_close(&mut self, _out: &mut Self::Outcome) -> (Option<String>, Vec<Correction>) {
        (
            Some(
                "this window has changes that are not saved, and no form to ask about them".into(),
            ),
            Vec::new(),
        )
    }
    /// A verb of the window itself -- a save, a play -- answered before the
    /// conversation reads the message; `true` when it was one.
    fn window_verb(
        &mut self,
        _message: &Message,
        _args: &[Value],
        _out: &mut Self::Outcome,
    ) -> bool {
        false
    }
}

/// **One turn of an editor's conversation**: the message the host sent, read
/// and answered, the gesture in it handed to the editor's
/// [`Converse::route`].
pub fn turn<E: Converse>(editor: &mut E, event: &Event, version: i64) -> E::Outcome {
    let args = &event.args;
    let widget = args.first().map_or(0, int);
    let tag = args.get(3).map(text).unwrap_or_default();
    let window = editor.window_id();
    let message = Message {
        addr: event.addr.clone(),
        argc: args.len(),
        widget,
        seq: args.get(1).map_or(0, int),
        against: args.get(2).map_or(0, int),
        owns: editor.owns(widget, &tag),
        tag,
        version,
        is_window: window.is_some()
            && (args.is_empty() || i64::from(window.unwrap_or_default()) == widget),
    };
    let mut out = E::Outcome::at(version);
    // **The window's close is every editor's**, by one rule
    if message.addr == "/gui_event" && message.is_window && message.tag == crate::closing::VERB {
        let (reason, corrections) = crate::closing::close(editor, &mut out);
        out.set_turn(Kind::Route);
        out.set_answer(conversation::answer(
            message.seq,
            out.version(),
            reason,
            corrections,
        ));
        return out;
    }
    if editor.window_verb(&message, args, &mut out) {
        return out;
    }
    match editor.conversation().read(&message) {
        Turn::Nothing => {}
        Turn::Closed => {
            out.set_turn(Kind::Closed);
            editor.closed();
        }
        Turn::Step { seq, redo } => {
            out.set_turn(Kind::Step);
            out.set_step(seq, redo);
        }
        Turn::Stale {
            widget,
            seq,
            reason,
        } => {
            out.set_turn(Kind::Stale);
            let corrections = editor.resync(widget);
            out.set_answer(conversation::answer(
                seq,
                version,
                Some(reason),
                corrections,
            ));
        }
        Turn::Route { widget, seq } => {
            out.set_turn(Kind::Route);
            let values = args.get(4..).unwrap_or_default();
            let (reason, corrections) = editor.route(widget, &message.tag, values, &mut out);
            let after = out.version();
            editor.conversation().applied(after);
            out.set_answer(conversation::answer(seq, after, reason, corrections));
        }
    }
    out
}
