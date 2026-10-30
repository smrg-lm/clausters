//! **A roll's coordinator**: every action of a notes member opened over a
//! box's sequence -- a gesture, the space bar, a close -- carried out in the
//! application that holds its engine.
//!
//! The roll plays as a client's notes editor does: its sequence on an event
//! lane of a transport of its own ([`NotesPlayback`]), taken from the
//! engine's ids when the window opens, so its play cursor is its own and a
//! pass over it never moves the multitrack's. An edit is heard while it
//! plays, and so is an undo the multitrack's window walks over it, since the
//! two are one history.

use clausters_core::ids::IdSpaces;
use clausters_editing::apply::Medium;
use clausters_editing::notes_playback::NotesPlayback;
use serde_json::{Value, json};

use super::{Editing, Member, MemberId, Raised, Ran};
use crate::turn::{Event, Kind};

/// **One roll's playback**, and its position cursor.
#[derive(Debug)]
pub(super) struct Roll {
    playback: NotesPlayback<Box<dyn Medium + Send>>,
    rate: f64,
    /// The position cursor, a beat of the sequence: where a play starts and
    /// a stop goes back to.
    cursor: f64,
}

impl Roll {
    fn new(editing: &mut Editing) -> Option<Roll> {
        let engine = editing.engine.as_mut()?;
        Some(Roll {
            rate: engine.rate(),
            playback: NotesPlayback::in_medium(engine.medium()),
            cursor: 0.0,
        })
    }
}

/// A window's effect.
fn effect(kind: &str, fields: Value) -> Value {
    let mut value = json!({"kind": kind});
    if let (Some(map), Value::Object(more)) = (value.as_object_mut(), fields) {
        map.extend(more);
    }
    value
}

/// A pass's `range` (`[start, end]` in beats) and its loop switch.
fn pass(value: &Value) -> (Option<(f64, f64)>, bool) {
    let range = value["range"]
        .as_array()
        .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
    (range, value["looping"].as_bool().unwrap_or(false))
}

impl Editing {
    /// **A message of a roll's window** -- a notes member opened over a box's
    /// sequence -- as the editing context takes it: the turn recorded, the
    /// window answered, every other window the edit moved corrected (the
    /// multitrack's box over the same notes), the roll's own pass asked, and
    /// `notes` said when the sequence changed, for the holder to sound the
    /// multitrack again. `ids` are the engine's, which the roll's transport
    /// is taken from.
    pub(crate) fn notes_act(
        &mut self,
        member: MemberId,
        action: &Value,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut roll = match self.rolls.remove(&member) {
            Some(roll) => roll,
            None => match Roll::new(self) {
                Some(roll) => roll,
                None => {
                    return Ran {
                        end: Some(Err(Raised {
                            kind: "runtime".into(),
                            message: "the roll has no engine".into(),
                        })),
                        ..Ran::default()
                    };
                }
            },
        };
        let mut ran = Ran::default();
        let open = self.roll_run(member, &mut roll, action, ids, &mut ran);
        if open {
            self.rolls.insert(member, roll);
        } else {
            let _ = roll.playback.close(ids);
            self.unwindowed.insert(member);
        }
        ran
    }

    /// Carries `action` out on `roll`; answers whether the roll stays open.
    fn roll_run(
        &mut self,
        member: MemberId,
        roll: &mut Roll,
        action: &Value,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> bool {
        match action["verb"].as_str().unwrap_or_default() {
            "event" => {
                let event = Event {
                    addr: action["addr"].as_str().unwrap_or_default().to_string(),
                    args: action["args"].as_array().cloned().unwrap_or_default(),
                };
                let Some(turned) = self.event(member, &event) else {
                    ran.end = Some(Ok(json!(false)));
                    return true;
                };
                let corrections = turned
                    .corrections
                    .iter()
                    .chain(turned.stepped.iter().flat_map(|s| &s.corrections));
                for corrected in corrections {
                    ran.effects
                        .push(effect("echo", json!({"answer": corrected.answer})));
                }
                let super::Outcome::Notes(outcome) = turned.outcome else {
                    ran.end = Some(Ok(json!(false)));
                    return true;
                };
                if outcome.turn == Kind::Closed {
                    ran.effects.push(effect("closedWindow", json!({})));
                    ran.end = Some(Ok(json!(false)));
                    return false;
                }
                let stepped = turned.stepped.as_ref().is_some_and(|s| s.stepped);
                if stepped {
                    ran.effects.push(effect("resync", json!({})));
                }
                ran.effects
                    .push(effect("echo", json!({"answer": outcome.answer})));
                if outcome.changed || stepped {
                    ran.effects.push(effect("notes", json!({})));
                    self.roll_update(member, roll, ids);
                }
                if outcome.locate.is_some() || outcome.play.is_some() {
                    // whether it rolls is the engine's: a pass may have ended
                    // on its own
                    self.roll_rolling(roll);
                }
                // **a cursor placed moves the music** where it plays
                if let Some(beat) = outcome.locate {
                    roll.cursor = beat;
                    self.roll_with(member, |sequence| {
                        roll.playback.place(sequence, beat, roll.rate)
                    });
                }
                if let Some(asked) = &outcome.play {
                    let (range, looping) = pass(asked);
                    let (cursor, rate) = (roll.cursor, roll.rate);
                    self.roll_with(member, |sequence| {
                        if roll.playback.rolling() {
                            roll.playback.stop(sequence, cursor, rate)
                        } else {
                            roll.playback
                                .play_pass(sequence, cursor, range, looping, rate, ids)
                                .unwrap_or_default()
                        }
                    });
                }
                if let Some(asked) = &outcome.relooped {
                    let (range, looping) = pass(asked);
                    self.roll_with(member, |sequence| {
                        roll.playback.set_loop(sequence, range, looping, roll.rate)
                    });
                }
                ran.end = Some(Ok(json!(outcome.changed)));
                true
            }
            "closed" | "close" => {
                ran.effects.push(effect("closedWindow", json!({})));
                ran.end = Some(Ok(json!(false)));
                false
            }
            // the transport its play cursor reads, taken now
            "open" => {
                match roll.playback.alloc_transport(ids) {
                    Ok(transport) => {
                        ran.effects
                            .push(effect("transport", json!({"transport": transport})));
                    }
                    Err(e) => ran.effects.push(effect(
                        "say",
                        json!({"reason": format!("the roll cannot be played: {e}")}),
                    )),
                }
                ran.end = Some(Ok(Value::Null));
                true
            }
            other => {
                ran.end = Some(Err(Raised {
                    kind: "value".into(),
                    message: format!("no roll verb {other:?}"),
                }));
                true
            }
        }
    }

    /// Runs `f` over the sequence of notes member `member`, locked; what it
    /// answers is carried out on the engine as it was made.
    fn roll_with<T>(
        &self,
        member: MemberId,
        f: impl FnOnce(&clausters_document::EventSequence) -> T,
    ) -> Option<T> {
        let Some(Member::Notes(editor)) = self.member(member) else {
            return None;
        };
        let shared = editor.sequence().clone();
        let sequence = shared.lock().unwrap_or_else(|e| e.into_inner());
        Some(f(&sequence))
    }

    /// Whether the roll's transport rolls, as the engine says now.
    fn roll_rolling(&self, roll: &mut Roll) {
        let rolling = roll.playback.transport().is_some_and(|transport| {
            self.engine
                .as_ref()
                .is_some_and(|engine| engine.rolling(transport))
        });
        roll.playback.set_rolling(rolling);
    }

    /// The roll's lane takes its sequence again, as it now is.
    fn roll_update(&self, member: MemberId, roll: &mut Roll, ids: &mut IdSpaces) {
        let rate = roll.rate;
        self.roll_with(member, |sequence| roll.playback.update(sequence, rate, ids));
    }

    /// **Every open roll hears its sequence again**: a step the multitrack's
    /// window walked may have been a roll's edit.
    pub(super) fn sound_rolls(&mut self, ids: &mut IdSpaces) {
        let mut rolls = std::mem::take(&mut self.rolls);
        for (member, roll) in &mut rolls {
            self.roll_update(*member, roll, ids);
        }
        self.rolls = rolls;
    }

    /// **What a script reads of a roll**: `transport` (its transport, once
    /// its window opened, and whether it rolls).
    pub(crate) fn notes_query(&self, member: MemberId, query: &str) -> Value {
        match (query, self.rolls.get(&member)) {
            ("transport", Some(roll)) => json!({
                "transport": roll.playback.transport(),
                "rolling": roll.playback.rolling(),
            }),
            ("transport", None) => json!({"error": "the roll has no transport"}),
            _ => json!({"error": format!("no roll query {query:?}")}),
        }
    }

    /// **A roll is gone**: its lane, its graphs and its transport. Its
    /// member stays for the history and is taken again by the next roll over
    /// its sequence ([`Editing::open_notes`]).
    pub fn forget_roll(&mut self, member: MemberId, ids: &mut IdSpaces) {
        if let Some(mut roll) = self.rolls.remove(&member) {
            let _ = roll.playback.close(ids);
        }
        if matches!(self.member(member), Some(Member::Notes(_))) {
            self.unwindowed.insert(member);
        }
    }
}
