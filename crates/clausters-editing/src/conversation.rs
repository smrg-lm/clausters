//! **The exchange between a host and whoever owns the data**, written once.
//!
//! The fourth of the projections, and the one that is not a projection of a
//! structure at all: it is the *protocol* every editor speaks, whatever it
//! edits. A host draws what a hand did and reports it; the owner reads the
//! report, checks it against the version it was made on, applies it, and
//! answers — with an acknowledgement, with corrections, or with a reason. The
//! whole of that was written twice, and it is an algorithm rather than glue:
//! the largest single duplicated block in either client.
//!
//! # What crosses and what does not
//!
//! **The envelope, never the payload.** What a report *means* is the edit
//! ingestion's ([`crate::intake`]) and already crosses once; what this reads is
//! the stamp, the version, the tag and whether this editor drew the widget —
//! nine small fields. So a drag reporting a thousand boxes costs this nothing,
//! which is the whole reason the decision and the reading are separate calls.
//!
//! # The two decisions, and why they are two
//!
//! [`Conversation::read`] says **what kind of turn this is** — a close, a
//! history step, an edit made against a picture that is gone, or an edit to
//! route. [`answer`] says **what to send back**, and it runs afterwards because
//! what an answer carries is collected while routing: the corrections the
//! gesture did not survive intact, and the reason when one is owed.
//!
//! # The floor is the whole of the staleness rule
//!
//! A host stamps every event with the version it was last told, and it is told
//! only when an acknowledgement reaches it — a round trip a hand outruns. So an
//! edit naming a version the owner has already moved past is **the ordinary
//! case**, not a collision: a drag reporting as it goes, or a second gesture
//! begun inside one round trip. Refusing those refuses the hand.
//!
//! What is not ordinary is the data moving by a route the host never saw — a
//! script's edit, a second editor's, an undo — and the floor is what records
//! that: it rises when the version moved without an event moving it, and
//! nothing else moves it. That makes [`Conversation::stale`] a monotone test
//! rather than a race, and it is a verb rather than an assignment because the
//! floor was once *lowered* by a path that meant to reset it, which is not a
//! floor at all.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One message from the host, as much of it as the decision needs.
///
/// The payload is not here: what a report means is the edit ingestion's, and
/// this is only what kind of turn the message is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The OSC address — `"/gui_event"` or `"/gui_closed"`.
    pub addr: String,
    /// How many arguments came with it, which is what tells a malformed event
    /// from one that simply carries no payload.
    #[serde(default)]
    pub argc: usize,
    /// The widget the event names.
    #[serde(default)]
    pub widget: i64,
    /// The stamp an acknowledgement answers.
    #[serde(default)]
    pub seq: i64,
    /// The version the gesture was made against. **Zero is unstated** rather
    /// than a version — an older host, or one no owner has reported to — and
    /// unstated applies unchecked, which is the behaviour there was before
    /// there were versions at all.
    #[serde(default)]
    pub against: i64,
    /// The gesture's tag.
    #[serde(default)]
    pub tag: String,
    /// The version the structure is at **now**.
    #[serde(default)]
    pub version: i64,
    /// Whether the widget the message names is this editor's own window.
    ///
    /// The client's to answer, because it is the one holding the window: a
    /// close with no argument is this editor's if it has a window at all, and a
    /// close naming another window is somebody else's. One host carries several
    /// editors, and an editor with no window of its own — a multitrack that
    /// opened only a composed view — would otherwise read every close as its
    /// own and take itself out of the context that is still drawing.
    #[serde(default)]
    pub is_window: bool,
    /// Whether this editor drew the widget the event names.
    ///
    /// Also the client's: a view's widget table is the client's. Only what this
    /// editor draws is this editor's to answer — a poll loop may be shared with
    /// a second editor, and answering for its window would retire a pending
    /// edit nobody applied.
    #[serde(default)]
    pub owns: bool,
}

/// What one message turns out to be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "turn", rename_all = "camelCase")]
pub enum Turn {
    /// Not this editor's, or not an event at all. Nothing to do and nothing to
    /// say.
    Nothing,
    /// This editor's window closed.
    Closed,
    /// A walk through the history: `Ctrl`+`Z` and its twin, which the host
    /// addresses to the **window** rather than to a widget, because an undo is
    /// not aimed at anything under the cursor.
    ///
    /// Answered here rather than routed: a history step is not an edit to the
    /// data, it is a walk through the one the crate keeps.
    Step {
        /// The stamp to answer.
        seq: i64,
        /// Forward rather than back.
        redo: bool,
    },
    /// The data moved under the gesture, by a route no gesture produced.
    ///
    /// The edit is **not applied and not merged**: an edit-back payload is
    /// absolute *and* whole (a roll's notes are the list, not a diff), so
    /// applying one made against an older picture would silently drop whatever
    /// arrived in between. What goes back is the state as it stands.
    Stale {
        /// The widget to hand the state back to.
        widget: i64,
        /// The stamp to answer.
        seq: i64,
        /// What the host is told.
        reason: String,
    },
    /// An edit to read and apply.
    Route {
        /// The widget the gesture was on.
        widget: i64,
        /// The stamp to answer once it has been applied.
        seq: i64,
    },
}

/// What the acknowledgement is: the sentence a refused edit carries.
const OVERTAKEN: &str = "the composition changed since this edit";

/// **One view's end of the conversation**: the floor, and the version the last
/// answered event left behind.
///
/// Two integers, which is the whole of the state this protocol has. It is a
/// value and not a handle for exactly that reason — a client keeps the pair and
/// hands it back, and a pure function is cheaper to reason about than a
/// lifetime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    /// The **oldest version an incoming edit may name**, raised whenever the
    /// composition moves by a route that is not a host event, and by nothing
    /// else.
    #[serde(default)]
    pub floor: i64,
    /// The version at the last event this answered. When it differs from the
    /// version now, something moved that was not an event — which is what
    /// raises the floor.
    #[serde(default)]
    pub applied: i64,
}

impl Conversation {
    /// A conversation over a structure at `version`: nothing is stale yet.
    pub fn new(version: i64) -> Self {
        Self {
            floor: version,
            applied: version,
        }
    }

    /// Whether an edit made against `against` has been overtaken.
    ///
    /// Zero is unstated and applies unchecked. Overtaken means *by a route the
    /// host never saw*: every version an editor makes while answering the
    /// host's own events is one the host is either about to be told or has been
    /// told already.
    pub fn stale(&self, against: i64) -> bool {
        against != 0 && against < self.floor
    }

    /// The composition moved by a route no gesture took, so what is in flight
    /// was made against a picture that is gone.
    ///
    /// **The only way the floor moves.**
    pub fn raise_floor(&mut self, version: i64) {
        self.floor = version;
    }

    /// The version an answered event left behind.
    pub fn applied(&mut self, version: i64) {
        self.applied = version;
    }

    /// **What this message is**, and what the floor now stands at.
    pub fn read(&mut self, message: &Message) -> Turn {
        if message.addr == "/gui_closed" {
            return if message.is_window {
                Turn::Closed
            } else {
                Turn::Nothing
            };
        }
        if message.addr != "/gui_event" || message.argc < 3 {
            return Turn::Nothing;
        }
        if matches!(message.tag.as_str(), "undo" | "redo") && message.is_window {
            return Turn::Step {
                seq: message.seq,
                redo: message.tag == "redo",
            };
        }
        if !message.owns {
            return Turn::Nothing;
        }
        // The version moved since the last event was answered, and no event is
        // what moved it.
        if message.version != self.applied {
            self.raise_floor(message.version);
        }
        if self.stale(message.against) {
            return Turn::Stale {
                widget: message.widget,
                seq: message.seq,
                reason: OVERTAKEN.into(),
            };
        }
        Turn::Route {
            widget: message.widget,
            seq: message.seq,
        }
    }
}

/// One thing the host should be drawing instead of what it drew.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    /// The widget.
    pub widget: i64,
    /// The props, as the `/gui_*` surface carries them.
    pub props: Value,
}

/// **What to send the host back.**
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "camelCase")]
pub enum Answer {
    /// Nothing: an unasked push with nothing to say retires no pending edit and
    /// corrects no picture, so it is one message the wire does not carry.
    Silent,
    /// `/gui_ack`: the stamp, the version, and a reason where one is owed.
    #[serde(rename_all = "camelCase")]
    Ack {
        /// What is being answered. **A stamp of zero retires nothing**, which
        /// is exactly what an unasked push needs: an undo answers no gesture,
        /// so it carries values and a version and takes no pending edit with
        /// it.
        seq: i64,
        /// The version the host names back on its next gesture. That round trip
        /// is the whole of the staleness check, and it costs one integer.
        doc_version: i64,
        /// Why the edit did not do what it asked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The same, with what the host should be drawing instead — in one bundle,
    /// which is what lets it adopt the correction without a redefine.
    #[serde(rename_all = "camelCase")]
    Push {
        /// See [`Answer::Ack`].
        seq: i64,
        /// See [`Answer::Ack`].
        doc_version: i64,
        /// See [`Answer::Ack`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// What the host draws instead of what it drew.
        corrections: Vec<Correction>,
    },
}

/// **What to answer the host with**, given what the turn came to.
///
/// An editor snaps a placement to the musical grid and refuses an edit to a
/// generator, and without an answer the host could learn neither — so a note
/// dragged onto read-only samples stayed drawn where the hand put it, and a
/// clip landed half a grid step from where it was released. The stamp closes
/// both, because it lets the host retire what it drew and adopt what actually
/// happened.
///
/// There is no success flag anywhere in it: applied, transformed and refused
/// are **one message**, and a refusal is simply the previous value among the
/// corrections.
pub fn answer(
    seq: i64,
    doc_version: i64,
    reason: Option<String>,
    corrections: Vec<Correction>,
) -> Answer {
    if seq == 0 && corrections.is_empty() {
        return Answer::Silent;
    }
    if corrections.is_empty() {
        return Answer::Ack {
            seq,
            doc_version,
            reason,
        };
    }
    Answer::Push {
        seq,
        doc_version,
        reason,
        corrections,
    }
}

/// [`Conversation::read`] over JSON, which is how the two client doors carry
/// it: the conversation's two integers and the message in, the turn and the two
/// integers as they now stand out.
pub fn read_json(state: &str, message: &str) -> String {
    let mut conversation = serde_json::from_str::<Conversation>(state).unwrap_or_default();
    let Ok(message) = serde_json::from_str::<Message>(message) else {
        return serde_json::json!({ "turn": "nothing", "state": conversation }).to_string();
    };
    let turn = conversation.read(&message);
    serde_json::json!({ "turn": turn, "state": conversation }).to_string()
}

/// [`answer`] over JSON.
pub fn answer_json(request: &str) -> String {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Request {
        #[serde(default)]
        seq: i64,
        #[serde(default)]
        doc_version: i64,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        corrections: Vec<Correction>,
    }
    let request: Request = serde_json::from_str(request).unwrap_or(Request {
        seq: 0,
        doc_version: 0,
        reason: None,
        corrections: Vec::new(),
    });
    let answered = answer(
        request.seq,
        request.doc_version,
        request.reason,
        request.corrections,
    );
    serde_json::to_string(&answered).unwrap_or_else(|_| r#"{"answer":"silent"}"#.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(tag: &str, widget: i64, seq: i64, against: i64, version: i64) -> Message {
        Message {
            addr: "/gui_event".into(),
            argc: 5,
            widget,
            seq,
            against,
            tag: tag.into(),
            version,
            is_window: false,
            owns: true,
        }
    }

    /// **The answers lag, and that is not a conflict.** An edit naming a
    /// version the owner has already moved past is the ordinary case: the
    /// acknowledgement had not arrived yet.
    #[test]
    fn an_edit_that_named_an_older_version_is_not_by_itself_stale() {
        let mut talk = Conversation::new(1);
        // Two edits inside one round trip: the second still names version 1.
        assert!(matches!(
            talk.read(&event("clips", 7, 1, 1, 1)),
            Turn::Route { .. }
        ));
        talk.applied(2);
        assert!(matches!(
            talk.read(&event("clips", 7, 2, 1, 2)),
            Turn::Route { .. }
        ));
    }

    /// What is not ordinary is the data moving by a route the host never saw.
    #[test]
    fn a_route_the_host_never_saw_raises_the_floor() {
        let mut talk = Conversation::new(1);
        talk.read(&event("clips", 7, 1, 1, 1));
        talk.applied(1);
        // A script edited the structure: the version moved and no event moved
        // it, so the next gesture made against the old picture is refused.
        let turn = talk.read(&event("clips", 7, 2, 1, 4));
        assert_eq!(
            turn,
            Turn::Stale {
                widget: 7,
                seq: 2,
                reason: OVERTAKEN.into(),
            }
        );
        assert_eq!(talk.floor, 4);
    }

    /// Zero is **unstated** rather than a version, and unstated applies
    /// unchecked — an older host, or one no owner has reported to.
    #[test]
    fn an_unstated_version_applies_unchecked() {
        let mut talk = Conversation::new(9);
        assert!(!talk.stale(0));
        assert!(matches!(
            talk.read(&event("clips", 7, 1, 0, 9)),
            Turn::Route { .. }
        ));
    }

    /// The floor only ever rises. It was once *lowered* by a path that meant to
    /// reset it, which is not a floor at all.
    #[test]
    fn the_floor_is_the_only_thing_that_decides_staleness() {
        let mut talk = Conversation::new(5);
        assert!(talk.stale(4));
        assert!(!talk.stale(5));
        talk.raise_floor(8);
        assert!(talk.stale(7));
    }

    /// An undo is addressed to the window, not to a widget, and is a walk
    /// rather than an edit.
    #[test]
    fn a_history_step_is_not_an_edit() {
        let mut talk = Conversation::new(1);
        let mut message = event("undo", 3, 4, 1, 1);
        message.is_window = true;
        message.owns = false;
        assert_eq!(
            talk.read(&message),
            Turn::Step {
                seq: 4,
                redo: false
            }
        );
        message.tag = "redo".into();
        assert_eq!(talk.read(&message), Turn::Step { seq: 4, redo: true });
    }

    /// A widget this editor did not draw is nobody's business here: answering
    /// for it would retire a pending edit nobody applied.
    #[test]
    fn a_widget_this_editor_did_not_draw_says_nothing() {
        let mut talk = Conversation::new(1);
        let mut message = event("clips", 7, 1, 1, 1);
        message.owns = false;
        assert_eq!(talk.read(&message), Turn::Nothing);
    }

    /// A close is this editor's only when it is this editor's window.
    #[test]
    fn a_close_is_read_against_this_editor_s_own_window() {
        let mut talk = Conversation::new(1);
        let closed = |is_window| Message {
            addr: "/gui_closed".into(),
            argc: 1,
            is_window,
            ..Message::default()
        };
        assert_eq!(talk.read(&closed(true)), Turn::Closed);
        assert_eq!(talk.read(&closed(false)), Turn::Nothing);
        // And a malformed event is nothing rather than an error.
        assert_eq!(
            talk.read(&Message {
                addr: "/gui_event".into(),
                argc: 2,
                ..Message::default()
            }),
            Turn::Nothing
        );
    }

    /// **An unasked push with nothing to say is one message the wire does not
    /// carry**, and everything else is one message whatever happened.
    #[test]
    fn an_answer_is_one_message_whatever_happened() {
        assert_eq!(answer(0, 3, None, Vec::new()), Answer::Silent);
        assert_eq!(
            answer(2, 3, None, Vec::new()),
            Answer::Ack {
                seq: 2,
                doc_version: 3,
                reason: None
            }
        );
        let corrected = answer(
            0,
            3,
            Some("no".into()),
            vec![Correction {
                widget: 7,
                props: json!({ "points": [0.0] }),
            }],
        );
        assert!(matches!(corrected, Answer::Push { seq: 0, .. }));
    }

    /// The doors carry the state out as it now stands, so a client keeps two
    /// integers and nothing else.
    #[test]
    fn the_door_hands_the_state_back() {
        let answered: Value = serde_json::from_str(&read_json(
            r#"{"floor":4,"applied":1}"#,
            &serde_json::to_string(&event("clips", 7, 2, 1, 4)).expect("JSON"),
        ))
        .expect("JSON");
        assert_eq!(answered["turn"]["turn"], json!("stale"));
        assert_eq!(answered["state"]["floor"], json!(4));

        let silent: Value = serde_json::from_str(&answer_json(r#"{"seq":0}"#)).expect("JSON");
        assert_eq!(silent["answer"], json!("silent"));
    }
}
