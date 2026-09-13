//! The session's source table, resolved to **server buffers**.
//!
//! A document says what plays when and deliberately never says where its
//! samples are; a session's `sources` table is that other half, and this is
//! where a host turns it into something it can draw and edit. The answer is a
//! server buffer per source, loaded with `/buffer_allocRead`.
//!
//! # Why a buffer and not a mapped file
//!
//! The host can already map a file of raw `f32` and draw it with no server at
//! all (the `path` bulk route), which is cheaper and needs nothing running. It
//! is the wrong route here for two reasons, and neither is about drawing.
//!
//! A session's samples are **audio the user brought** — a WAV, a FLAC, an
//! MP3 — and decoding those is the server's job and nobody else's here (it
//! decodes by content, through hound and symphonia). Mapping one as raw floats
//! draws the header as a click.
//!
//! And what is drawn has to be what is **edited**: a destructive edit is
//! `/buffer_gain`, `/buffer_setRange`, `/buffer_reverse` — commands addressed
//! to a buffer, run on the server's own NRT thread. A host drawing a mapped
//! file and editing a buffer would be showing one copy and writing another,
//! which is the two-owner problem again with the samples in place of the tree.
//!
//! So the picture and the samples are one thing: a source becomes a buffer,
//! the clip draws that buffer, and an edit writes the very samples on screen.
//!
//! # What it does not do
//!
//! It does not write anything back. Loading is one-way here — the file is the
//! user's and a session must never rewrite it (the format says so about
//! absolute paths), so a confirmed destructive edit is a *save* somewhere else,
//! and that decision belongs with the open-edit machinery in the crate rather
//! than with the loader.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use clausters_core::ids::{IdSpaces, Space};
use clausters_core::osc::{OscMessage, OscType};
use clausters_document::session::{Location, Session, Source};
use clausters_document::{Body, SourceId};
use clausters_editing::sources as projection;

/// One source, as the host holds it once it has been given to the server.
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

    /// Every buffer number the sources resolved to, in source order. What a
    /// **player** has to be told about: it maps the buffer directory once,
    /// when it starts, and these takes were read after that.
    pub fn bufnums(&self) -> Vec<i32> {
        self.map.values().map(|take| take.bufnum).collect()
    }

    /// **Every source and what it resolved to**, for a caller that needs the
    /// table rather than one entry — the instance plan's, which is asked what
    /// each source's buffer and width are before anything can play.
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
    /// The map a drawing consults.
    pub takes: Takes,
    /// The `/buffer_allocRead` messages, in source order. The caller sends
    /// them — this module builds no server and owns no link.
    pub messages: Vec<OscMessage>,
    /// Sources the tree names that could not be resolved, with why. Reported
    /// rather than swallowed: a clip with no samples is going to draw as an
    /// empty rectangle, and the reader deserves to know it is missing rather
    /// than empty.
    pub unresolved: Vec<(SourceId, String)>,
}

impl Load {
    /// **The load as steps**: each read and each join followed by the `/done`
    /// of that very buffer, in the order they were planned -- so a join is
    /// stitched after the reads it is made of, whoever walks them
    /// ([`clausters_editing::run::Runner`]).
    ///
    /// Sent as a batch, the stitch reached the session while its reads were
    /// still running: `part 0: buffer 1 is not allocated`, and a join that drew
    /// empty and could not be attached on the player.
    pub fn steps(&self) -> Vec<clausters_editing::apply::Step> {
        use clausters_editing::apply::Step;

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

/// Plans the load of every source the document actually names.
///
/// `beside` is the session file's own folder, which is what a relative path is
/// resolved against — the rule that makes a session directory movable, and the
/// format's own words rather than this host's convention.
///
/// The buffers come from `ids`, the host's one buffer space, so what goes on
/// allocating after the load -- a curve's table, a join made by a hand -- never
/// writes over a take. Numbers are handed out in source-id order, which makes
/// the same session load the same way twice.
pub fn plan(session: &Session, beside: &Path, ids: &mut IdSpaces) -> Load {
    let mut load = Load::default();
    for id in referenced(session) {
        let Some(source) = session.source(id) else {
            // The session's own `dangling` says this too; saying it here keeps
            // the caller from having to ask twice for one report.
            load.unresolved
                .push((id, "the source table does not hold it".into()));
            continue;
        };
        match locate(source, beside) {
            Ok(path) => {
                let bufnum = match ids.alloc(Space::Buffers, 1) {
                    Ok(bufnum) => bufnum as i32,
                    Err(e) => {
                        load.unresolved.push((id, e.to_string()));
                        continue;
                    }
                };
                load.takes.map.insert(
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
    stitch(session, &mut load, ids);
    load
}

/// Where a source's file is, absolute, or why it is nowhere.
fn locate(source: &Source, beside: &Path) -> Result<PathBuf, String> {
    match &source.location {
        Location::File { path } if path.is_empty() => Err("the source names no file".to_string()),
        Location::File { path } => {
            let p = Path::new(path);
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                beside.join(p)
            };
            if full.exists() {
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
/// pass as the files. This is the second pass, and it repeats: a join may be
/// over a join (the server allows four levels), so a round that installs
/// something makes the next round able to install more, and a round that
/// installs nothing is the end of what is reachable.
///
/// What is left over is **named rather than dropped**: a join over a take that
/// did not load is a box that will draw empty, and the reader deserves the
/// reason.
fn stitch(session: &Session, load: &mut Load, ids: &mut IdSpaces) {
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
        waiting.retain(|id| {
            let Some(source) = session.source(*id) else {
                return false;
            };
            // **What a join is** is the projection's
            // ([`clausters_editing::sources::stitch`]), which every endpoint
            // that realizes one asks -- the widths, the spans, the channel map
            // a narrow part fills the join with. What is left here is the
            // buffer number, which is this host's, and the wire.
            let Some(made) = projection::stitch(source, &held_takes(&load.takes)) else {
                // A part that has not landed yet is what the next round is for;
                // anything else is not a join and never will be.
                return matches!(source.location, Location::Segments { .. });
            };
            let Ok(bufnum) = ids.alloc(Space::Buffers, 1) else {
                return true;
            };
            let bufnum = bufnum as i32;
            load.takes.insert(
                *id,
                Take {
                    bufnum,
                    channels: Some(made.channels as u32),
                    frames: Some(made.frames),
                },
            );
            load.messages.push(stitch_message(bufnum, &made));
            installed = true;
            false
        });
        if !installed || waiting.is_empty() {
            break;
        }
    }
    for id in waiting {
        load.unresolved
            .push((id, "a join over samples that did not load".into()));
    }
}

/// The table as the projection asks for it: what each source resolved to.
pub fn held_takes(takes: &Takes) -> HashMap<SourceId, projection::Held> {
    takes
        .iter()
        .map(|(id, take)| {
            (
                *id,
                projection::Held {
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
/// The one place this host turns a resolved join into the wire, so the open
/// pass and an edit that mints one send the same message.
pub fn stitch_message(bufnum: i32, made: &projection::Stitch) -> OscMessage {
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

/// Every source the tree actually names, in a stable order.
///
/// The table may hold more than the document uses (a source of a deleted clip
/// still has its row until something prunes it), and loading those would read
/// files nothing draws.
fn referenced(session: &Session) -> Vec<SourceId> {
    let mut found: Vec<SourceId> = Vec::new();
    // **The piece names sources too**, and a session written today names them
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
                let Some(source) = window.source.samples() else {
                    continue;
                };
                if !found.contains(&source.source) {
                    found.push(source.source);
                }
            }
        }
    }
    session.document.walk(&mut |node| {
        // Assembled samples names one source per window; a reader that took
        // only the first would open a joined clip with the rest of it silent.
        let named: Vec<SourceId> = match &node.body {
            Body::Vector { source, .. } => vec![source.source],
            Body::Segments { segments, .. } => segments
                .iter()
                .filter_map(|s| s.source.samples())
                .map(|source| source.source)
                .collect(),
            _ => Vec::new(),
        };
        for source in named {
            if !found.contains(&source) {
                found.push(source);
            }
        }
    });
    found.sort_by_key(|id| id.0);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::{Document, Lifetime, Member, Node, NodeId, Opaque, SourceRef};

    fn take_node(id: u64, source: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Vector {
                source: SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                config: Default::default(),
            },
        )
    }

    fn aggregate(id: u64, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Aggregate {
                grouping: clausters_document::Grouping::Concrete,
                members,
                config: Opaque::none(),
            },
        )
    }

    fn at(offset: f64, node: Node) -> Member {
        Member {
            offset,
            dur: None,
            node,
        }
    }

    fn wav(dir: &Path, name: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, b"not really a wav, but it is there").expect("write");
        name.to_string()
    }

    fn spaces() -> IdSpaces {
        IdSpaces::new(
            clausters_core::ids::ServerShape::DEFAULT,
            clausters_core::ids::IdShare::WHOLE,
        )
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "clausters_gui_sources_{tag}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn a_relative_path_resolves_against_the_sessions_own_folder() {
        let dir = tmp("relative");
        let name = wav(&dir, "take.wav");
        let session = Session::new(Document::new(aggregate(1, vec![at(0.0, take_node(2, 7))])))
            .with_source(SourceId(7), Source::file(name, Lifetime::Session));
        let load = plan(&session, &dir, &mut spaces());
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
            clausters_editing::apply::Step::AwaitDone {
                command: "/buffer_allocRead".into(),
                index: Some(0),
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A source the table holds but the tree never names is **not** loaded:
    /// reading a file nothing draws is work the person did not ask for, and a
    /// table outlives the clips that used it.
    #[test]
    fn only_what_the_tree_names_is_read() {
        let dir = tmp("unused");
        let used = wav(&dir, "used.wav");
        let unused = wav(&dir, "unused.wav");
        let session = Session::new(Document::new(aggregate(1, vec![at(0.0, take_node(2, 1))])))
            .with_source(SourceId(1), Source::file(used, Lifetime::Session))
            .with_source(SourceId(2), Source::file(unused, Lifetime::Session));
        let load = plan(&session, &dir, &mut spaces());
        assert_eq!(load.takes.len(), 1);
        assert!(load.takes.get(SourceId(2)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Missing samples are **named**, not skipped: a clip that will draw empty
    /// has a reason, and the reason is worth one line in the log.
    #[test]
    fn what_cannot_be_resolved_is_reported_with_why() {
        let dir = tmp("missing");
        let session = Session::new(Document::new(aggregate(
            1,
            vec![
                at(0.0, take_node(2, 1)),
                at(1.0, take_node(3, 2)),
                at(2.0, take_node(4, 3)),
            ],
        )))
        .with_source(SourceId(1), Source::file("gone.wav", Lifetime::Session))
        .with_source(SourceId(2), Source::volatile(Lifetime::Temporary));
        let load = plan(&session, &dir, &mut spaces());
        assert!(load.takes.is_empty(), "nothing loadable");
        let why: Vec<&str> = load.unresolved.iter().map(|(_, w)| w.as_str()).collect();
        assert!(why[0].contains("is not there"), "{why:?}");
        assert!(why[1].contains("volatile"), "{why:?}");
        assert!(why[2].contains("source table"), "{why:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Buffers come from the host's one space, past what it already holds, in
    /// source order, so the same session loads the same way twice.
    #[test]
    fn buffers_are_allocated_from_the_host_s_space_in_order() {
        let dir = tmp("order");
        let a = wav(&dir, "a.wav");
        let b = wav(&dir, "b.wav");
        let session = Session::new(Document::new(aggregate(
            1,
            // Named out of order in the tree, on purpose.
            vec![at(0.0, take_node(2, 9)), at(1.0, take_node(3, 4))],
        )))
        .with_source(SourceId(9), Source::file(a, Lifetime::Session))
        .with_source(SourceId(4), Source::file(b, Lifetime::Session));
        let mut ids = spaces();
        let held = ids.alloc(Space::Buffers, 100).unwrap();
        let load = plan(&session, &dir, &mut ids);
        assert_eq!(load.takes.get(SourceId(4)).map(|t| t.bufnum), Some(100));
        assert_eq!(load.takes.get(SourceId(9)).map(|t| t.bufnum), Some(101));
        assert_eq!(held, 0, "past what the host already held");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
