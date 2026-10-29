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
//! `open` (a box of notes asked for its roll, `source`) and `save` (the
//! close form's Save, for the holder to write the session).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use clausters_core::ids::IdSpaces;
use clausters_document::NodeId;
use clausters_document::multitrack::nodes::PlannedFeed;
use clausters_document::multitrack::read::{PlacedRegion, track_reads};
use clausters_editing::apply::Medium;
use clausters_editing::playback::MultitrackPlayback;
use serde_json::{Value, json};

use super::held::{HeldList, HeldTake, PlacedList};
use super::{Caller, Editing, Member, MemberId, Raised, Ran, Render};
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
    session: Option<(clausters_document::session::Session, String)>,
    /// The peaks being measured, by take: where each is written.
    measuring: Vec<(clausters_document::SourceId, String)>,
}

impl Tracks {
    fn new(editing: &mut Editing) -> Option<Tracks> {
        let engine = editing.engine.as_mut()?;
        Some(Tracks {
            rate: engine.rate(),
            playback: MultitrackPlayback::in_medium(engine.medium()),
            taken: HashMap::new(),
            revision: 0,
            session: None,
            measuring: Vec::new(),
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
    /// **Starts `action` on multitrack member `member`** and carries it to
    /// its end: a multitrack has no render to wait on. `ids` are the
    /// engine's, which its playback is allocated from.
    pub(crate) fn multitrack_act(
        &mut self,
        member: MemberId,
        action: &Value,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
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

    /// **The peaks a session's takes were measured into have landed**:
    /// each take names its peaks from now on, and the window draws them.
    pub(crate) fn multitrack_landed(
        &mut self,
        member: MemberId,
        result: Result<(), String>,
    ) -> Ran {
        let measured = self
            .tracks
            .get_mut(&member)
            .map(|tracks| std::mem::take(&mut tracks.measuring))
            .unwrap_or_default();
        if result.is_ok()
            && let Some(Member::Multitrack(editor)) = self.member_mut(member)
        {
            for (source, peaks) in measured {
                if let Some(mut file) = editor.take(source).cloned() {
                    file.peaks = Some(peaks.as_str().into());
                    editor.set_take(source, file);
                }
            }
        }
        Ran {
            effects: vec![effect("resync", json!({}))],
            ..Ran::default()
        }
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
        let resolved = sources::resolve(&session, &folder, &mut |file| caller.header(file));
        if !resolved.measure.is_empty() {
            let _ =
                clausters_core::files::create_dir_all(sources::located(&folder, sources::PEAKS));
        }
        let Some(Member::Multitrack(editor)) = self.member_mut(member) else {
            return Err("the member is not a multitrack".into());
        };
        editor.set_multitrack(session.multitrack.clone());
        editor.set_takes(resolved.takes);
        editor.set_segments(resolved.segments);
        let unresolved: Vec<Value> = resolved
            .unresolved
            .iter()
            .map(|(id, why)| json!({"source": id.0, "reason": why}))
            .collect();
        if !resolved.measure.is_empty() {
            tracks.measuring = resolved
                .measure
                .iter()
                .map(|(id, peaks, _)| (*id, peaks.clone()))
                .collect();
            ran.render = Some(Render {
                run: 1,
                steps: resolved
                    .measure
                    .into_iter()
                    .map(|(_, _, step)| step)
                    .collect(),
            });
        }
        tracks.session = Some((session, path.to_string()));
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
        path: Option<&str>,
    ) -> Result<Value, String> {
        use clausters_document::session::{Location, Session, Source};
        use clausters_document::{Document, Lifetime};
        let Some(Member::Multitrack(editor)) = self.member(member) else {
            return Err("the member is not a multitrack".into());
        };
        let path = match (path, tracks.session.as_ref()) {
            (Some(path), _) => path.to_string(),
            (None, Some((_, path))) => path.clone(),
            (None, None) => return Err("the session has no file yet: save it as".into()),
        };
        let mut session = tracks
            .session
            .as_ref()
            .map(|(session, _)| session.clone())
            .unwrap_or_else(|| Session::new(Document::empty()));
        session.multitrack = editor.multitrack().clone();
        let (_, placed) = editor.read_sources();
        for (id, source) in placed {
            if session.sources.contains_key(&id) {
                continue;
            }
            let entry = match source {
                super::held::PlacedSource::File(file) => Source::file(
                    file.path.to_string(),
                    Lifetime::Session,
                )
                .shaped(file.channels, file.frames, file.rate),
                super::held::PlacedSource::Parts(_) => continue,
            };
            session.sources.insert(id, entry);
        }
        for source in editor.joins_read() {
            if let (false, Some(parts)) = (
                session.sources.contains_key(&source),
                editor.segments_of(source),
            ) {
                let mut entry = Source::file("", Lifetime::Session);
                entry.location = Location::Segments { parts };
                session.sources.insert(source, entry);
            }
        }
        let folder = std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let written = crate::multitrack::sources::written(session.clone(), &folder);
        let text = serde_json::to_string_pretty(&written).map_err(|e| e.to_string())?;
        clausters_core::files::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
        tracks.session = Some((session, path.clone()));
        Ok(json!({"destination": path}))
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
        let result = match verb {
            "open" if action["session"].is_string() => {
                let path = action["session"].as_str().unwrap_or_default().to_string();
                let opened = self.open_session(member, tracks, &path, caller, &mut ran);
                if opened.is_ok() {
                    self.sound(member, tracks, ids);
                }
                opened.map(|mut answer| {
                    answer["transport"] = json!(tracks.playback.transport());
                    answer
                })
            }
            "save" => self.save_session(member, tracks, action["path"].as_str()),
            "open" | "sound" => {
                self.sound(member, tracks, ids);
                Ok(json!({"transport": tracks.playback.transport()}))
            }
            "event" => {
                let event = Event {
                    addr: action["addr"].as_str().unwrap_or_default().to_string(),
                    args: action["args"].as_array().cloned().unwrap_or_default(),
                };
                Ok(self.multitrack_event(member, tracks, &event, ids, &mut ran))
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
                Ok(serde_json::from_str(&answer).unwrap_or(Value::Null))
            }
            "play" => {
                let range = action["range"]
                    .as_array()
                    .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
                tracks.playback.play_pass(range, action["looping"] == true);
                Ok(Value::Null)
            }
            "pause" => {
                tracks.playback.pause();
                Ok(Value::Null)
            }
            "stop" => {
                let mark = action["mark"].as_f64().unwrap_or(0.0);
                tracks.playback.stop(mark);
                Ok(Value::Null)
            }
            "locate" => {
                let at = action["at"].as_f64().unwrap_or(0.0);
                if tracks.playback.rolling() {
                    tracks.playback.locate(at);
                } else {
                    tracks.playback.cue(at);
                }
                Ok(Value::Null)
            }
            "playing" => Ok(json!(self.multitrack_rolling(tracks))),
            "close" | "closed" => {
                self.close_tracks(member, tracks, ids);
                ran.effects.push(effect(
                    if verb == "closed" {
                        "closedWindow"
                    } else {
                        "closeWindow"
                    },
                    json!({}),
                ));
                Ok(json!(false))
            }
            other => {
                return refused("value", format!("no multitrack verb {other:?}"));
            }
        };
        ran.end = Some(result.map_err(|message: String| Raised {
            kind: "value".into(),
            message,
        }));
        ran
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
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Value {
        let Some(turned) = self.event(member, event) else {
            return json!(false);
        };
        let super::Outcome::Multitrack(outcome) = turned.outcome else {
            return json!(false);
        };
        match outcome.turn {
            Kind::Nothing => return json!(false),
            Kind::Closed => {
                self.close_tracks(member, tracks, ids);
                ran.effects.push(effect("closedWindow", json!({})));
                return json!(false);
            }
            Kind::Step => {
                let stepped = turned.stepped.as_ref().is_some_and(|s| s.stepped);
                for corrected in turned.stepped.iter().flat_map(|s| &s.corrections) {
                    if corrected.member == member {
                        ran.effects
                            .push(effect("echo", json!({"answer": corrected.answer})));
                    }
                }
                if stepped {
                    self.sound(member, tracks, ids);
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
        if let Some(secs) = outcome.locate {
            tracks.playback.cue(secs);
        }
        match outcome.transport {
            Some(TransportVerb::Toggle) => {
                if self.multitrack_rolling(tracks) {
                    tracks.playback.pause();
                } else {
                    tracks.playback.play_pass(None, false);
                }
            }
            Some(TransportVerb::PlayStop {
                mark,
                range,
                looping,
            }) => {
                if self.multitrack_rolling(tracks) {
                    tracks.playback.stop(mark);
                } else {
                    tracks.playback.play_pass(range, looping);
                }
            }
            Some(TransportVerb::Loop { range, looping }) => {
                tracks.playback.set_loop(range, looping);
            }
            Some(TransportVerb::Stop { mark }) => {
                tracks.playback.stop(mark);
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
                tracks.playback.cue(secs);
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
        if outcome.save {
            ran.effects.push(effect("save", json!({})));
        }
        if outcome.close {
            self.close_tracks(member, tracks, ids);
            ran.effects.push(effect("closeWindow", json!({})));
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
    }

    /// The multitrack's nodes and its tracks' readers, gone.
    fn close_tracks(&mut self, member: MemberId, tracks: &mut Tracks, ids: &mut IdSpaces) {
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
        let Some(mut tracks) = self.tracks.remove(&member) else {
            return json!({"error": "the multitrack has no session"});
        };
        let answer = match query {
            "transport" => json!({
                "transport": tracks.playback.transport(),
                "rolling": self.multitrack_rolling(&mut tracks),
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
