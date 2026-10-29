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
use super::{Editing, Member, MemberId, Raised, Ran};
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
}

impl Tracks {
    fn new(editing: &mut Editing) -> Option<Tracks> {
        let engine = editing.engine.as_mut()?;
        Some(Tracks {
            rate: engine.rate(),
            playback: MultitrackPlayback::in_medium(engine.medium()),
            taken: HashMap::new(),
            revision: 0,
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
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut tracks = match self.tracks.remove(&member) {
            Some(tracks) => tracks,
            None => match Tracks::new(self) {
                Some(tracks) => tracks,
                None => return refused("runtime", "the multitrack has no engine"),
            },
        };
        let ran = self.multitrack_run(member, &mut tracks, action, ids);
        self.tracks.insert(member, tracks);
        ran
    }

    fn multitrack_run(
        &mut self,
        member: MemberId,
        tracks: &mut Tracks,
        action: &Value,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        let verb = action["verb"].as_str().unwrap_or_default();
        let result = match verb {
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
