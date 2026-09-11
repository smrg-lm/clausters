//! **Sounding the piece**: a reader per region, following the transport.
//!
//! The multitrack application's playback, and it lives here rather than in a
//! script because there is one of it. Every client that drives a multitrack
//! needs the same three rules — where a region's reader sits, how long it
//! lasts, and what a track's strip does to its level — and each of them written
//! in Python and again in TypeScript is the divergence the project's standing
//! rule is about. The clients say what the piece *is*; this makes it sound.
//!
//! # Nothing here computes time
//!
//! A region is a **resident node** whose phase is `TransportPos(offset)`, so it
//! seeks when the transport seeks, loops when it loops and holds when it stops,
//! with nothing sent per pass and no position of its own. There is no queue, no
//! onset scan and no re-cue: *"is this under the cursor"* is the reader's own
//! arithmetic (`docs/architecture.md`, "Playback time in a session: read, never
//! computed").
//!
//! # An edit reaches a node that is already running
//!
//! Moving a region while the piece plays is one `/node_set` of `offset` on a
//! live node, so what is sounding is not cut and nothing is re-cued. That is
//! the whole reason this diffs rather than rebuilding: a piece is a statement,
//! and *making the sound be what is drawn* is one verb whether it is the first
//! time or the hundredth — but freeing and re-creating every reader to say it
//! would click on every drag.
//!
//! # What a region has to be for this to reach it
//!
//! A window onto **samples** that somebody resolved to a server buffer. A
//! window onto notes fires voices instead and keeps a queue, which is the half
//! of `C54` this does not dissolve; a composite is a tree and is the audio
//! editor's. Both are drawn and neither sounds here, which is a gap with a
//! name rather than a silence.

use std::collections::HashMap;

use clausters_core::osc::{OscMessage, OscType};
use clausters_document::NodeId;
use clausters_document::multitrack::{Content, Multitrack, Region, Track};

use crate::host::Host;

/// One sounding reader: which region it plays, and which channel of it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Voice {
    /// The region it reads.
    pub region: NodeId,
    /// Which channel of the samples, which is also the bus it goes out on.
    pub channel: u32,
}

/// What a reader is set to: everything a `/node_set` would carry.
///
/// Compared rather than remembered as a message, so a node that has not moved
/// costs nothing on the wire — which is what keeps a drag from setting every
/// other region on the lane.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Reading {
    /// The server buffer.
    pub bufnum: i32,
    /// Where the region starts on the piece's timeline, in frames.
    pub offset: f64,
    /// The frame of the source its own zero reads.
    pub start: f64,
    /// How long it lasts, in frames.
    pub span: f64,
    /// Its level, after the mixer's three rules.
    pub amp: f64,
}

/// The most channels one region is played over — the same bound the take
/// monitor keeps, and for the same reason: a malformed channel count must not
/// fill the node tree.
const MAX_CHANNELS: u32 = 32;

/// **What the piece should sound like**, region by region.
///
/// A pure function of the piece and its resolved samples, so it is testable
/// without a server and cannot disagree with the picture: both are derived from
/// the same `Multitrack`.
pub fn reading(piece: &Multitrack, look: &super::piece::Look<'_>) -> Vec<(Voice, Reading)> {
    let soloing = piece.tracks.iter().any(|t| t.soloed);
    let mut out = Vec::new();
    for track in &piece.tracks {
        let amp = gain(track, soloing);
        let Some(lane) = track.active_lane().or_else(|| track.lanes.first()) else {
            continue;
        };
        for region in &lane.regions {
            let Some(Samples {
                bufnum,
                channels,
                start,
                content,
            }) = sounds(region, look)
            else {
                continue;
            };
            // **A region's own mute is the region's**, and it is not the
            // track's: a box silenced by hand stays silenced when the track is
            // unmuted, which is why the two are separate fields.
            let amp = if region.muted { 0.0 } else { amp };
            for channel in 0..channels.min(MAX_CHANNELS) {
                out.push((
                    Voice {
                        region: region.id,
                        channel,
                    },
                    Reading {
                        bufnum,
                        offset: look.frame_at(region.position.0),
                        start,
                        // **A region may be longer than what fills it**, and
                        // then it is silent for the rest: the gate closes at
                        // whichever ends first. Without this the reader runs
                        // past its window, `BufRd` clamps and the last sample
                        // is held -- a tone where the piece has nothing, which
                        // is exactly what a four-beat region over two seconds
                        // of audio sounded like.
                        span: look
                            .frames_over(region.position.0, region.length.0)
                            .min(content),
                        amp,
                    },
                ));
            }
        }
    }
    out
}

/// **What a track contributes**: nothing when it is muted, nothing when another
/// is soloed, its fader otherwise.
///
/// The mixer's three rules, and they are here rather than in a client for the
/// reason the module doc gives: the document carries the flags and says nothing
/// about what they mean, so whoever *reads* them decides — and there has to be
/// one reader, or two clients decide differently.
pub fn gain(track: &Track, soloing: bool) -> f64 {
    if track.muted || (soloing && !track.soloed) {
        return 0.0;
    }
    clausters_document::multitrack::picture::level_of(track).max(0.0)
}

/// What a region reads: the buffer, its shape, where its own zero opens, and
/// **how much there is** — the four things a reader needs and the document has.
struct Samples {
    bufnum: i32,
    channels: u32,
    /// The frame of the source the region's zero reads.
    start: f64,
    /// How long the window lasts, in frames of the timeline.
    content: f64,
}

/// What a region plays — `None` for one that is drawn but does not sound here.
fn sounds(region: &Region, look: &super::piece::Look<'_>) -> Option<Samples> {
    let Content::Window { window, .. } = &region.content else {
        return None;
    };
    let source = window.source.samples()?;
    let take = look.takes?.get(source.source)?;
    // The window's own start and duration are in **seconds** — a recording's
    // units — so they meet the timeline through the rate and never through the
    // tempo.
    Some(Samples {
        bufnum: take.bufnum,
        channels: take.channels.unwrap_or(1).max(1),
        start: window.start * look.rate,
        content: window.duration * look.rate,
    })
}

impl Host {
    /// **Makes what sounds be what the piece says.**
    ///
    /// One call, whether it is the first time or after any edit: a reader that
    /// is new is created, one that moved is `/node_set`, one whose region is
    /// gone is freed, and one that did not change costs nothing. Returns how
    /// many readers the piece has.
    ///
    /// It is a no-op for a host with no piece and for one with no server —
    /// a session opens, edits, undoes and saves without either.
    pub fn sound_piece(&mut self) -> usize {
        let Some(owner) = self.owner.as_ref() else {
            return 0;
        };
        if !owner.draws_piece() || self.player().is_none() {
            return 0;
        }
        let want: HashMap<Voice, Reading> = reading(&owner.piece, &owner.piece_look())
            .into_iter()
            .collect();
        let mut messages = Vec::new();
        // Gone: the regions that were removed, or stopped naming samples.
        let leaving: Vec<Voice> = self
            .sounding
            .keys()
            .filter(|voice| !want.contains_key(voice))
            .copied()
            .collect();
        for voice in leaving {
            if let Some((node, _)) = self.sounding.remove(&voice) {
                messages.push(OscMessage {
                    addr: "/node_free".into(),
                    args: vec![OscType::Int(node)],
                });
            }
        }
        for (voice, reading) in want {
            match self.sounding.get_mut(&voice) {
                // **Already running**: an edit is a set on a live node, so
                // nothing that is sounding is cut and nothing is re-cued.
                Some((node, was)) if *was != reading => {
                    let node = *node;
                    *was = reading;
                    messages.push(set_message(node, &reading));
                }
                Some(_) => {}
                None => {
                    let node = self.next_piece_node();
                    self.sounding.insert(voice, (node, reading));
                    messages.push(new_message(node, voice, &reading));
                }
            }
        }
        tracing::debug!(
            "sound_piece: {} message(s), {} reader(s)",
            messages.len(),
            self.sounding.len()
        );
        for message in messages {
            self.send_to_player(message);
        }
        self.sounding.len()
    }

    /// Frees every reader the piece has — what closing a window owes the
    /// server, and what a host that stops owning a piece owes it.
    pub fn hush_piece(&mut self) {
        let nodes: Vec<i32> = self.sounding.values().map(|(node, _)| *node).collect();
        self.sounding.clear();
        for node in nodes {
            self.send_to_player(OscMessage {
                addr: "/node_free".into(),
                args: vec![OscType::Int(node)],
            });
        }
    }

    /// How many readers the piece is sounding through, for a caller reporting
    /// what it built.
    pub fn sounding_count(&self) -> usize {
        self.sounding.len()
    }

    /// **Rolls or freezes the piece**, answering which it did.
    ///
    /// One verb, because the transport has one: `stop` freezes the governed
    /// group with every reader's state intact and `play` thaws it, so pressing
    /// twice *continues* rather than starting the piece over. There is no
    /// "load" step and nothing to re-cue — the readers are resident and the
    /// position is the engine's.
    ///
    /// `None` when there is no piece sounding, which is what tells the caller
    /// to fall through to whatever else the key meant.
    pub fn roll_piece(&mut self) -> Option<bool> {
        if self.sounding.is_empty() {
            return None;
        }
        self.piece_rolling = !self.piece_rolling;
        tracing::info!(
            "the piece is {}",
            if self.piece_rolling {
                "rolling"
            } else {
                "frozen"
            }
        );
        self.send_to_player(OscMessage {
            addr: if self.piece_rolling {
                "/transport_play".into()
            } else {
                "/transport_stop".into()
            },
            args: vec![],
        });
        Some(self.piece_rolling)
    }

    /// Whether the piece is rolling.
    pub fn piece_rolling(&self) -> bool {
        self.piece_rolling
    }

    /// The next node id for a piece reader.
    ///
    /// Counted up rather than derived from the region, because a region id is
    /// the document's and a node id is the server's: a piece opened twice would
    /// otherwise ask one server for the same node twice.
    fn next_piece_node(&mut self) -> i32 {
        let node = self.next_piece_node;
        self.next_piece_node += 1;
        node
    }
}

/// The `/synth_new` that starts a reader, in the group the transport governs.
fn new_message(node: i32, voice: Voice, reading: &Reading) -> OscMessage {
    let mut args = vec![
        OscType::String(super::super::play::TAKE_DEF.into()),
        OscType::Int(node),
        OscType::Int(1),                                // add to the tail…
        OscType::Int(super::super::play::take_group()), // …of the piece's own group
        OscType::String("chan".into()),
        OscType::Float(voice.channel as f32),
        OscType::String("out".into()),
        OscType::Float(voice.channel as f32),
    ];
    args.extend(settings(reading));
    OscMessage {
        addr: "/synth_new".into(),
        args,
    }
}

/// The `/node_set` an edit leaves — everything a placement can move, and
/// nothing else: which channel a reader is on never changes, because a region
/// that changed shape is a different set of voices.
fn set_message(node: i32, reading: &Reading) -> OscMessage {
    let mut args = vec![OscType::Int(node)];
    args.extend(settings(reading));
    OscMessage {
        addr: "/node_set".into(),
        args,
    }
}

fn settings(reading: &Reading) -> Vec<OscType> {
    vec![
        OscType::String("bufnum".into()),
        OscType::Float(reading.bufnum as f32),
        OscType::String("offset".into()),
        OscType::Float(reading.offset as f32),
        OscType::String("start".into()),
        OscType::Float(reading.start as f32),
        OscType::String("span".into()),
        OscType::Float(reading.span as f32),
        OscType::String("amp".into()),
        OscType::Float(reading.amp as f32),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::{Content, Region};
    use clausters_document::{
        Beat, Lifetime, Opaque, SegmentRef, SegmentSource, SourceId, SourceRef,
    };

    use crate::host::document::piece::Look;
    use crate::host::document::sources::Take;
    use crate::host::document::sources::Takes;
    use clausters_core::tempomap::TempoMap;

    fn window(source: u64, start: f64) -> Content {
        Content::Window {
            window: SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start,
                duration: 2.0,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        }
    }

    fn takes() -> Takes {
        let mut takes = Takes::default();
        takes.insert(
            SourceId(1),
            Take {
                bufnum: 7,
                channels: Some(2),
                frames: Some(96_000),
            },
        );
        takes
    }

    /// A hundred frames a beat, forty-eight thousand a second — the two scales
    /// stay apart so a length converted through the wrong one is obvious.
    fn look(takes: &Takes) -> Look<'_> {
        Look {
            tempo: TempoMap::new(480.0),
            rate: 48_000.0,
            takes: Some(takes),
        }
    }

    fn piece() -> Multitrack {
        let mut first = Track::new(NodeId(10), NodeId(11));
        first.name = Some("one".into());
        first.lanes[0].regions = vec![Region::new(
            NodeId(12),
            Beat(2.0),
            Beat(4.0),
            window(1, 0.5),
        )];
        let mut second = Track::new(NodeId(20), NodeId(21));
        second.lanes[0].regions = vec![Region::new(
            NodeId(22),
            Beat(0.0),
            Beat(2.0),
            window(1, 0.0),
        )];
        Multitrack {
            tracks: vec![first, second],
            ..Multitrack::default()
        }
    }

    /// **A reader per channel of every region**, placed in frames: the position
    /// through the beat, the window into the samples through the *rate*, since
    /// a recording's seconds are a wall-clock fact no tempo scales.
    #[test]
    fn every_region_reads_its_own_window_at_its_own_place() {
        let resolved = takes();
        let want = reading(&piece(), &look(&resolved));
        assert_eq!(want.len(), 4, "two regions, stereo: {want:?}");
        let (_, first) = want
            .iter()
            .find(|(v, _)| v.region == NodeId(12) && v.channel == 0)
            .expect("the first region's left channel");
        assert_eq!(first.bufnum, 7);
        assert_eq!(first.offset, 200.0, "two beats at a hundred frames each");
        assert_eq!(first.span, 400.0, "four beats long");
        // **A region longer than what fills it is silent for the rest.** The
        // window here is two seconds and the region four beats; shorten the
        // window and the gate closes with it rather than holding the last
        // sample, which is a tone where the piece has nothing.
        // A beat a second here, so the two lengths are comparable by eye.
        let mut brief = piece();
        if let Content::Window { window, .. } = &mut brief.tracks[0].lanes[0].regions[0].content {
            window.duration = 1.0;
        }
        let held = takes();
        let seconds = Look {
            tempo: TempoMap::new(1.0), // a beat a second, so the two are comparable
            rate: 48_000.0,
            takes: Some(&held),
        };
        let (_, clipped) = reading(&brief, &seconds)
            .into_iter()
            .find(|(v, _)| v.region == NodeId(12) && v.channel == 0)
            .expect("still a reader");
        assert_eq!(
            clipped.span, 48_000.0,
            "one second of window under a four-beat placement: the window wins"
        );
        assert_eq!(
            first.start,
            0.5 * 48_000.0,
            "half a second into the file, through the rate"
        );
        assert_eq!(first.amp, 1.0);
    }

    /// **The mixer's three rules, and there is one of them.** A muted track is
    /// silent, a soloed track anywhere silences the rest, and the fader is what
    /// is left — the rules the document carries flags for and says nothing
    /// about, so whoever reads them decides and there has to be one reader.
    #[test]
    fn a_strip_decides_what_a_track_contributes() {
        let amp_of = |piece: &Multitrack, track: u64| {
            let region = piece
                .tracks
                .iter()
                .find(|t| t.id == NodeId(track))
                .and_then(|t| t.lanes[0].regions.first())
                .map(|r| r.id)
                .expect("a region");
            reading(piece, &look(&takes()))
                .into_iter()
                .find(|(v, _)| v.region == region && v.channel == 0)
                .map(|(_, r)| r.amp)
                .expect("a reader")
        };

        let mut muted = piece();
        muted.tracks[0].muted = true;
        assert_eq!(amp_of(&muted, 10), 0.0, "muted");
        assert_eq!(amp_of(&muted, 20), 1.0, "and only that one");

        let mut soloed = piece();
        soloed.tracks[1].soloed = true;
        assert_eq!(amp_of(&soloed, 10), 0.0, "a solo silences the others");
        assert_eq!(amp_of(&soloed, 20), 1.0);

        let mut faded = piece();
        faded.tracks[0].level = 0.25;
        assert_eq!(amp_of(&faded, 10), 0.25);

        // A region's own mute is the region's, and it survives its track being
        // unmuted -- which is why the two are separate fields.
        let mut hushed = piece();
        hushed.tracks[0].lanes[0].regions[0].muted = true;
        assert_eq!(amp_of(&hushed, 10), 0.0);
    }

    /// A region that does not name resolved samples is **drawn and silent**,
    /// with no reader and no complaint: a window onto notes fires voices, a
    /// composite is a tree, and a source nobody read in has nothing to play.
    #[test]
    fn a_region_with_no_samples_behind_it_sounds_through_nothing() {
        let whole = piece;
        let mut piece = whole();
        piece.tracks[0].lanes[0].regions[0].content = Content::Composite {
            node: Box::new(clausters_document::Node::new(
                NodeId(99),
                clausters_document::Body::Clang {
                    config: Opaque::default(),
                    fires: None,
                },
            )),
        };
        let takes = takes();
        let want = reading(&piece, &look(&takes));
        assert!(
            want.iter().all(|(v, _)| v.region != NodeId(12)),
            "no reader for it: {want:?}"
        );
        assert_eq!(want.len(), 2, "and the other region still sounds");

        // ...and neither does one whose source nobody resolved.
        let fresh = whole();
        let nothing = Takes::default();
        let unresolved = reading(&fresh, &look(&nothing));
        assert!(unresolved.is_empty(), "{unresolved:?}");
    }
}
