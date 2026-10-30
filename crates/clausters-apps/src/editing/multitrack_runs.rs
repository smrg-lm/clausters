//! **The multitrack's coordinator**: every action of a multitrack member --
//! a gesture, a script's verb, the transport, a close -- carried out in the
//! application that holds its engine, and answered as the window's effects.
//!
//! The audio editor's coordinator ([`super::coordinator`]) runs its actions
//! as runs that stop on what only the caller does, a render above all. A
//! multitrack renders nothing: an edit moves where things are and never the
//! samples they read, so every action here ends where it starts, and the one
//! thing it hands the engine is each track's take ([`HeldTake`]), read by
//! the application into the track's fed buffers. An edit of a track's
//! regions hands the track's take over again and makes no node.
//!
//! The vocabulary is the audio editor's: the window's effects are `echo`
//! (an answer to the window), `set`, `publish`, `resync` (every widget drawn
//! again, after a history step), `closeWindow` and `closedWindow`; and
//! `open` (a box of notes asked for its roll, `source`) and `say` (a
//! `reason` the window shows: a save that failed).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use clausters_core::ids::IdSpaces;
use clausters_document::NodeId;
use clausters_document::multitrack::nodes::PlannedFeed;
use clausters_document::multitrack::read::{PlacedRegion, track_reads};
use clausters_editing::apply::Medium;
use clausters_editing::playback::{MultitrackPlayback, View};
use serde_json::{Value, json};

use super::held::{HeldList, HeldTake, PlacedList};
use super::{Caller, Editing, Member, MemberId, Raised, Ran};
use crate::multitrack::editor::{Kind, TransportVerb};
use crate::turn::Event;

/// **One multitrack member's playback**, and what its tracks' readers were
/// last handed.
#[derive(Debug)]
pub(super) struct Tracks {
    playback: MultitrackPlayback<Box<dyn Medium + Send>>,
    rate: f64,
    /// Each track's regions as last handed to the engine, and the buffers
    /// its readers play.
    taken: HashMap<NodeId, (Vec<PlacedRegion>, PlannedFeed)>,
    revision: u64,
    /// The session it was opened from, and where it is: what a save
    /// writes again, with the multitrack as it now is.
    pub(super) session: Option<(clausters_document::session::Session, String)>,
    /// **Where a Save with no path writes**, when the holder said
    /// (`saveTo` on the open): `Some(None)` writes nowhere. Unsaid, it
    /// writes the session it opened.
    pub(super) save_to: Option<Option<String>>,
    /// Captured contents confirmed by the last open or save, independent of
    /// navigation and the editing context's conversation version.
    pub(super) saved: Option<clausters_document::session::Session>,
}

impl Tracks {
    /// **What a pass is read off**, in the playback's samples: a selection
    /// `range` and a position `cursor`, in seconds.
    fn view(&self, range: Option<(f64, f64)>, cursor: Option<f64>) -> View {
        View {
            selection: range.map(|(from, to)| (self.frame(from), self.frame(to.max(from)))),
            cursor: cursor.map(|secs| self.frame(secs)),
        }
    }

    /// A second of the multitrack as the playback's sample.
    fn frame(&self, secs: f64) -> u64 {
        self.playback.secs_to_samples(secs) as u64
    }

    /// Whether a pass that is not looped stops where the contents end.
    pub(super) fn stops_at_end(&self) -> bool {
        self.playback.end() == clausters_editing::playback::End::Contents
    }

    fn new(editing: &mut Editing) -> Option<Tracks> {
        let engine = editing.engine.as_mut()?;
        Some(Tracks {
            rate: engine.rate(),
            playback: MultitrackPlayback::in_medium(engine.medium()),
            taken: HashMap::new(),
            revision: 0,
            session: None,
            save_to: None,
            saved: None,
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

fn refused(kind: &str, message: impl Into<String>) -> Ran {
    Ran {
        end: Some(Err(Raised {
            kind: kind.into(),
            message: message.into(),
        })),
        ..Ran::default()
    }
}

impl Editing {
    /// Start an action in this member's editing order. Session storage and
    /// measurements may wait on their identified operation; edits and
    /// transport use the application's engine and ids.
    pub(crate) fn multitrack_act(
        &mut self,
        member: MemberId,
        action: &Value,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        self.select_member(member);
        let mut tracks = match self.tracks.remove(&member) {
            Some(tracks) => tracks,
            None => match Tracks::new(self) {
                Some(tracks) => tracks,
                None => return refused("runtime", "the multitrack has no engine"),
            },
        };
        let ran = self.multitrack_run(member, &mut tracks, action, caller, ids);
        self.tracks.insert(member, tracks);
        ran
    }

    /// **A session opened over member `member`**: read from `path` through
    /// the file layer, its multitrack put in the editor, its takes' headers
    /// read through `caller`, its joins known, and the peaks it has not got
    /// measured by a render handed back. A source that is not there is
    /// said, and its boxes are silent.
    fn open_session(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        path: &str,
        caller: &mut dyn Caller,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        use crate::multitrack::sources;
        let bytes = clausters_core::files::read(path).map_err(|e| format!("{path}: {e}"))?;
        let session: clausters_document::session::Session =
            serde_json::from_slice(&bytes).map_err(|e| format!("{path}: not a session: {e}"))?;
        let folder = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut resolved = sources::resolve(&session, &folder, &mut |file| caller.header(file));
        if !resolved.measure.is_empty() {
            let _ =
                clausters_core::files::create_dir_all(sources::located(&folder, sources::PEAKS));
        }
        let unresolved: Vec<Value> = resolved
            .unresolved
            .iter()
            .map(|(id, why)| json!({"source": id.0, "reason": why}))
            .collect();
        let measured = std::mem::take(&mut resolved.measure);
        let seat = self
            .seats
            .get_mut(member as usize)
            .ok_or("the member is absent")?;
        let Member::Multitrack(editor) = &mut seat.member else {
            return Err("the member is not a multitrack".into());
        };
        let ticket = self.multitrack_runtime.prepare(
            editor.as_ref(),
            &mut self.services,
            crate::multitrack::operations::Request {
                target: crate::application::operations::Target {
                    member,
                    object: editor.session_binding(),
                    server: seat.server,
                },
                command: crate::multitrack::operations::Command::Open(Box::new(
                    crate::multitrack::operations::Opened {
                        session: session.clone(),
                        resolved,
                    },
                )),
            },
        )?;
        let finished = self
            .multitrack_runtime
            .operations
            .finish_ready(ticket)
            .expect("an open has no pending effects");
        let settled = self.multitrack_runtime.settle_turn(
            editor.as_mut(),
            &mut self.services,
            finished,
            &mut self.version,
        );
        if settled.protocol.committed != Some(true) {
            return Err("the session changed before opening".into());
        }
        if !measured.is_empty() {
            self.measure_multitrack(member, measured, ran)?;
        }
        tracks.session = Some((session, path.to_string()));
        tracks.saved = self.multitrack_snapshot(member, tracks);
        Ok(json!({"unresolved": unresolved}))
    }

    /// **The session written** at `path`, or where it was opened from: the
    /// multitrack as it now is, its takes and its joins in the table, every
    /// file inside the session's folder named relative to it. A multitrack
    /// opened over no session asks for a path.
    fn save_session(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        save: super::multitrack_operations::SaveTo,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        let Some(super::Seat {
            member: Member::Multitrack(editor),
            ..
        }) = self.seats.get(member as usize)
        else {
            return Err("the member is not a multitrack".into());
        };
        let path = match (
            save.path.as_deref(),
            &tracks.save_to,
            tracks.session.as_ref(),
        ) {
            (Some(path), _, _) => path.to_string(),
            (None, Some(Some(path)), _) => path.clone(),
            (None, Some(None), _) => {
                return Err("this session is read-only: it was opened with nowhere to save".into());
            }
            (None, None, Some((_, path))) => path.clone(),
            (None, None, None) => return Err("the session has no file yet: save it as".into()),
        };
        let session = editor.session_snapshot(tracks.session.as_ref().map(|(session, _)| session));
        let target = crate::application::operations::Target {
            member,
            object: editor.session_binding(),
            server: self.seats[member as usize].server,
        };
        let saved = crate::multitrack::operations::Saved {
            session,
            path: path.clone(),
            close: save.close,
            save_as: save.save_as,
        };
        let ticket = self.multitrack_runtime.prepare(
            editor.as_ref(),
            &mut self.services,
            crate::multitrack::operations::Request {
                target,
                command: crate::multitrack::operations::Command::Save(Box::new(saved)),
            },
        )?;
        let dispatch = self.multitrack_runtime.next(ticket);
        let crate::application::runtime::Dispatch::Response(protocol) = dispatch else {
            unreachable!("a session save waits on its storage batch");
        };
        self.wait_multitrack_operation(protocol.batch.expect("a storage batch"), ran);
        Ok(json!({"destination": path}))
    }

    fn multitrack_snapshot(
        &self,
        member: MemberId,
        tracks: &Tracks,
    ) -> Option<clausters_document::session::Session> {
        let Member::Multitrack(editor) = self.member(member)? else {
            return None;
        };
        Some(editor.session_snapshot(tracks.session.as_ref().map(|(session, _)| session)))
    }

    fn multitrack_run(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        action: &Value,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        let verb = action["verb"].as_str().unwrap_or_default();
        if action.get("saveTo").is_some() {
            tracks.save_to = Some(action["saveTo"].as_str().map(str::to_string));
        }
        let result = match verb {
            "open" if action["session"].is_string() => {
                let path = action["session"].as_str().unwrap_or_default().to_string();
                let opened = self.open_session(member, tracks, &path, caller, &mut ran);
                if opened.is_ok() {
                    self.sound(member, tracks, ids);
                    tracks.saved = self.multitrack_snapshot(member, tracks);
                }
                opened.map(|mut answer| {
                    answer["transport"] = json!(tracks.playback.transport());
                    answer
                })
            }
            "retry" => {
                self.retry_multitrack(member, caller, ids, &mut ran);
                if ran.end.is_none() {
                    ran.end = Some(Ok(Value::Null));
                }
                return ran;
            }
            "save" => self.save_session(
                member,
                tracks,
                super::multitrack_operations::SaveTo {
                    path: action["path"].as_str().map(str::to_string),
                    close: false,
                    save_as: action["path"].is_string(),
                },
                &mut ran,
            ),
            "open" | "sound" => {
                self.sound(member, tracks, ids);
                if verb == "open" {
                    tracks.saved = self.multitrack_snapshot(member, tracks);
                }
                Ok(json!({"transport": tracks.playback.transport()}))
            }
            "event" => {
                let event = Event {
                    addr: action["addr"].as_str().unwrap_or_default().to_string(),
                    args: action["args"].as_array().cloned().unwrap_or_default(),
                };
                Ok(self.multitrack_event(member, tracks, &event, caller, ids, &mut ran))
            }
            "verb" => {
                let call = action["call"].to_string();
                let version = self.version;
                let answer = match self.member_mut(member) {
                    Some(Member::Multitrack(editor)) => {
                        let mut call: Value = serde_json::from_str(&call).unwrap_or_default();
                        call["version"] = json!(version);
                        crate::multitrack::editor::call_json(editor, &call.to_string())
                    }
                    _ => return refused("value", "the member is not a multitrack"),
                };
                self.sound(member, tracks, ids);
                // what the window shows changed with it: drawn again
                if matches!(action["call"]["verb"].as_str(), Some("sync" | "notesView")) {
                    ran.effects.push(effect("resync", json!({})));
                }
                Ok(serde_json::from_str(&answer).unwrap_or(Value::Null))
            }
            // **A script's play**: over its range, or -- with none -- a
            // paused pass goes on, and otherwise one starts at the position
            // cursor, as the space bar's does
            "play" => {
                let range = action["range"]
                    .as_array()
                    .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
                if range.is_none() && tracks.playback.paused() {
                    tracks.playback.resume();
                } else {
                    let (_, cursor) = self.multitrack_view(member);
                    let view = tracks.view(range, cursor);
                    tracks.playback.play(action["looping"] == true, view);
                }
                Ok(Value::Null)
            }
            "pause" => {
                tracks.playback.pause();
                Ok(Value::Null)
            }
            // a script's stop goes back to `mark`, which is the position
            // cursor from then on: a play with no range starts there
            "stop" => {
                let mark = action["mark"].as_f64().unwrap_or(0.0);
                if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
                    editor.set_cursor(Some(mark));
                }
                let back = tracks.frame(mark);
                tracks.playback.stop(back);
                Ok(Value::Null)
            }
            // **A script's locate places the position cursor**, where a play
            // with no range starts: the head goes there while it rolls, and a
            // stopped transport stands on it
            "locate" => {
                let at = action["at"].as_f64().unwrap_or(0.0);
                if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
                    editor.set_cursor(Some(at));
                }
                let frame = tracks.frame(at);
                if tracks.playback.rolling() {
                    tracks.playback.locate(frame);
                } else {
                    tracks.playback.cue(frame);
                }
                Ok(Value::Null)
            }
            "playing" => Ok(json!(self.multitrack_rolling(tracks))),
            "handle" => self.multitrack_handle(member, tracks, action, ids, &mut ran),
            "undo" | "redo" => {
                let direction = if verb == "undo" {
                    clausters_document::history::Direction::Undo
                } else {
                    clausters_document::history::Direction::Redo
                };
                let before = self.version;
                let stepped = self
                    .prepare_audio_walk(direction)
                    .unwrap_or_else(|| self.step(direction));
                for corrected in &stepped.corrections {
                    ran.effects
                        .push(effect("echo", json!({"answer": corrected.answer})));
                }
                for freed in self.audio_carry_step(stepped, caller, ids, &mut ran) {
                    caller.delete(&freed.files);
                }
                let applied = self.version != before;
                if applied {
                    self.sound(member, tracks, ids);
                    self.sound_rolls(ids);
                    ran.effects.push(effect("resync", json!({})));
                }
                Ok(json!(applied))
            }
            "close" | "closed" => {
                self.close_tracks(member, tracks, ids);
                ran.effects.push(self.closing(
                    member,
                    if verb == "closed" {
                        "closedWindow"
                    } else {
                        "closeWindow"
                    },
                ));
                Ok(json!(false))
            }
            other => {
                return refused("value", format!("no multitrack verb {other:?}"));
            }
        };
        if !ran.render.as_ref().is_some_and(|render| {
            render
                .steps
                .iter()
                .any(|step| matches!(step, clausters_editing::apply::Step::Store { .. }))
        }) {
            ran.end = Some(result.map_err(|message: String| Raised {
                kind: "value".into(),
                message,
            }));
        }
        ran
    }

    /// **A script's call on the multitrack this member holds**, while its
    /// window has it: `call` is one verb of the multitrack's handle
    /// (`clausters_document::multitrack::handle`), answered as the handle
    /// answers it. An `apply` is an edit of the window's: recorded in the
    /// history under `label` -- or, with `record: false`, applied and left
    /// for a `record` call that makes a script's block one entry, its
    /// `legs` (`{"forward", "backward"}`) in order -- sounded, and the
    /// window drawn again.
    fn multitrack_handle(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        action: &Value,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        use crate::turn::{Leg, Record};
        use clausters_document::multitrack::handle;

        let call = &action["call"];
        let label = action["label"].as_str().unwrap_or("edit").to_string();
        let Some(Member::Multitrack(editor)) = self.member(member) else {
            return Err("the member is not a multitrack".into());
        };
        let leg = |forward: Value, backward: Value| Leg {
            holds_forward: editor.joins_named(&forward),
            holds_backward: editor.joins_named(&backward),
            key: String::new(),
            forward: json!({ "edit": forward }),
            backward,
        };
        if matches!(call["verb"].as_str(), Some("apply" | "record")) {
            self.multitrack_runtime
                .operations
                .check_write(crate::application::operations::Target {
                    member,
                    object: editor.session_binding(),
                    server: self.seats[member as usize].server,
                })
                .map_err(str::to_string)?;
        }
        if call["verb"] == "record" {
            let legs: Vec<Leg> = call["legs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| leg(l["forward"].clone(), l["backward"].clone()))
                .collect();
            let taken = !legs.is_empty() && self.record(member, &Record { label, legs }, false);
            return Ok(json!(taken));
        }
        let shared = editor.shared();
        let request = call.to_string();
        let answer: Value = {
            let mut multitrack = shared.lock().unwrap_or_else(|e| e.into_inner());
            serde_json::from_str(&handle::call_json(&mut multitrack, &request))
                .unwrap_or(Value::Null)
        };
        if let Some(error) = answer["error"].as_str() {
            return Err(error.to_string());
        }
        if !handle::mutates(&request) || answer["applied"] != true {
            return Ok(answer);
        }
        let entry = match (action["record"] != false, answer.get("current")) {
            (true, Some(current)) => Some(Record {
                label,
                legs: vec![leg(call["intent"].clone(), current.clone())],
            }),
            _ => None,
        };
        let taken = entry.is_some_and(|record| self.record(member, &record, false));
        if !taken {
            self.moved();
        }
        self.sound(member, tracks, ids);
        ran.effects.push(effect("resync", json!({})));
        Ok(answer)
    }

    /// **The window's closing effect** (`closeWindow` or `closedWindow`),
    /// carrying the multitrack as the window leaves it: what a script that
    /// held it goes on holding.
    pub(super) fn closing(&self, member: MemberId, kind: &str) -> Value {
        let multitrack = match self.member(member) {
            Some(Member::Multitrack(editor)) => {
                serde_json::to_value(&*editor.multitrack()).unwrap_or(Value::Null)
            }
            _ => Value::Null,
        };
        effect(kind, json!({ "multitrack": multitrack }))
    }

    /// **What a pass of member `member` is read off**: its window's time
    /// range and position cursor, in seconds.
    fn multitrack_view(&self, member: MemberId) -> (Option<(f64, f64)>, Option<f64>) {
        match self.member(member) {
            Some(Member::Multitrack(editor)) => (editor.range(), editor.cursor()),
            _ => (None, None),
        }
    }

    /// Whether the multitrack's transport rolls, as the engine says now.
    fn multitrack_rolling(&self, tracks: &mut Tracks) -> bool {
        let rolling = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.rolling(tracks.playback.transport()));
        tracks.playback.set_rolling(rolling);
        rolling
    }

    /// **A message of the window's**, as the editing context takes it: the
    /// turn recorded, the edit sounded, the transport asked, the window
    /// answered. Answers whether the multitrack changed.
    fn multitrack_event(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        event: &Event,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Value {
        // what a close asks about: whether anything moved since the session
        // was last written
        let unsaved =
            tracks.saved.is_some() && self.multitrack_snapshot(member, tracks) != tracks.saved;
        // and whether it has a file to be written over, or a Save asks where
        let untitled = tracks.session.is_none() && !matches!(tracks.save_to, Some(Some(_)));
        if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
            editor.set_unsaved(unsaved);
            editor.set_untitled(untitled);
        }
        let before = self.version;
        let Some(mut turned) = self.event(member, event) else {
            return json!(false);
        };
        let super::Outcome::Multitrack(outcome) = turned.outcome else {
            return json!(false);
        };
        match outcome.turn {
            Kind::Nothing => return json!(false),
            Kind::Closed => {
                self.close_tracks(member, tracks, ids);
                ran.effects.push(self.closing(member, "closedWindow"));
                return json!(false);
            }
            Kind::Step => {
                if let Some(step) = turned.stepped.take() {
                    for corrected in &step.corrections {
                        if corrected.member == member {
                            ran.effects
                                .push(effect("echo", json!({"answer":corrected.answer})));
                        }
                    }
                    for freed in self.audio_carry_step(step, caller, ids, ran) {
                        caller.delete(&freed.files);
                    }
                }
                let stepped = self.version != before;
                if stepped {
                    self.sound(member, tracks, ids);
                    self.sound_rolls(ids);
                    ran.effects.push(effect("resync", json!({})));
                }
                ran.effects
                    .push(effect("echo", json!({"answer": outcome.answer})));
                return json!(stepped);
            }
            Kind::Stale | Kind::Route => {}
        }
        if outcome.changed {
            self.sound(member, tracks, ids);
        }
        // **The rules every editor plays by** (`clausters_editing::pass`),
        // read off this window's selection and position cursor
        let (range, cursor) = self.multitrack_view(member);
        let view = tracks.view(range, cursor);
        if outcome.follow.is_some() || outcome.locate.is_some() {
            // whether it rolls is the engine's: a pass may have ended on its own
            self.multitrack_rolling(tracks);
        }
        if let Some(place) = outcome.follow {
            tracks.playback.follow(view, place);
        }
        // a cursor placed moves the music where it plays
        if let Some(secs) = outcome.locate {
            let frame = tracks.frame(secs);
            tracks.playback.place(frame, view);
        }
        match outcome.transport {
            Some(TransportVerb::Toggle) => {
                if self.multitrack_rolling(tracks) {
                    tracks.playback.pause();
                } else if tracks.playback.paused() {
                    tracks.playback.resume();
                } else {
                    let looping = tracks.playback.looping();
                    tracks.playback.play(looping, view);
                }
            }
            Some(TransportVerb::PlayStop {
                mark,
                range,
                looping,
            }) => {
                if self.multitrack_rolling(tracks) {
                    let back = tracks.frame(mark);
                    tracks.playback.stop(back);
                } else {
                    let view = tracks.view(range, Some(mark));
                    tracks.playback.play(looping, view);
                }
            }
            Some(TransportVerb::Loop { range, looping }) => {
                let view = tracks.view(range, cursor);
                tracks.playback.set_pass(looping, view);
            }
            Some(TransportVerb::Stop { mark }) => {
                let back = tracks.frame(mark);
                tracks.playback.stop(back);
            }
            Some(TransportVerb::StopAtEnd) => {
                let end = match tracks.playback.end() {
                    clausters_editing::playback::End::Open => {
                        clausters_editing::playback::End::Contents
                    }
                    _ => clausters_editing::playback::End::Open,
                };
                tracks.playback.set_end(end);
            }
            Some(TransportVerb::Cue { secs }) => {
                let frame = tracks.frame(secs);
                tracks.playback.cue(frame);
            }
            None => {}
        }
        if let Some(source) = outcome.open {
            ran.effects.push(effect("open", json!({"source": source})));
        }
        ran.effects
            .push(effect("echo", json!({"answer": outcome.answer})));
        if outcome.changed {
            let version = self.version;
            if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
                let settled = editor.settle(version);
                ran.effects.push(effect("echo", json!({"answer": settled})));
            }
        }
        // **The close form's Save** (or Ctrl+S): the session written where it
        // saves, and a close only once it is -- a save that fails keeps the
        // window, and says why
        if outcome.save || outcome.save_as.is_some() {
            let save = super::multitrack_operations::SaveTo {
                path: outcome.save_as.clone(),
                close: outcome.close,
                save_as: outcome.save_as.is_some(),
            };
            if let Err(why) = self.save_session(member, tracks, save, ran) {
                ran.effects.push(effect("say", json!({"reason":why})));
            }
            return json!(outcome.changed);
        }
        if outcome.close {
            self.close_tracks(member, tracks, ids);
            ran.effects.push(self.closing(member, "closeWindow"));
        }
        json!(outcome.changed)
    }

    /// **What sounds made the multitrack as it now is**: each track's
    /// regions read again, the take of every track whose regions moved
    /// handed to the engine, a track that went closed, each track's width
    /// written into the multitrack (the widest of its regions, which a save
    /// keeps), and the nodes brought in step.
    fn sound(&mut self, member: MemberId, tracks: &mut Tracks, ids: &mut IdSpaces) {
        let Some(Member::Multitrack(editor)) = self.member(member) else {
            return;
        };
        let (shapes, sources) = editor.read_sources();
        let shared = editor.shared();
        let reads = {
            let mut multitrack = shared.lock().unwrap_or_else(|e| e.into_inner());
            let reads = track_reads(&multitrack, tracks.rate, &shapes, &BTreeSet::new());
            for read in &reads {
                if let Some(track) = multitrack.tracks.iter_mut().find(|t| t.id == read.track)
                    && !read.regions.is_empty()
                {
                    track.channels = read.channels;
                }
            }
            reads
        };
        let sources = Arc::new(sources);
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        let mut seen = BTreeSet::new();
        for read in &reads {
            if read.regions.is_empty() {
                continue;
            }
            seen.insert(read.track);
            let same = tracks
                .taken
                .get(&read.track)
                .is_some_and(|(regions, feed)| {
                    *regions == read.regions && feed.buffers.len() == read.channels
                });
            if same {
                continue;
            }
            tracks.revision += 1;
            let take = HeldTake {
                reader: read.track.0,
                revision: tracks.revision,
                list: HeldList::Placed(PlacedList {
                    regions: read.regions.clone().into(),
                    sources: Arc::clone(&sources),
                    frames: read.frames,
                }),
                channels: read.channels as u32,
                rate: tracks.rate,
            };
            // a take the engine cannot read is the engine's to say; the track
            // plays what it played before
            if let Ok(buffers) = engine.take(member, &take, ids) {
                let feed = PlannedFeed {
                    buffers,
                    frames: read.frames as f64,
                };
                tracks
                    .taken
                    .insert(read.track, (read.regions.clone(), feed));
            }
        }
        let gone: Vec<NodeId> = tracks
            .taken
            .keys()
            .filter(|track| !seen.contains(track))
            .copied()
            .collect();
        for track in gone {
            tracks.taken.remove(&track);
            engine.close(member, track.0, ids);
        }
        let feeds: HashMap<NodeId, PlannedFeed> = tracks
            .taken
            .iter()
            .map(|(track, (_, feed))| (*track, feed.clone()))
            .collect();
        let multitrack = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // an id space run out leaves what was made before it
        let _ = tracks
            .playback
            .sync_fed(&multitrack, tracks.rate, &feeds, 1.0, ids);
        // **The boxes of notes**: what they play, on the transport's lane
        if let Some(Member::Multitrack(editor)) = self.member(member) {
            let placed = editor.placed_notes();
            let _ = tracks.playback.notes(&placed, ids);
        }
        // the meters the window reads, once their buses are made
        let meters = tracks
            .playback
            .meters()
            .into_iter()
            .map(|(track, bus, channels)| crate::multitrack::Meter {
                track,
                bus,
                channels,
            })
            .collect();
        if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
            editor.set_meters(meters);
        }
    }

    /// The multitrack's nodes and its tracks' readers, gone.
    pub(super) fn close_tracks(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        ids: &mut IdSpaces,
    ) {
        if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
            editor.invalidate_session_binding();
        }
        let _ = tracks.playback.close(ids);
        tracks.taken.clear();
        if let Some(engine) = self.engine.as_mut() {
            engine.close_member(member, ids);
        }
    }

    /// **What a script reads of a multitrack member**: `transport` (its
    /// transport, whether it rolls) or `output` (each track's meter buses,
    /// and the samples played as silence).
    pub(crate) fn multitrack_query(&mut self, member: MemberId, query: &str) -> Value {
        self.select_member(member);
        let Some(mut tracks) = self.tracks.remove(&member) else {
            return json!({"error": "the multitrack has no session"});
        };
        let answer = match query {
            "transport" => json!({
                "transport": tracks.playback.transport(),
                "rolling": self.multitrack_rolling(&mut tracks),
            }),
            "history" => json!({
                "canUndo": self.can_undo(),
                "canRedo": self.can_redo(),
                "undoLabel": self.undo_label(),
                "redoLabel": self.redo_label(),
            }),
            "output" => {
                let engine = self.engine.as_ref();
                let meters: BTreeMap<String, Vec<f32>> = tracks
                    .playback
                    .meters()
                    .into_iter()
                    .map(|(track, bus, channels)| {
                        let levels = (0..channels)
                            .map(|c| engine.map_or(0.0, |e| e.control(bus as usize + c)))
                            .collect();
                        (track.to_string(), levels)
                    })
                    .collect();
                json!({"tracks": meters, "starved": engine.map_or(0, |e| e.starved())})
            }
            _ => json!({"error": format!("no multitrack query {query:?}")}),
        };
        self.tracks.insert(member, tracks);
        answer
    }

    /// **A multitrack member is gone**: its nodes and its tracks' readers.
    pub fn forget_multitrack(&mut self, member: MemberId, ids: &mut IdSpaces) {
        if let Some(mut tracks) = self.tracks.remove(&member) {
            self.close_tracks(member, &mut tracks, ids);
        }
    }
}
