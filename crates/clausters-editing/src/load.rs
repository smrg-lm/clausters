//! **A session's source table, loaded into a server.**
//!
//! A document says what plays when and deliberately never says where its
//! samples are; a session's `sources` table is that other half, and loading it
//! is how a saved multitrack is opened by anything that means to sound it or draw
//! it. The answer is a server buffer per source: `/buffer_allocRead` for a file,
//! `/buffer_stitch` for a join, each followed by the `/done` of that very buffer
//! so that a join is stitched only after the reads it is made of.
//!
//! It was the GUI host's alone, which left a session one endpoint could open and
//! sound and neither client could: a script reopening a multitrack loaded its takes
//! by hand, and a join could not be loaded at all without reading its recipe a
//! second time. Every endpoint now plans the load here and walks the steps
//! through its [`Runner`](crate::run::Runner).
//!
//! # What it does not do
//!
//! It owns no buffer numbers and sends nothing: the numbers are the caller's
//! (the host's one buffer space, a client's allocator), and the steps go out
//! through whatever the caller talks to a server with. It writes nothing back
//! either — the file is the user's, and a session must never rewrite it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use clausters_core::osc::{OscMessage, OscType};
use clausters_document::session::{Location, Session, Source};
use clausters_document::{Body, SourceId};
use serde_json::{Value, json};

use crate::apply::Step;
use crate::sources::{self as projection, Held, Stitch};

/// One source, as it is held once it has been given to the server.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Take {
    /// The server buffer its samples were read into.
    pub bufnum: i32,
    /// Channels, when the table said. The file decides in the end — this is
    /// what the session claimed, and it is only used to size a picture before
    /// the buffer answers for itself.
    pub channels: Option<u32>,
    /// Frames per channel, when the table said.
    pub frames: Option<u64>,
}

/// Which buffer each source was read into.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Takes {
    map: BTreeMap<SourceId, Take>,
}

impl Takes {
    /// Records what a source resolved to — what [`plan`] fills, and what a test
    /// over anything downstream of it needs to be able to state.
    pub fn insert(&mut self, source: SourceId, take: Take) {
        self.map.insert(source, take);
    }

    /// **The source a buffer number came from** — the lookup read the other
    /// way, which is what a box built by a hand needs: a picture names a server
    /// buffer and the document names a source, and this table is the only thing
    /// that knows they are the same samples.
    pub fn source_of(&self, bufnum: i32) -> Option<SourceId> {
        self.map
            .iter()
            .find_map(|(id, take)| (take.bufnum == bufnum).then_some(*id))
    }

    /// The take a source became, if it became one.
    pub fn get(&self, source: SourceId) -> Option<Take> {
        self.map.get(&source).copied()
    }

    /// Every buffer number the sources resolved to, in source order.
    pub fn bufnums(&self) -> Vec<i32> {
        self.map.values().map(|take| take.bufnum).collect()
    }

    /// **Every source and what it resolved to**, for a caller that needs the
    /// table rather than one entry.
    pub fn iter(&self) -> impl Iterator<Item = (&SourceId, &Take)> {
        self.map.iter()
    }

    /// How many sources were resolved.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// What loading a session's samples comes to: the buffers it will occupy, and
/// the commands that fill them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Load {
    /// The map a drawing and a plan consult.
    pub takes: Takes,
    /// The `/buffer_allocRead` and `/buffer_stitch` messages, reads first and
    /// each join after the sources it is over. The caller sends them, through
    /// [`Load::steps`].
    pub messages: Vec<OscMessage>,
    /// Sources the multitrack names that could not be resolved, with why. Reported
    /// rather than swallowed: a box with no samples is going to draw as an
    /// empty rectangle, and the reader deserves to know it is missing rather
    /// than empty.
    pub unresolved: Vec<(SourceId, String)>,
}

impl Load {
    /// **The load as steps**: each read and each join followed by the `/done`
    /// of that very buffer, in the order they were planned -- so a join is
    /// stitched after the reads it is made of, whoever walks them
    /// ([`crate::run::Runner`]).
    ///
    /// Sent as a batch, the stitch reached the server while its reads were
    /// still running: `part 0: buffer 1 is not allocated`, and a join that drew
    /// empty.
    pub fn steps(&self) -> Vec<Step> {
        self.messages
            .iter()
            .flat_map(|message| {
                let index = match message.args.first() {
                    Some(OscType::Int(bufnum)) => Some(*bufnum),
                    _ => None,
                };
                [
                    Step::Send(message.clone()),
                    Step::AwaitDone {
                        command: message.addr.clone(),
                        index,
                    },
                ]
            })
            .collect()
    }
}

/// Plans the load of every source the session actually names.
///
/// `beside` is the session file's own folder, which is what a relative path is
/// resolved against — the rule that makes a session directory movable, and the
/// format's own words rather than any endpoint's convention.
///
/// `present` says whether a file is there. The one that can answer it is
/// whoever shares the server's filesystem — the GUI host, whose server is in
/// its own process — and a caller that cannot answers `true` and lets the
/// server's refusal of the read say it instead.
///
/// `next_buffer` hands out a buffer number, or says why there is none. Numbers
/// are taken in source-id order, which makes the same session load the same way
/// twice.
pub fn plan(
    session: &Session,
    beside: &Path,
    present: &dyn Fn(&Path) -> bool,
    next_buffer: &mut dyn FnMut() -> Result<i32, String>,
) -> Load {
    let mut load = Load::default();
    for id in referenced(session) {
        let Some(source) = session.source(id) else {
            // The session's own `dangling` says this too; saying it here keeps
            // the caller from having to ask twice for one report.
            load.unresolved
                .push((id, "the source table does not hold it".into()));
            continue;
        };
        match locate(source, beside, present) {
            Ok(path) => {
                let bufnum = match next_buffer() {
                    Ok(bufnum) => bufnum,
                    Err(why) => {
                        load.unresolved.push((id, why));
                        continue;
                    }
                };
                load.takes.insert(
                    id,
                    Take {
                        bufnum,
                        channels: source.channels,
                        frames: source.frames,
                    },
                );
                load.messages.push(OscMessage {
                    addr: "/buffer_allocRead".into(),
                    args: vec![
                        OscType::Int(bufnum),
                        OscType::String(path.to_string_lossy().into_owned()),
                    ],
                });
            }
            Err(why) => {
                // A join is not missing, it is not installed yet: it waits for
                // the pass that has its parts.
                if !matches!(source.location, Location::Segments { .. }) {
                    load.unresolved.push((id, why));
                }
            }
        }
    }
    stitch(session, &mut load, next_buffer);
    load
}

/// Where a source's file is, absolute, or why it is nowhere.
fn locate(
    source: &Source,
    beside: &Path,
    present: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, String> {
    match &source.location {
        Location::File { path } if path.is_empty() => Err("the source names no file".to_string()),
        Location::File { path } => {
            let p = Path::new(path);
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                beside.join(p)
            };
            if present(&full) {
                Ok(full)
            } else {
                Err(format!("{} is not there", full.display()))
            }
        }
        // Samples that only ever existed in a running system. The session was
        // allowed to save without it -- that is the format's decision, so that
        // a save is never blocked -- and opening one is where the cost is paid.
        Location::Volatile => Err("the samples were never written down (volatile)".to_string()),
        // A join has no file to find: its samples are spans of the sources
        // beside it, so it is installed once those are there. `stitch` is that
        // pass, and this one leaves it alone.
        Location::Segments { .. } => Err("a join is installed from its parts".to_string()),
    }
}

/// **Installs every join, once the sources it is over are there.**
///
/// A [`Location::Segments`] source owns no samples — it is spans of other
/// sources — so it cannot be read from a path and cannot be planned in the same
/// pass as the files. This is the second pass, and it repeats: a join's part
/// may name a source that is itself a join (the server allows four levels), so
/// a round that installs something makes the next round able to install more,
/// and a round that installs nothing is the end of what is reachable.
///
/// What is left over is **named rather than dropped**: a join over a take that
/// did not load is a box that will draw empty, and the reader deserves the
/// reason.
fn stitch(
    session: &Session,
    load: &mut Load,
    next_buffer: &mut dyn FnMut() -> Result<i32, String>,
) {
    let mut waiting: Vec<SourceId> = referenced(session)
        .into_iter()
        .filter(|id| {
            session
                .source(*id)
                .is_some_and(|s| matches!(s.location, Location::Segments { .. }))
        })
        .collect();
    loop {
        let mut installed = false;
        let mut left = Vec::new();
        for id in waiting {
            let Some(source) = session.source(id) else {
                continue;
            };
            // **What a join is** is the projection's
            // ([`projection::stitch`]) -- the widths, the spans, the channel
            // map a narrow part fills the join with. What is left here is the
            // buffer number, which is the caller's, and the wire.
            let Some(made) = projection::stitch(source, &held_takes(&load.takes)) else {
                // A part that has not landed yet is what the next round is for.
                left.push(id);
                continue;
            };
            let bufnum = match next_buffer() {
                Ok(bufnum) => bufnum,
                Err(why) => {
                    load.unresolved.push((id, why));
                    continue;
                }
            };
            load.takes.insert(
                id,
                Take {
                    bufnum,
                    channels: Some(made.channels as u32),
                    frames: Some(made.frames),
                },
            );
            load.messages.push(stitch_message(bufnum, &made));
            installed = true;
        }
        waiting = left;
        if !installed || waiting.is_empty() {
            break;
        }
    }
    for id in waiting {
        load.unresolved
            .push((id, "a join over samples that did not load".into()));
    }
}

/// The table as the join projection asks for it: what each source resolved to.
pub fn held_takes(takes: &Takes) -> HashMap<SourceId, Held> {
    takes
        .iter()
        .map(|(id, take)| {
            (
                *id,
                Held {
                    buffer: take.bufnum,
                    channels: take.channels.unwrap_or(1).max(1) as usize,
                    frames: take.frames.unwrap_or(0),
                },
            )
        })
        .collect()
}

/// **A join, as `/buffer_stitch` takes it**: the buffer to make, then one
/// fixed-width group per part.
///
/// The one place a resolved join becomes the wire, so a load and an edit that
/// mints one send the same message.
pub fn stitch_message(bufnum: i32, made: &Stitch) -> OscMessage {
    let mut args = vec![
        OscType::Int(bufnum),
        OscType::Int(made.channels as i32),
        OscType::Float(made.rate as f32),
    ];
    for part in &made.parts {
        args.push(OscType::Int(part.buffer));
        args.push(OscType::Int(part.start as i32));
        args.push(OscType::Int(part.frames as i32));
        args.push(OscType::Int(part.fade_in as i32));
        args.push(OscType::Int(part.fade_out as i32));
        // **Every part spells its whole map**, which is what makes the group
        // fixed width.
        for picked in &part.channels {
            args.push(OscType::Int(*picked));
        }
    }
    OscMessage {
        addr: "/buffer_stitch".into(),
        args,
    }
}

/// Every source the session actually names, in a stable order, and the sources
/// a join names beside them.
///
/// The table may hold more than the multitrack uses (a source of a deleted box still
/// has its row until something prunes it), and loading those would read files
/// nothing draws. What a join is made of is named by the join, so a take only a
/// join reads is loaded too.
fn referenced(session: &Session) -> Vec<SourceId> {
    let mut found: Vec<SourceId> = Vec::new();
    let name = |source: SourceId, found: &mut Vec<SourceId>| {
        if !found.contains(&source) {
            found.push(source);
        }
    };
    // **The multitrack names sources too**, and a session written today names them
    // *only* there: a region is a window onto a source, so a reader that walked
    // the general tree alone read nothing in and drew every box empty.
    for track in &session.multitrack.tracks {
        for lane in &track.lanes {
            for region in &lane.regions {
                let clausters_document::multitrack::Content::Window { window, .. } =
                    &region.content
                else {
                    continue;
                };
                if let Some(source) = window.source.samples() {
                    name(source.source, &mut found);
                }
            }
        }
    }
    session.document.walk(&mut |node| {
        // Assembled samples names one source per window; a reader that took
        // only the first would open a joined clip with the rest of it silent.
        match &node.body {
            Body::Vector { source, .. } => name(source.source, &mut found),
            Body::Segments { segments, .. } => {
                for source in segments.iter().filter_map(|s| s.source.samples()) {
                    name(source.source, &mut found);
                }
            }
            _ => {}
        }
    });
    // The parts of every join named so far, and of joins those name.
    let mut at = 0;
    while at < found.len() {
        if let Some(Location::Segments { parts }) =
            session.source(found[at]).map(|source| &source.location)
        {
            for part in parts {
                name(part.source.source, &mut found);
            }
        }
        at += 1;
    }
    found.sort_by_key(|id| id.0);
    found
}

/// [`plan`] as the JSON both client doors carry.
///
/// The request is `{"session", "beside", "buffers"}`: the session as the crate
/// writes it, the folder its relative paths are read against, and the buffer
/// numbers the caller has set aside — one per source in the table is always
/// enough, and the ones the load did not take come back as `unused` for the
/// caller to give back.
///
/// The answer is `{"takes", "steps", "unresolved", "unused"}`: source id to
/// `{"buffer", "channels", "frames"}` (a width or a length the table does not
/// state is 0), the steps to walk through a runner, `[source, why]` for every
/// source that will not load, and the numbers left over. A request that is not
/// a session answers `{"error"}`.
///
/// A client cannot see the server's filesystem — a page never can, and a
/// script's server need not be on its machine — so every file is taken to be
/// there, and one that is not is the server's refusal of its read.
pub fn plan_json(request: &str) -> String {
    let request: Value = serde_json::from_str(request).unwrap_or(Value::Null);
    let session = match request.get("session").cloned().map(Session::read) {
        Some(Ok(session)) => session,
        Some(Err(e)) => return json!({ "error": format!("not a session: {e}") }).to_string(),
        None => return json!({ "error": "the request names no session" }).to_string(),
    };
    let beside = request.get("beside").and_then(Value::as_str).unwrap_or(".");
    let mut buffers = request
        .get("buffers")
        .and_then(Value::as_array)
        .map(|numbers| {
            numbers
                .iter()
                .filter_map(Value::as_i64)
                .map(|n| n as i32)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter();
    let load = plan(&session, Path::new(beside), &|_| true, &mut || {
        buffers
            .next()
            .ok_or_else(|| "no buffer number was set aside for it".to_string())
    });
    // The file each read names, so a buffer that came from one knows it.
    let paths: HashMap<i32, &str> = load
        .messages
        .iter()
        .filter(|message| message.addr == "/buffer_allocRead")
        .filter_map(|message| match message.args.as_slice() {
            [OscType::Int(bufnum), OscType::String(path), ..] => Some((*bufnum, path.as_str())),
            _ => None,
        })
        .collect();
    let takes: serde_json::Map<String, Value> = load
        .takes
        .iter()
        .map(|(id, take)| {
            let mut entry = json!({
                "buffer": take.bufnum,
                "channels": take.channels.unwrap_or(0),
                "frames": take.frames.unwrap_or(0),
            });
            if let Some(path) = paths.get(&take.bufnum) {
                entry["path"] = json!(path);
            }
            (id.0.to_string(), entry)
        })
        .collect();
    json!({
        "takes": takes,
        "steps": crate::apply::steps_json(&load.steps()),
        "unresolved": load
            .unresolved
            .iter()
            .map(|(id, why)| json!([id.0, why]))
            .collect::<Vec<_>>(),
        "unused": buffers.collect::<Vec<_>>(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(dir: &Path, name: &str) -> String {
        std::fs::write(dir.join(name), b"not really a wav, but it is there").expect("write");
        name.to_string()
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "clausters_editing_load_{tag}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// A multitrack of one track whose boxes window `sources`, in order.
    fn multitrack(sources: &[u64]) -> Value {
        let regions: Vec<Value> = sources
            .iter()
            .enumerate()
            .map(|(i, source)| {
                json!({
                    "id": 20 + i, "position": i as f64, "length": 1.0,
                    "content": {"fill": "window", "window": {
                        "source": {"source": source, "lifetime": "session", "generation": 0},
                        "start": 0.0, "duration": 1.0}}
                })
            })
            .collect();
        json!({"tracks": [{"id": 10, "name": "t", "lanes": [{"id": 11, "regions": regions}]}]})
    }

    fn session(sources: &[u64], table: Value) -> Session {
        serde_json::from_value(json!({
            "format": 3, "multitrack": multitrack(sources), "sources": table,
        }))
        .expect("a session")
    }

    fn counting(from: i32) -> impl FnMut() -> Result<i32, String> {
        let mut next = from;
        move || {
            next += 1;
            Ok(next - 1)
        }
    }

    fn file(name: &str) -> Value {
        json!({"location": {"at": "file", "path": name}, "lifetime": "session",
               "channels": 1, "frames": 4, "sample_rate": 48000.0})
    }

    /// Two halves of two takes, the second take's first.
    fn swapped() -> Value {
        json!({"location": {"at": "segments", "parts": [
                   {"source": {"source": 2, "lifetime": "session", "generation": 0,
                               "range": {"start": 2, "end": 4}}},
                   {"source": {"source": 1, "lifetime": "session", "generation": 0,
                               "range": {"start": 0, "end": 2}}}]},
               "lifetime": "session", "channels": 1, "frames": 4, "sample_rate": 48000.0})
    }

    #[test]
    fn a_relative_path_resolves_against_the_sessions_own_folder() {
        let dir = tmp("relative");
        let name = wav(&dir, "take.wav");
        let session = session(&[7], json!({"7": file(&name)}));
        let load = plan(&session, &dir, &|p| p.exists(), &mut counting(0));
        assert!(load.unresolved.is_empty(), "{:?}", load.unresolved);
        assert_eq!(load.takes.get(SourceId(7)).map(|t| t.bufnum), Some(0));
        let OscType::String(path) = &load.messages[0].args[1] else {
            panic!("a path");
        };
        assert_eq!(Path::new(path), dir.join("take.wav"), "beside the session");
        assert_eq!(load.messages[0].addr, "/buffer_allocRead");
        // **And the read is waited for**, on that very buffer.
        assert_eq!(
            load.steps()[1],
            Step::AwaitDone {
                command: "/buffer_allocRead".into(),
                index: Some(0),
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A source the table holds but nothing names is **not** loaded: reading a
    /// file nothing draws is work the person did not ask for, and a table
    /// outlives the boxes that used it.
    #[test]
    fn only_what_the_multitrack_names_is_read() {
        let session = session(
            &[1],
            json!({"1": file("used.wav"), "2": file("unused.wav")}),
        );
        let load = plan(&session, Path::new("."), &|_| true, &mut counting(0));
        assert_eq!(load.takes.len(), 1);
        assert!(load.takes.get(SourceId(2)).is_none());
    }

    /// Missing samples are **named**, not skipped: a box that will draw empty
    /// has a reason, and the reason is worth one line in the log.
    #[test]
    fn what_cannot_be_resolved_is_reported_with_why() {
        let dir = tmp("missing");
        let session = session(
            &[1, 2, 3],
            json!({
                "1": file("gone.wav"),
                "2": {"location": {"at": "volatile"}, "lifetime": "temporary"},
            }),
        );
        let load = plan(&session, &dir, &|p| p.exists(), &mut counting(0));
        assert!(load.takes.is_empty(), "nothing loadable");
        let why: Vec<&str> = load.unresolved.iter().map(|(_, w)| w.as_str()).collect();
        assert!(why[0].contains("is not there"), "{why:?}");
        assert!(why[1].contains("volatile"), "{why:?}");
        assert!(why[2].contains("source table"), "{why:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A join is stitched after the reads it is over**, and a take only the
    /// join reads is loaded for it.
    #[test]
    fn a_join_is_stitched_after_the_takes_it_is_made_of() {
        let session = session(
            &[3],
            json!({"1": file("one.wav"), "2": file("two.wav"), "3": swapped()}),
        );
        let load = plan(&session, Path::new("/takes"), &|_| true, &mut counting(100));
        assert!(load.unresolved.is_empty(), "{:?}", load.unresolved);
        let addrs: Vec<&str> = load.messages.iter().map(|m| m.addr.as_str()).collect();
        assert_eq!(
            addrs,
            ["/buffer_allocRead", "/buffer_allocRead", "/buffer_stitch"]
        );
        assert_eq!(load.takes.get(SourceId(3)).map(|t| t.bufnum), Some(102));
        assert_eq!(
            load.messages[2].args[3..6],
            [OscType::Int(101), OscType::Int(2), OscType::Int(2)],
            "the second take's tail first"
        );
    }

    /// The client door: the numbers it was given, the steps in order, what was
    /// not taken handed back -- and no filesystem asked.
    #[test]
    fn the_door_plans_with_the_callers_numbers() {
        let written = serde_json::to_value(session(
            &[3, 4],
            json!({"1": file("one.wav"), "2": file("two.wav"), "3": swapped()}),
        ))
        .unwrap();
        let answer: Value = serde_json::from_str(&plan_json(
            &json!({"session": written, "beside": "/nowhere", "buffers": [5, 6, 7, 8, 9]})
                .to_string(),
        ))
        .unwrap();
        assert_eq!(answer["takes"]["1"]["buffer"], 5);
        assert_eq!(answer["takes"]["3"]["buffer"], 7);
        assert_eq!(answer["takes"]["3"]["frames"], 4);
        assert_eq!(answer["unused"], json!([8, 9]));
        assert_eq!(
            answer["unresolved"],
            json!([[4, "the source table does not hold it"]])
        );
        let steps = answer["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 6, "a send and its wait per buffer: {steps:?}");

        let refused: Value = serde_json::from_str(&plan_json(r#"{"session": 3}"#)).unwrap();
        assert!(refused["error"].is_string());
    }
}
