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
                    ran.effects.push(self.roll_closing(member, "closedWindow"));
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
                    self.sound_rolls(ids);
                }
                // **File > Close**, by the rule every window closes by: a roll
                // edits what its multitrack holds, so nothing is asked
                if outcome.close {
                    ran.effects.push(self.roll_closing(member, "closeWindow"));
                    ran.end = Some(Ok(json!(false)));
                    return false;
                }
                self.roll_pass(
                    member,
                    roll,
                    outcome.locate,
                    outcome.play.as_ref(),
                    outcome.relooped.as_ref(),
                    ids,
                );
                ran.end = Some(Ok(json!(outcome.changed)));
                true
            }
            "closed" => {
                ran.effects.push(self.roll_closing(member, "closedWindow"));
                ran.end = Some(Ok(json!(false)));
                false
            }
            "close" => {
                ran.effects.push(self.roll_closing(member, "closeWindow"));
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
                ran.end = Some(
                    match self.handler_verb(member, roll, other, action, ids, ran) {
                        Some(answer) => answer.map_err(|message| Raised {
                            kind: "command".into(),
                            message,
                        }),
                        None => Err(Raised {
                            kind: "value".into(),
                            message: format!("no roll verb {other:?}"),
                        }),
                    },
                );
                true
            }
        }
    }

    /// **A handler's verb, a roll's and a page's alike**: the transport as a
    /// script plays it, the history its window walks, a setting of the
    /// window's, and one verb of the sequence's handle while the window has
    /// it. `None` for a verb that is none of these.
    fn handler_verb(
        &mut self,
        member: MemberId,
        roll: &mut Roll,
        verb: &str,
        action: &Value,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Option<Result<Value, String>> {
        let rate = roll.rate;
        Some(match verb {
            "playing" => {
                self.roll_rolling(roll);
                Ok(json!(roll.playback.rolling()))
            }
            "play" => {
                if let Some(beat) = action["beat"].as_f64() {
                    roll.cursor = beat;
                }
                let (range, looping) = pass(action);
                let cursor = roll.cursor;
                self.roll_with(member, |sequence| {
                    roll.playback
                        .play_pass(sequence, cursor, range, looping, rate, ids)
                        .unwrap_or_default()
                });
                Ok(Value::Null)
            }
            "pause" => {
                roll.playback.pause();
                Ok(Value::Null)
            }
            "resume" => {
                roll.playback.resume();
                Ok(Value::Null)
            }
            "stop" => {
                self.roll_rolling(roll);
                let back = action["mark"].as_f64().unwrap_or(roll.cursor);
                roll.cursor = back;
                self.roll_with(member, |sequence| roll.playback.stop(sequence, back, rate));
                Ok(Value::Null)
            }
            "locate" => match action["at"].as_f64() {
                Some(at) => {
                    roll.cursor = at;
                    self.roll_with(member, |sequence| roll.playback.place(sequence, at, rate));
                    Ok(Value::Null)
                }
                None => Err("a locate names where: at".into()),
            },
            "loop" => {
                let (range, looping) = pass(action);
                self.roll_with(member, |sequence| {
                    roll.playback.set_loop(sequence, range, looping, rate)
                });
                Ok(Value::Null)
            }
            "end" => match clausters_editing::playback::End::from_json(&action["end"]) {
                Some(end) => {
                    self.roll_with(member, |sequence| {
                        roll.playback.set_end(end, sequence, rate)
                    });
                    Ok(Value::Null)
                }
                None => Err(format!("a pass ends nowhere like {}", action["end"])),
            },
            "undo" | "redo" => {
                let direction = if verb == "undo" {
                    clausters_document::history::Direction::Undo
                } else {
                    clausters_document::history::Direction::Redo
                };
                let before = self.version;
                self.select(Some(member));
                let stepped = self.walk_history(direction);
                for corrected in &stepped.corrections {
                    ran.effects
                        .push(effect("echo", json!({"answer": corrected.answer})));
                }
                let moved = self.version != before;
                if moved {
                    self.roll_update(member, roll, ids);
                    self.sound_rolls(ids);
                    ran.effects.push(effect("resync", json!({})));
                }
                Ok(json!(moved))
            }
            // **a setting of the window's**, a member's verb that then redraws
            // it: its grid, its value in hand, what is selected
            "set" => {
                let answer = self.member_text(member, &action["call"]);
                ran.effects.push(effect("resync", json!({})));
                Ok(answer)
            }
            "handle" => self.sequence_handle(member, roll, action, ids, ran),
            _ => return None,
        })
    }

    /// One verb of the member's own door, its text read.
    fn member_text(&mut self, member: MemberId, call: &Value) -> Value {
        let text = call.to_string();
        let answer = match self.member_mut(member) {
            Some(Member::Notes(editor)) => crate::notes::editor::call_json(editor, &text),
            #[cfg(feature = "notation")]
            Some(Member::Score(editor)) => crate::score::editor::call_json(editor, &text),
            _ => "{}".into(),
        };
        serde_json::from_str(&answer).unwrap_or(Value::Null)
    }

    /// The sequence member `member` shows: a roll's, or that of a page opened
    /// over one.
    fn shown_sequence(&self, member: MemberId) -> Option<crate::notes::Shared> {
        match self.member(member) {
            Some(Member::Notes(editor)) => Some(editor.sequence().clone()),
            #[cfg(feature = "notation")]
            Some(Member::Score(editor)) => editor.sequence().cloned(),
            _ => None,
        }
    }

    /// **A script's call on the sequence this window has**: `call` is one
    /// verb of the sequence's handle (`clausters_document::events::call_json`),
    /// answered as the handle answers it. A change is applied, heard and
    /// drawn in every window over the sequence; a `record` call (`legs`,
    /// each `{"forward", "backward"}`) makes the changes a script made one
    /// entry, named `label`, of the history its windows walk.
    fn sequence_handle(
        &mut self,
        member: MemberId,
        roll: &mut Roll,
        action: &Value,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        use crate::turn::{Leg, Record};
        let call = &action["call"];
        if call["verb"] == "record" {
            let legs: Vec<Leg> = call["legs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|leg| Leg {
                    holds_forward: Vec::new(),
                    holds_backward: Vec::new(),
                    key: String::new(),
                    forward: json!({"edit": leg["forward"]}),
                    backward: leg["backward"].clone(),
                })
                .collect();
            let label = action["label"].as_str().unwrap_or("edit").to_string();
            let taken = !legs.is_empty() && self.record(member, &Record { label, legs }, false);
            return Ok(json!(taken));
        }
        let shared = self
            .shown_sequence(member)
            .ok_or("this window shows no sequence")?;
        let request = call.to_string();
        let mutates = clausters_document::events::mutates(&request);
        let answer = {
            let mut sequence = shared.lock().unwrap_or_else(|e| e.into_inner());
            clausters_document::events::call_json(&mut sequence, &request)
        };
        let answer: Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
        if let Some(error) = answer.get("error").and_then(Value::as_str) {
            return Err(error.to_string());
        }
        if mutates && answer["applied"] != false {
            self.moved();
            self.roll_update(member, roll, ids);
            self.sound_rolls(ids);
            ran.effects.push(effect("resync", json!({})));
            ran.effects.push(effect("notes", json!({})));
        }
        Ok(answer)
    }

    /// **The window's closing effect**, carrying what its member leaves: what
    /// a script that held it goes on holding.
    fn roll_closing(&self, member: MemberId, kind: &str) -> Value {
        let left = match self.member(member) {
            Some(Member::Notes(editor)) => {
                let sequence = editor.sequence().lock().unwrap_or_else(|e| e.into_inner());
                json!({"sequence": &*sequence})
            }
            #[cfg(feature = "notation")]
            Some(Member::Score(editor)) => {
                let mei = editor
                    .score()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .mei();
                let sequence = editor
                    .sequence()
                    .map(|shared| json!(&*shared.lock().unwrap_or_else(|e| e.into_inner())));
                json!({"mei": mei, "path": editor.file_path(), "sequence": sequence})
            }
            _ => Value::Null,
        };
        effect(kind, json!({"left": left}))
    }

    /// **What a turn asked of the pass**, a roll's or a page's alike: the
    /// cursor placed, a play or a stop, the loop switch.
    fn roll_pass(
        &self,
        member: MemberId,
        roll: &mut Roll,
        locate: Option<f64>,
        play: Option<&Value>,
        relooped: Option<&Value>,
        ids: &mut IdSpaces,
    ) {
        if locate.is_some() || play.is_some() {
            // whether it rolls is the engine's: a pass may have ended on its
            // own
            self.roll_rolling(roll);
        }
        // **a cursor placed moves the music** where it plays
        if let Some(beat) = locate {
            roll.cursor = beat;
            self.roll_with(member, |sequence| {
                roll.playback.place(sequence, beat, roll.rate)
            });
        }
        if let Some(asked) = play {
            // a page plays from where its selection starts, which it names
            if let Some(from) = asked["from"].as_f64() {
                roll.cursor = from;
            }
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
        if let Some(asked) = relooped {
            let (range, looping) = pass(asked);
            self.roll_with(member, |sequence| {
                roll.playback.set_loop(sequence, range, looping, roll.rate)
            });
        }
    }

    /// Runs `f` over what member `member` plays: a roll's sequence, locked,
    /// or a page's score rendered at the engraver's tempo. What it answers is
    /// carried out on the engine as it was made.
    fn roll_with<T>(
        &self,
        member: MemberId,
        f: impl FnOnce(&clausters_document::EventSequence) -> T,
    ) -> Option<T> {
        match self.member(member) {
            Some(Member::Notes(editor)) => {
                let shared = editor.sequence().clone();
                let sequence = shared.lock().unwrap_or_else(|e| e.into_inner());
                Some(f(&sequence))
            }
            #[cfg(feature = "notation")]
            Some(Member::Score(editor)) => {
                let sequence = editor
                    .rendered()
                    .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()))
                    .ok()?;
                Some(f(&sequence))
            }
            _ => None,
        }
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
            ("transport", None) => json!({"error": "it has no transport"}),
            _ => json!({"error": format!("no roll query {query:?}")}),
        }
    }

    /// **The undo order member `member`'s object is in closes**: nothing
    /// walks it again, as when the last window a client opened over a sequence
    /// or a symbolic score goes. The next window over them starts a new one.
    pub fn close_order_of(&mut self, member: MemberId) {
        if let Some(seat) = self.seats.get(member as usize) {
            let id = super::object_id(&seat.member);
            self.services
                .histories
                .close_order(crate::application::histories::Object { member, id });
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

#[cfg(feature = "notation")]
impl Editing {
    /// **An action of a score member in the application**: a gesture, a
    /// handler's verb, or the file and the window. The page plays as a roll
    /// plays, on a transport of its own, its score rendered; the file goes
    /// through the storage the application reads and writes, disk or a page's.
    pub(crate) fn score_act(
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
                            message: "the score has no engine".into(),
                        })),
                        ..Ran::default()
                    };
                }
            },
        };
        let mut ran = Ran::default();
        if self.score_run(member, &mut roll, action, ids, &mut ran) {
            self.rolls.insert(member, roll);
        } else {
            let _ = roll.playback.close(ids);
        }
        ran
    }

    /// Carries `action` out; answers whether the window stays.
    fn score_run(
        &mut self,
        member: MemberId,
        roll: &mut Roll,
        action: &Value,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> bool {
        let turned = match action["verb"].as_str().unwrap_or_default() {
            "event" => self.event(
                member,
                &Event {
                    addr: action["addr"].as_str().unwrap_or_default().to_string(),
                    args: action["args"].as_array().cloned().unwrap_or_default(),
                },
            ),
            // a handler's verb over what is selected: one entry of the history
            "act" => self.act(member, &action["call"]),
            "closed" => {
                ran.effects.push(self.roll_closing(member, "closedWindow"));
                ran.end = Some(Ok(json!(false)));
                return false;
            }
            "close" => {
                ran.effects.push(self.roll_closing(member, "closeWindow"));
                ran.end = Some(Ok(json!(false)));
                return false;
            }
            "page" => {
                ran.end = Some(self.score_page(member, roll, action, ids, ran).map_err(
                    |message| Raised {
                        kind: "command".into(),
                        message,
                    },
                ));
                return true;
            }
            "open" => {
                match roll.playback.alloc_transport(ids) {
                    Ok(transport) => ran
                        .effects
                        .push(effect("transport", json!({"transport": transport}))),
                    Err(e) => ran.effects.push(effect(
                        "say",
                        json!({"reason": format!("the score cannot be played: {e}")}),
                    )),
                }
                ran.end = Some(Ok(Value::Null));
                return true;
            }
            "save" => {
                let path = action["path"].as_str().map(str::to_string);
                ran.end = Some(self.score_save(member, path).map_err(|message| Raised {
                    kind: "command".into(),
                    message,
                }));
                return true;
            }
            "load" => {
                let path = action["path"].as_str().unwrap_or_default().to_string();
                match self.score_read(member, &path, roll, ids, ran) {
                    Ok(changed) => ran.end = Some(Ok(json!(changed))),
                    Err(message) => {
                        ran.end = Some(Err(Raised {
                            kind: "command".into(),
                            message,
                        }));
                    }
                }
                return true;
            }
            other => {
                ran.end = Some(
                    match self.handler_verb(member, roll, other, action, ids, ran) {
                        Some(answer) => answer.map_err(|message| Raised {
                            kind: "command".into(),
                            message,
                        }),
                        None => Err(Raised {
                            kind: "value".into(),
                            message: format!("no score verb {other:?}"),
                        }),
                    },
                );
                return true;
            }
        };
        let Some(turned) = turned else {
            ran.end = Some(Ok(json!(false)));
            return true;
        };
        for corrected in turned
            .corrections
            .iter()
            .chain(turned.stepped.iter().flat_map(|s| &s.corrections))
        {
            ran.effects
                .push(effect("echo", json!({"answer": corrected.answer})));
        }
        let stepped = turned.stepped.as_ref().is_some_and(|s| s.stepped);
        let super::Outcome::Score(outcome) = turned.outcome else {
            ran.end = Some(Ok(json!(false)));
            return true;
        };
        if outcome.turn == Kind::Closed {
            ran.effects.push(self.roll_closing(member, "closedWindow"));
            ran.end = Some(Ok(json!(false)));
            return false;
        }
        if stepped {
            ran.effects.push(effect("resync", json!({})));
        }
        ran.effects
            .push(effect("echo", json!({"answer": outcome.answer})));
        if outcome.changed || stepped {
            self.roll_update(member, roll, ids);
            self.sound_rolls(ids);
        }
        // **a close waits for the file it saves to**, and stays when it was
        // not written
        let written = match &outcome.save {
            Some(path) => match self.score_save(member, Some(path.clone())) {
                Ok(_) => true,
                Err(why) => {
                    ran.effects.push(effect("say", json!({"reason": why})));
                    false
                }
            },
            None => true,
        };
        if outcome.close && written {
            ran.effects.push(self.roll_closing(member, "closeWindow"));
            ran.end = Some(Ok(json!(false)));
            return false;
        }
        if let Some(path) = &outcome.open
            && let Err(why) = self.score_read(member, path, roll, ids, ran)
        {
            ran.effects.push(effect("say", json!({"reason": why})));
        }
        // the writer of a MIDI file is the holder's, which links it
        if let Some(export) = &outcome.export {
            match self.score_rendered(member) {
                Ok(sequence) => ran.effects.push(effect(
                    "export",
                    json!({"path": export["path"], "format": export["format"],
                        "sequence": sequence}),
                )),
                Err(why) => ran.effects.push(effect("say", json!({"reason": why}))),
            }
        }
        self.roll_pass(
            member,
            roll,
            outcome.locate,
            outcome.play.as_ref(),
            outcome.relooped.as_ref(),
            ids,
        );
        ran.end = Some(Ok(json!({
            "changed": outcome.changed,
            "refused": outcome.refused,
            "selected": outcome.selected,
        })));
        true
    }

    /// **A script's call on the document this window has**, while it has it:
    /// its reads (`mei`, `sheet`, `display_list`) and its edits, each one
    /// entry of the history -- `apply` and `load` the editor's own verbs,
    /// `transpose`, `transpose_to` and `edit` the MEI they leave. A page over
    /// a sequence is edited by the editor's verbs, which write the sequence.
    fn score_page(
        &mut self,
        member: MemberId,
        roll: &mut Roll,
        action: &Value,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        use crate::turn::{Leg, Record};
        let call = &action["call"];
        let verb = call["verb"].as_str().unwrap_or_default();
        let (shared, over) = match self.member(member) {
            Some(Member::Score(editor)) => (editor.score().clone(), editor.is_over()),
            _ => return Err("the member is not a score".into()),
        };
        let edited = match verb {
            "mei" => {
                let score = shared.lock().unwrap_or_else(|e| e.into_inner());
                return Ok(json!({"mei": score.mei()}));
            }
            "sheet" => {
                let score = shared.lock().unwrap_or_else(|e| e.into_inner());
                return score
                    .sheet()
                    .map(|sheet| json!({"sheet": sheet}))
                    .ok_or_else(|| "the document could not be read into a model".to_string());
            }
            "display_list" => {
                let mut score = shared.lock().unwrap_or_else(|e| e.into_inner());
                let page = call["page"].as_i64().unwrap_or(1) as i32;
                return serde_json::to_value(score.display_list(page)).map_err(|e| e.to_string());
            }
            "apply" | "load" => {
                let asked = if verb == "apply" {
                    json!({"action": "op", "op": call["op"]})
                } else {
                    json!({"action": "open", "data": call["mei"]})
                };
                let turned = self
                    .act(member, &asked)
                    .ok_or("the member is not a score")?;
                for corrected in &turned.corrections {
                    ran.effects
                        .push(effect("echo", json!({"answer": corrected.answer})));
                }
                let super::Outcome::Score(outcome) = turned.outcome else {
                    return Err("the member is not a score".into());
                };
                ran.effects
                    .push(effect("echo", json!({"answer": outcome.answer})));
                if outcome.changed {
                    self.roll_update(member, roll, ids);
                    self.sound_rolls(ids);
                }
                return Ok(json!(outcome.changed));
            }
            "transpose" | "transpose_to" | "edit" if over => {
                return Err("a page over a sequence is edited by its editor's verbs".into());
            }
            "transpose" | "transpose_to" | "edit" => {
                let mut score = shared.lock().unwrap_or_else(|e| e.into_inner());
                let before = score.mei();
                let id = call["id"].as_str().unwrap_or_default();
                let done = match verb {
                    "transpose" => score.transpose(id, call["steps"].as_i64().unwrap_or(0) as i32),
                    "transpose_to" => score.transpose_to(
                        id,
                        call["position"].as_i64().unwrap_or(0) as i32,
                        call["page"].as_i64().unwrap_or(1) as i32,
                    ),
                    _ => score.edit(
                        call["action"].as_str().unwrap_or_default(),
                        &call["param"].to_string(),
                    ),
                };
                if !done {
                    return Ok(json!(false));
                }
                let after = score.mei();
                (after != before).then_some((before, after))
            }
            other => return Err(format!("no verb {other:?} of a score")),
        };
        if let Some((before, after)) = edited {
            let label = call["label"].as_str().unwrap_or(verb).to_string();
            let leg = Leg {
                holds_forward: Vec::new(),
                holds_backward: Vec::new(),
                key: String::new(),
                forward: json!({"edit": {"mei": after}}),
                backward: json!({"mei": before}),
            };
            self.record(
                member,
                &Record {
                    label,
                    legs: vec![leg],
                },
                false,
            );
            self.roll_update(member, roll, ids);
            ran.effects.push(effect("resync", json!({})));
        }
        Ok(json!(true))
    }

    /// The score as the sequence it plays as, as JSON.
    fn score_rendered(&self, member: MemberId) -> Result<Value, String> {
        match self.member(member) {
            Some(Member::Score(editor)) => editor.rendered(),
            _ => Err("the member is not a score".into()),
        }
    }

    /// **The score written as MEI**, at `path` or at the file it has, which
    /// is its from then on; its changes are then not what a close asks about.
    /// Answers the path. A write that fails changes nothing.
    fn score_save(&mut self, member: MemberId, path: Option<String>) -> Result<Value, String> {
        let Some(Member::Score(editor)) = self.member_mut(member) else {
            return Err("the member is not a score".into());
        };
        let path = path
            .or_else(|| editor.file_path())
            .ok_or("the score has no file yet: save it as")?;
        let mei = editor
            .score()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mei();
        clausters_core::files::write(&path, mei).map_err(|e| format!("{path}: {e}"))?;
        crate::score::editor::call_json(editor, &json!({"verb": "sync", "path": path}).to_string());
        crate::score::editor::call_json(editor, r#"{"verb": "saved"}"#);
        Ok(json!({"destination": path}))
    }

    /// **The document at `path`, read in place of the score** as one entry
    /// of the history, the score there a step back, and heard. A read that
    /// fails leaves the score as it is. Whether it changed.
    fn score_read(
        &mut self,
        member: MemberId,
        path: &str,
        roll: &mut Roll,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Result<bool, String> {
        let bytes = clausters_core::files::read(path).map_err(|e| format!("{path}: {e}"))?;
        let data = String::from_utf8(bytes).map_err(|e| format!("{path}: {e}"))?;
        let turned = self
            .act(member, &json!({"action": "open", "data": data}))
            .ok_or("the member is not a score")?;
        for corrected in &turned.corrections {
            ran.effects
                .push(effect("echo", json!({"answer": corrected.answer})));
        }
        let super::Outcome::Score(outcome) = turned.outcome else {
            return Err("the member is not a score".into());
        };
        ran.effects
            .push(effect("echo", json!({"answer": outcome.answer})));
        if let Some(why) = outcome.refused {
            return Err(why);
        }
        if outcome.changed {
            self.roll_update(member, roll, ids);
            if let Some(Member::Score(editor)) = self.member_mut(member) {
                crate::score::editor::call_json(
                    editor,
                    &json!({"verb": "sync", "path": path}).to_string(),
                );
            }
        }
        Ok(outcome.changed)
    }
}
