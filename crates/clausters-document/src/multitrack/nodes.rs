//! **From a piece to the nodes that play it** — the instance plan.
//!
//! [`clausters_core::mixer`] says what a track and a clip *are* on the server;
//! this says which of them a given [`Multitrack`] needs, wired to which
//! buffers, at which frames, with which levels. Between the two there is
//! nothing left for a client to decide, which is the point: a piece plays the
//! same in both of them because neither of them works it out.
//!
//! What a caller still owns is what only a caller can know — where a source's
//! samples actually are (a buffer number is a running server's fact, not a
//! document's) and what a node's id is. Everything in between is here.
//!
//! # Why the arithmetic is here and not in a client
//!
//! Three rules, and each was written twice before this module existed:
//!
//! - **Beats to frames.** A region sits on the musical axis and a reader counts
//!   frames, so something has to cross, through the tempo map and the sample
//!   rate. Two crossings are two places for a box to land in a different place.
//! - **The mixer's rule about solo.** The document records that a track *was
//!   marked* soloed and deliberately stops there ([`Track::soloed`]); what a
//!   solo anywhere does to everything else is the mixer's, and this is the
//!   mixer.
//! - **Which slot a box goes in.** A mono take is panned into a track and a
//!   stereo take is balanced, so the source's width picks the clip def -- and a
//!   client that guessed would produce a piece that sounds different in the
//!   other client.

use std::collections::HashMap;

use clausters_core::mixer;
use clausters_core::tempomap::TempoMap;

use crate::multitrack::{Content, Multitrack, Track};
use crate::{NodeId, SourceId};

/// What a caller knows about a source that the document does not: where its
/// samples are on a running server, and how wide they are.
///
/// A buffer number is not a property of a piece — the same piece opened twice
/// has two of them — which is exactly why the document holds a source id and a
/// session table holds this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceInfo {
    /// The server buffer holding the samples.
    pub buffer: i32,
    /// How many channels it has, which is what decides the clip's wiring.
    pub channels: usize,
}

/// One reader: one channel of one box.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedReader {
    /// Which channel of the source it takes.
    pub channel: usize,
    /// The buffer it reads.
    pub buffer: i32,
    /// Where the box begins on the transport, in frames.
    pub at: f64,
    /// How long it lasts, in frames.
    pub span: f64,
    /// The first frame of the source it reads.
    pub start: f64,
    /// Whether the window wraps past the end of the source.
    pub looping: bool,
}

/// One clip: a box, its strip and its readers.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedClip {
    /// The region this plays, which is the identity the editor knows it by.
    pub region: NodeId,
    /// The slot of its track it is added to — the source's width picks it.
    pub slot: String,
    /// Its own gain, before the track's.
    pub gain: f32,
    /// `1.0` when this box alone is silenced.
    pub mute: f32,
    /// One per channel of the source.
    pub readers: Vec<PlannedReader>,
}

/// One track: its strip and the clips on it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedTrack {
    /// The track this is, by the identity the document gives it.
    pub track: NodeId,
    /// How wide it is.
    pub channels: usize,
    /// `1.0` when the mixer's rule silences it — its own mute, or somebody
    /// else's solo.
    pub mute: f32,
    /// The clips to add to it, in the order they are on the timeline.
    pub clips: Vec<PlannedClip>,
}

/// The whole piece as instances: one graph, and everything else a slot.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The graph to instantiate — `mt.piece.<channels>`.
    pub graph: String,
    /// How wide the master is.
    pub channels: usize,
    /// The tracks, in the order the document shows them.
    pub tracks: Vec<PlannedTrack>,
    /// Every `(source width, track width)` pair the piece uses, which is what
    /// [`clausters_core::mixer::defs_for`] is handed.
    pub widths: Vec<(usize, usize)>,
}

/// **What silences a track**: its own mute, or somebody else's solo.
///
/// The document records that a track was *marked* soloed and stops there,
/// because what a mark does to everything else is a rule about a mixer and not
/// a fact about a piece. This is that rule, in one place: with nothing soloed
/// every unmuted track plays; with anything soloed, only the soloed ones do,
/// and a track that is both soloed and muted is still muted — a mute is a
/// statement about *this* track and a solo is a statement about the others.
pub fn track_mute(piece: &Multitrack, track: &Track) -> f32 {
    let soloing = piece.tracks.iter().any(|t| t.soloed);
    let silent = track.muted || (soloing && !track.soloed);
    if silent { 1.0 } else { 0.0 }
}

/// The tempo map a piece measures its beats against.
///
/// A piece that never said a tempo did not say one, and the document refuses to
/// invent 120 — that would be the format deciding a musical question. So the
/// default arrives from the caller, who is the one with a reason to have it.
fn tempo_map(piece: &Multitrack, default_bpm: f64) -> TempoMap {
    let changes: Vec<clausters_core::tempomap::TempoChange> = piece
        .tempo
        .iter()
        .map(|t| clausters_core::tempomap::TempoChange {
            beats: t.at.get(),
            // The map's tempo is beats per **second** and a piece writes beats
            // per minute; the division is here, once, where the field is read.
            tempo: t.bpm / 60.0,
            ramp: t.ramp,
        })
        .collect();
    // The map counts beats per **second** and a piece writes beats per minute,
    // here as everywhere else in this function.
    let default = default_bpm / 60.0;
    TempoMap::from_changes(&changes, default).unwrap_or_else(|_| TempoMap::new(default))
}

/// **The plan for a piece**: what to instantiate, wired to what, at what frame.
///
/// `sources` answers where a source's samples are; a region whose source it
/// does not know is left out of the plan rather than planned as silence, so a
/// take that has not finished loading is simply not playing yet and the piece
/// is otherwise whole.
///
/// `default_bpm` is the tempo, in beats per minute, that a piece which never
/// stated one is read at — the caller's, for the reason above.
pub fn plan(
    piece: &Multitrack,
    sample_rate: f64,
    default_bpm: f64,
    sources: &HashMap<SourceId, SourceInfo>,
) -> Plan {
    let map = tempo_map(piece, default_bpm);
    let frames = |beat: f64| map.secs_at(beat) * sample_rate;
    let mut widths: Vec<(usize, usize)> = Vec::new();
    let mut tracks = Vec::new();

    for track in &piece.tracks {
        let channels = track.channels.max(1);
        let mut clips = Vec::new();
        let Some(lane) = track.active_lane() else {
            tracks.push(PlannedTrack {
                track: track.id,
                channels,
                mute: track_mute(piece, track),
                clips,
            });
            continue;
        };
        for region in &lane.regions {
            let Content::Window {
                window, looping, ..
            } = &region.content
            else {
                // A window onto a node of the document is content the piece
                // holds rather than samples, and nothing reads one yet. Named
                // rather than silently dropped: it is the score's road in.
                continue;
            };
            let Some(source) = window.source.samples() else {
                continue;
            };
            let Some(info) = sources.get(&source.source).copied() else {
                continue;
            };
            let at = frames(region.position.get());
            let end = frames(region.position.get() + region.length.get());
            let readers = (0..info.channels.max(1))
                .map(|channel| PlannedReader {
                    channel,
                    buffer: info.buffer,
                    at,
                    span: (end - at).max(0.0),
                    // The window's own start is in the source's addressing
                    // unit, which for samples is the frame -- the one place a
                    // number in this document is not on the musical axis.
                    start: window.start,
                    looping: *looping,
                })
                .collect();
            let pair = (info.channels.max(1), channels);
            if !widths.contains(&pair) {
                widths.push(pair);
            }
            clips.push(PlannedClip {
                region: region.id,
                slot: mixer::clip_slot(info.channels.max(1)),
                gain: 1.0,
                mute: if region.muted { 1.0 } else { 0.0 },
                readers,
            });
        }
        tracks.push(PlannedTrack {
            track: track.id,
            channels,
            mute: track_mute(piece, track),
            clips,
        });
    }

    let channels = piece.channels.max(1);
    Plan {
        graph: mixer::piece_name(channels),
        channels,
        tracks,
        widths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multitrack::{Lane, Region};
    use crate::timebase::Beat;
    use crate::{Lifetime, SegmentRef, SegmentSource, SourceRef};

    fn sources() -> HashMap<SourceId, SourceInfo> {
        HashMap::from([
            (
                SourceId(1),
                SourceInfo {
                    buffer: 10,
                    channels: 1,
                },
            ),
            (
                SourceId(2),
                SourceInfo {
                    buffer: 11,
                    channels: 2,
                },
            ),
        ])
    }

    fn region(id: u64, source: u64, at: f64, len: f64) -> Region {
        let content = Content::Window {
            window: SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start: 0.0,
                duration: len,
            },
            playrate: 1.0,
            args: crate::Opaque::none(),
            looping: false,
        };
        Region::new(NodeId(id), Beat(at), Beat(len), content)
    }

    fn piece() -> Multitrack {
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.lanes[0] = Lane {
            regions: vec![region(3, 1, 0.0, 2.0), region(4, 2, 2.0, 2.0)],
            ..Lane::new(NodeId(2))
        };
        Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        }
    }

    /// **A box lands where the tempo map says it does.** At 60 bpm a beat is a
    /// second; the second box starts two beats in, which is two seconds of
    /// frames and not two of anything else.
    #[test]
    fn a_box_is_planned_in_frames_off_the_musical_axis() {
        let plan = plan(&piece(), 48_000.0, 60.0, &sources());
        let clips = &plan.tracks[0].clips;
        assert_eq!(clips[0].readers[0].at, 0.0);
        assert_eq!(clips[0].readers[0].span, 2.0 * 48_000.0);
        assert_eq!(clips[1].readers[0].at, 2.0 * 48_000.0);
    }

    /// **The source's width picks the slot**, because a mono take is panned
    /// into the track and a stereo one is balanced — one reader against two.
    #[test]
    fn the_source_width_picks_the_slot_and_the_readers() {
        let plan = plan(&piece(), 48_000.0, 60.0, &sources());
        let clips = &plan.tracks[0].clips;
        assert_eq!(clips[0].slot, mixer::clip_slot(1));
        assert_eq!(clips[0].readers.len(), 1);
        assert_eq!(clips[1].slot, mixer::clip_slot(2));
        assert_eq!(clips[1].readers.len(), 2);
        assert_eq!(clips[1].readers[1].channel, 1);
        assert_eq!(plan.widths, vec![(1, 2), (2, 2)]);
    }

    /// **A source nobody can find is not planned**, rather than planned as
    /// silence: a take still loading is not playing yet, and the rest of the
    /// piece is whole.
    #[test]
    fn a_source_with_no_buffer_is_left_out() {
        let mut table = sources();
        table.remove(&SourceId(2));
        let plan = plan(&piece(), 48_000.0, 60.0, &table);
        assert_eq!(plan.tracks[0].clips.len(), 1);
        assert_eq!(plan.widths, vec![(1, 2)]);
    }

    /// **A solo anywhere silences everything else**, which is the mixer's rule
    /// and the reason the document only records the mark.
    #[test]
    fn a_solo_anywhere_silences_the_tracks_that_are_not() {
        let mut p = piece();
        p.tracks.push(Track::new(NodeId(5), NodeId(6)));
        assert_eq!(
            track_mute(&p, &p.tracks[0]),
            0.0,
            "nothing soloed, all play"
        );

        p.tracks[1].soloed = true;
        assert_eq!(track_mute(&p, &p.tracks[0]), 1.0, "the others go quiet");
        assert_eq!(track_mute(&p, &p.tracks[1]), 0.0);

        p.tracks[1].muted = true;
        assert_eq!(
            track_mute(&p, &p.tracks[1]),
            1.0,
            "a mute is about this track and a solo about the others, so a mute wins"
        );
    }
}
