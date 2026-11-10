//! **The multitrack's coordinator**: every action of a multitrack member --
//! a gesture, a script's verb, the transport, a close -- carried out in the
//! application that holds its engine, and answered as the window's effects.
//!
//! The audio editor's coordinator ([`super::coordinator`]) runs its actions
//! as runs that stop on what only the caller does, a render above all. A
//! multitrack edit moves where things are and never the samples they read.
//! Session storage and source measurement wait on identified operations;
//! other actions complete in this turn. Playback hands each track's take
//! ([`HeldTake`]) to the engine, into the track's fed buffers. An edit of a track's
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
use super::multitrack_action::{Action, Handle, HandleCall, Kind as ActionKind};
use super::{Caller, Editing, Member, MemberId, Raised, Ran};
use crate::multitrack::editor::{FileChange, Kind, TransportVerb};
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
    /// **A save this turn handed to storage**: the turn's end is that
    /// storage's, when it lands, not the turn's own.
    storing: bool,
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

    pub(super) fn new(editing: &mut Editing) -> Option<Tracks> {
        let engine = editing.engine.as_mut()?;
        Some(Tracks {
            rate: engine.rate(),
            playback: MultitrackPlayback::in_medium(engine.medium()),
            taken: HashMap::new(),
            revision: 0,
            storing: false,
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
        let action = match Action::read(action) {
            Ok(action) => action,
            Err(reason) => return refused("value", reason),
        };
        self.multitrack_act_ready(member, &action, caller, ids)
    }

    fn multitrack_act_ready(
        &mut self,
        member: MemberId,
        action: &Action,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        // with no session, what answers is the window's: its File verbs and
        // the forms they put up
        if matches!(self.member(member), Some(Member::Multitrack(editor))
            if !editor.has_session() && !action.allows_empty()
                && !matches!(&action.kind, ActionKind::Event(event) if editor.holds_form(event)))
        {
            return refused("value", "no file is open");
        }
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
        path: &str,
        caller: &mut dyn Caller,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        let bytes = clausters_core::files::read(path).map_err(|e| format!("{path}: {e}"))?;
        let session: clausters_document::session::Session =
            serde_json::from_slice(&bytes).map_err(|e| format!("{path}: not a session: {e}"))?;
        self.open_session_snapshot(member, session, Some(path.to_string()), None, caller, ran)
    }

    fn open_session_snapshot(
        &mut self,
        member: MemberId,
        session: clausters_document::session::Session,
        path: Option<String>,
        checkpoint: Option<clausters_document::session::Session>,
        caller: &mut dyn Caller,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        use crate::multitrack::sources;
        let destination = path.as_deref().unwrap_or("");
        let folder = std::path::Path::new(destination)
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
        self.change_multitrack_session(
            member,
            crate::multitrack::operations::Command::Open(Box::new(
                crate::multitrack::operations::Opened {
                    session: session.clone(),
                    resolved,
                },
            )),
            ran,
        )?;
        if !measured.is_empty() {
            self.measure_multitrack(member, measured, ran)?;
        }
        if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
            editor.file_mut().session = Some(session);
            editor.file_mut().path = path;
            editor.set_checkpoint(Some(
                checkpoint.unwrap_or_else(|| editor.session_snapshot()),
            ));
        }
        Ok(json!({"unresolved": unresolved}))
    }

    /// **The window's session changed** (`command` a new session, an open
    /// or none at all): one transition through the session's runtime. The
    /// session it let go of is said (`released`), for the host to hand its
    /// holder what the window left it as.
    fn change_multitrack_session(
        &mut self,
        member: MemberId,
        command: crate::multitrack::operations::Command,
        ran: &mut Ran,
    ) -> Result<(), String> {
        let seat = self
            .seats
            .get_mut(member as usize)
            .ok_or("the member is absent")?;
        let Member::Multitrack(editor) = &mut seat.member else {
            return Err("the member is not a multitrack".into());
        };
        let request = crate::multitrack::operations::Request {
            target: crate::application::operations::Target {
                member,
                object: editor.session_binding(),
                server: seat.server,
            },
            command,
        };
        let ticket =
            self.multitrack_runtime
                .prepare(editor.as_ref(), &mut self.services, request)?;
        let finished = self
            .multitrack_runtime
            .operations
            .finish_ready(ticket)
            .expect("a binding transition has no pending I/O");
        let settled = self.multitrack_runtime.settle_turn(
            editor.as_mut(),
            &mut self.services,
            finished,
            &mut self.version,
        );
        if settled.protocol.committed != Some(true) {
            return Err("the session changed before confirmation".into());
        }
        if let Some(released) = settled.outcome.and_then(|outcome| outcome.released) {
            ran.effects.push(effect(
                "released",
                json!({"snapshot": serde_json::to_value(released.snapshot).unwrap_or(Value::Null)}),
            ));
        }
        self.version += 1;
        editor.binding_changed(self.version);
        self.select_member(member);
        Ok(())
    }

    /// **The window's session changed** (File > New, Open, Close file, or
    /// the door's verbs): a new untitled session, the one at a path, or
    /// none, the window drawn again over it and its tracks sounded. A
    /// session another window holds is not opened twice: that window comes
    /// forward and this one stays as it is.
    pub(super) fn change_file(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        change: FileChange,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        use crate::multitrack::operations::Command;
        let answer = match &change {
            FileChange::New => self
                .change_multitrack_session(member, Command::New, ran)
                .map(|()| Value::Null)?,
            FileChange::Close => self
                .change_multitrack_session(member, Command::Unbind, ran)
                .map(|()| Value::Null)?,
            FileChange::Open(path) => {
                let here = match self.member(member) {
                    Some(Member::Multitrack(editor)) => editor.window_id(),
                    _ => None,
                };
                if let Some(window) = self
                    .window_holding_file(path)
                    .filter(|window| Some(*window) != here)
                {
                    ran.effects.push(effect("front", json!({"window": window})));
                    return Err(format!("{path} is open in another window"));
                }
                self.open_session(member, path, caller, ran)?
            }
        };
        self.stop_multitrack(member, tracks, ids);
        if change != FileChange::Close {
            self.sound(member, tracks, ids);
        }
        if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
            if let FileChange::Open(_) = change {
                editor.set_checkpoint(Some(editor.session_snapshot()));
            }
            // a window opened read-only was so over the session it opened:
            // the one in its place saves
            editor.set_saves(true);
        }
        ran.effects.push(effect("publish", json!({})));
        ran.effects.push(effect("resync", json!({})));
        // a gesture made against the session it let go of is refused, and one
        // made after is not
        ran.effects.push(self.told_version());
        Ok(answer)
    }

    /// **The session written** at `path`, or where it was opened from: the
    /// multitrack as it now is, its takes and its joins in the table, every
    /// file inside the session's folder named relative to it. A multitrack
    /// opened over no session asks for a path.
    fn save_session(
        &mut self,
        member: MemberId,
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
            &editor.file().save_to,
            editor.file().path.as_ref(),
        ) {
            (Some(path), _, _) => path.to_string(),
            (None, Some(Some(path)), _) => path.clone(),
            (None, Some(None), _) => {
                return Err("this session is read-only: it was opened with nowhere to save".into());
            }
            (None, None, Some(path)) => path.clone(),
            (None, None, None) => return Err("the session has no file yet: save it as".into()),
        };
        let session = editor.session_snapshot();
        let target = crate::application::operations::Target {
            member,
            object: editor.session_binding(),
            server: self.seats[member as usize].server,
        };
        let saved = crate::multitrack::operations::Saved {
            session,
            path: path.clone(),
            then: save.then,
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

    fn multitrack_run(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        action: &Action,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        if let Some(save_to) = &action.save_to
            && let Some(Member::Multitrack(editor)) = self.member_mut(member)
        {
            editor.file_mut().save_to = Some(save_to.clone());
        }
        let result = match &action.kind {
            ActionKind::NewFile => {
                self.change_file(member, tracks, FileChange::New, caller, ids, &mut ran)
            }
            ActionKind::CloseFile => {
                self.change_file(member, tracks, FileChange::Close, caller, ids, &mut ran)
            }
            ActionKind::Open {
                snapshot: Some(snapshot),
                ..
            } => {
                let opened = self.open_session_snapshot(
                    member,
                    snapshot.session.clone(),
                    snapshot.path.clone(),
                    snapshot.saved.clone(),
                    caller,
                    &mut ran,
                );
                if opened.is_ok() {
                    self.stop_multitrack(member, tracks, ids);
                    self.sound(member, tracks, ids);
                }
                opened
            }
            ActionKind::Open {
                session: Some(path),
                ..
            } => self
                .change_file(
                    member,
                    tracks,
                    FileChange::Open(path.clone()),
                    caller,
                    ids,
                    &mut ran,
                )
                .map(|mut answer| {
                    answer["transport"] = json!(tracks.playback.transport());
                    answer
                }),
            ActionKind::Save { path } => match self.save_session(
                member,
                super::multitrack_operations::SaveTo {
                    path: path.clone(),
                    then: None,
                    save_as: path.is_some(),
                },
                &mut ran,
            ) {
                Ok(answer) => {
                    tracks.storing = true;
                    Ok(answer)
                }
                Err(why) => Err(why),
            },
            ActionKind::Open {
                session: None,
                snapshot: None,
            }
            | ActionKind::Sound => {
                self.sound(member, tracks, ids);
                if matches!(action.kind, ActionKind::Open { .. })
                    && let Some(Member::Multitrack(editor)) = self.member_mut(member)
                {
                    editor.set_checkpoint(Some(editor.session_snapshot()));
                }
                Ok(json!({"transport": tracks.playback.transport()}))
            }
            ActionKind::Event(event) => {
                Ok(self.multitrack_event(member, tracks, event, caller, ids, &mut ran))
            }
            ActionKind::Verb { call } => {
                let version = self.version;
                let answer = match self.member_mut(member) {
                    Some(Member::Multitrack(editor)) => call.execute(editor, version),
                    _ => return refused("value", "the member is not a multitrack"),
                };
                self.sound(member, tracks, ids);
                // what the window shows changed with it: drawn again
                if call.resyncs() {
                    ran.effects.push(effect("resync", json!({})));
                }
                Ok(answer)
            }
            // **A script's play**: over its range, or -- with none -- a
            // paused pass goes on, and otherwise one starts at the position
            // cursor, as the space bar's does
            ActionKind::Play { range, looping } => {
                let range = range.map(|[from, to]| (from, to));
                if range.is_none() && tracks.playback.paused() {
                    tracks.playback.resume();
                } else {
                    let (_, cursor) = self.multitrack_view(member);
                    let view = tracks.view(range, cursor);
                    tracks.playback.play(*looping, view);
                }
                Ok(Value::Null)
            }
            ActionKind::Pause => {
                tracks.playback.pause();
                Ok(Value::Null)
            }
            // a script's stop goes back to `mark`, which is the position
            // cursor from then on: a play with no range starts there
            ActionKind::Stop { mark } => {
                let mark = *mark;
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
            ActionKind::Locate { at } => {
                let at = *at;
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
            ActionKind::Playing => Ok(json!(self.multitrack_rolling(tracks))),
            ActionKind::Handle(handle) => {
                self.multitrack_handle(member, tracks, handle, ids, &mut ran)
            }
            ActionKind::Undo | ActionKind::Redo => {
                let direction = if matches!(action.kind, ActionKind::Undo) {
                    clausters_document::history::Direction::Undo
                } else {
                    clausters_document::history::Direction::Redo
                };
                let before = self.version;
                let stepped = self.walk_history(direction);
                for corrected in &stepped.corrections {
                    ran.effects
                        .push(effect("echo", json!({"answer": corrected.answer})));
                }
                for freed in self.carry_step(stepped, caller, ids, &mut ran) {
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
            ActionKind::Close | ActionKind::Closed => {
                self.close_tracks(member, tracks, ids);
                ran.effects.push(self.closing(
                    member,
                    if matches!(action.kind, ActionKind::Closed) {
                        "closedWindow"
                    } else {
                        "closeWindow"
                    },
                ));
                Ok(json!(false))
            }
        };
        // **where the session it opened saves**, when the opener said: the
        // session the window held before was another's
        if result.is_ok()
            && matches!(action.kind, ActionKind::Open { .. } | ActionKind::NewFile)
            && let Some(save_to) = &action.save_to
            && let Some(Member::Multitrack(editor)) = self.member_mut(member)
        {
            editor.file_mut().save_to = Some(save_to.clone());
            editor.set_saves(save_to.is_some());
        }
        if std::mem::take(&mut tracks.storing) {
            // a save ends when its storage lands
            return ran;
        }
        ran.end = Some(result.map_err(|message: String| Raised {
            kind: "value".into(),
            message,
        }));
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
        action: &Handle,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        use crate::turn::{Leg, Record};
        use clausters_document::multitrack::{
            edit::{MultitrackIntent, payload},
            handle,
        };

        let call = &action.call;
        let label = action.label.clone();
        let Some(Member::Multitrack(editor)) = self.member(member) else {
            return Err("the member is not a multitrack".into());
        };
        let leg = |forward: &MultitrackIntent, backward: &MultitrackIntent| Leg {
            holds_forward: editor.joins_in_intent(forward),
            holds_backward: editor.joins_in_intent(backward),
            key: String::new(),
            forward: json!({ "edit": payload(forward).0 }),
            backward: payload(backward).0,
        };
        if matches!(
            call,
            HandleCall::Record(_) | HandleCall::Request(handle::Request::Apply { .. })
        ) {
            self.multitrack_runtime
                .operations
                .check_write(crate::application::operations::Target {
                    member,
                    object: editor.session_binding(),
                    server: self.seats[member as usize].server,
                })
                .map_err(str::to_string)?;
        }
        if let HandleCall::Record(recorded) = call {
            let legs: Vec<Leg> = recorded
                .iter()
                .map(|recorded| leg(&recorded.forward, &recorded.backward))
                .collect();
            let taken = !legs.is_empty() && self.record(member, &Record { label, legs }, false);
            ran.effects.push(self.told_version());
            return Ok(json!(taken));
        }
        let HandleCall::Request(request) = call else {
            unreachable!()
        };
        let shared = editor.shared();
        let (answer, entry) = {
            let mut multitrack = shared.lock().unwrap_or_else(|e| e.into_inner());
            let handle::Request::Apply { intent, inverse } = request else {
                let answer = request.execute(&mut multitrack);
                return if let Some(error) = answer["error"].as_str() {
                    Err(error.to_string())
                } else {
                    Ok(answer)
                };
            };
            let applied = handle::apply(&mut multitrack, intent, *inverse);
            let entry = applied
                .current
                .as_ref()
                .filter(|_| action.record)
                .map(|current| Record {
                    label,
                    legs: vec![leg(intent, current)],
                });
            let answer = serde_json::to_value(&applied).unwrap_or(Value::Null);
            if !applied.applied {
                return Ok(answer);
            }
            (answer, entry)
        };
        let taken = entry.is_some_and(|record| self.record(member, &record, false));
        if !taken {
            self.moved();
        }
        self.sound(member, tracks, ids);
        ran.effects.push(effect("resync", json!({})));
        ran.effects.push(self.told_version());
        Ok(answer)
    }

    /// **The window told the version it draws now**, after what moved it came
    /// by no gesture of its own: a script's edit, or a change of session. What
    /// it draws is already that, so the next gesture is made against it and is
    /// not refused as overtaken.
    pub(super) fn told_version(&self) -> Value {
        let told = clausters_editing::conversation::Answer::Ack {
            seq: 0,
            doc_version: self.version,
            reason: None,
        };
        effect("echo", json!({"answer": told}))
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
        let snapshot = match self.member(member) {
            Some(Member::Multitrack(editor)) => editor.closing_snapshot(),
            _ => Value::Null,
        };
        effect(
            kind,
            json!({ "multitrack": multitrack, "snapshot": snapshot }),
        )
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
        let version = self.version;
        if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
            let file = editor.file();
            let unsaved = editor.unsaved_at(version);
            let untitled = file.path.is_none() && !matches!(file.save_to, Some(Some(_)));
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
                    // **every window the step moved is corrected**, as an
                    // undo asked by a client corrects them: a box's roll
                    // walks the multitrack's history
                    for corrected in &step.corrections {
                        ran.effects
                            .push(effect("echo", json!({"answer":corrected.answer})));
                    }
                    for freed in self.carry_step(step, caller, ids, ran) {
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
        if let Some((track, port, value)) = &outcome.port_hold {
            tracks.playback.hold_port(*track, port, *value);
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
            use crate::closing::Leaving;
            let then = match (&outcome.file, outcome.close) {
                (_, true) => Some(Leaving::Window),
                (Some(change), false) => Some(Leaving::File(change.clone())),
                (None, false) => None,
            };
            let save = super::multitrack_operations::SaveTo {
                path: outcome.save_as.clone(),
                then,
                save_as: outcome.save_as.is_some(),
            };
            match self.save_session(member, save, ran) {
                Ok(_) => tracks.storing = true,
                Err(why) => ran.effects.push(effect("say", json!({"reason":why}))),
            }
            return json!(outcome.changed);
        }
        if outcome.close {
            self.close_tracks(member, tracks, ids);
            ran.effects.push(self.closing(member, "closeWindow"));
        }
        // **File > New, Open, Close file**, with nothing to lose or the
        // close form answered Don't save
        if let Some(change) = outcome.file
            && let Err(why) = self.change_file(member, tracks, change, caller, ids, ran)
        {
            ran.effects.push(effect("say", json!({"reason": why})));
        }
        json!(outcome.changed)
    }

    /// **What sounds made the multitrack as it now is**: each track's
    /// regions read again, the take of every track whose regions moved
    /// handed to the engine, a track that went closed, each track's width
    /// written into the multitrack (the widest of its regions, which a save
    /// keeps), and the nodes brought in step.
    fn sound(&mut self, member: MemberId, tracks: &mut Tracks, ids: &mut IdSpaces) {
        if matches!(self.member(member), Some(Member::Multitrack(editor)) if !editor.observes_session())
        {
            return;
        }

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
        self.stop_multitrack(member, tracks, ids);
    }

    fn stop_multitrack(&mut self, member: MemberId, tracks: &mut Tracks, ids: &mut IdSpaces) {
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
        let version = self.version;
        let Some(Member::Multitrack(editor)) = self.member(member) else {
            return json!({"error":"the member is not a multitrack"});
        };
        if query == "file" {
            return if !editor.has_session() {
                json!({"empty":true})
            } else {
                json!({"empty":false, "object":editor.session_binding(), "path":editor.file().path.as_ref(),
                    "dirty":editor.unsaved_at(version)})
            };
        }
        if query == "session" && editor.has_session() {
            return serde_json::to_value(editor.file_snapshot()).unwrap_or(Value::Null);
        }
        if !editor.has_session() {
            return if query == "history" {
                json!({"canUndo":false,"canRedo":false,"undoLabel":null,"redoLabel":null})
            } else {
                json!({"error":"no file is open"})
            };
        }
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
        // in no window now, it holds no file a later open would find, and
        // nothing walks its session again: a window that opens it next is a
        // new member, over what this one left
        if let Some(Member::Multitrack(editor)) = self.member_mut(member) {
            editor.set_window(None);
            let id = editor.session_binding();
            self.services
                .histories
                .close_order(crate::application::histories::Object { member, id });
        }
    }
}
