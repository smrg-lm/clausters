//! **From a multitrack to the nodes that play it** -- the instance plan.
//!
//! [`clausters_core::mixer`] says what a track and a clip *are* on the server;
//! this says which of them a given [`Multitrack`] needs, wired to which
//! buffers, at which frames, with which levels. Between the two there is
//! nothing left for a client to decide, which is the point: a multitrack plays the
//! same in both of them because neither of them works it out.
//!
//! What a caller still owns is what only a caller can know -- where a source's
//! samples actually are (a buffer number is a running server's fact, not a
//! document's) and what a node's id is. Everything in between is here.
//!
//! # Why the arithmetic is here and not in a client
//!
//! Three rules, and each was written twice before this module existed:
//!
//! - **Seconds to frames.** A region sits in seconds and a reader counts
//!   frames, so something has to cross, through the sample rate. Two crossings
//!   are two places for a box to land in a different place.
//! - **The mixer's rule about solo.** The document records that a track *was
//!   marked* soloed and deliberately stops there ([`Track::soloed`]); what a
//!   solo anywhere does to everything else is the mixer's, and this is the
//!   mixer.
//! - **Which slot a box goes in.** A mono take is panned into a track and a
//!   stereo take is balanced, so the source's width picks the clip def -- and a
//!   client that guessed would produce a multitrack that sounds different in the
//!   other client.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use clausters_core::mixer;
use clausters_core::tempomap::TempoMap;

use crate::multitrack::{Automation, Content, Multitrack, Track};
use crate::{NodeId, SourceId};

/// What a caller knows about a source that the document does not: where its
/// samples are on a running server, and how wide they are.
///
/// A buffer number is not a property of a multitrack -- the same multitrack opened twice
/// has two of them -- which is exactly why the document holds a source id and a
/// session table holds this.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// The server buffer holding the samples.
    pub buffer: i32,
    /// How many channels it has, which is what decides the clip's wiring.
    pub channels: usize,
}

/// One reader: one channel of one box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedReader {
    /// Which channel of the source it takes.
    pub channel: usize,
    /// The buffer it reads.
    pub buffer: i32,
    /// Where the box begins on the transport, in frames.
    pub at: f64,
    /// How long it lasts, in frames.
    pub span: f64,
    /// The **second of the source** its own zero reads.
    ///
    /// Seconds, because the frame that is depends on the rate those samples
    /// were written at and the buffer is what knows it: the reader crosses it
    /// with `BufSampleRate` (`mixer::START`). A plan that crossed it here
    /// would need every source's rate to say a number the server already has.
    pub start: f64,
    /// Whether the window wraps past the end of the source.
    pub looping: bool,
    /// **How fast it reads its source**, as a factor over the source's own
    /// pitch -- the region's `playrate` and nothing else. The rest of the
    /// reading speed is the source's rate against the engine's, which the
    /// reader takes off the buffer (`mixer::RATE`).
    pub rate: f64,
}

/// One curve, ready to be heard: the port it drives and the table a reader
/// follows.
///
/// **The table is sampled here** rather than handed over as break-points,
/// because sampling it is arithmetic and arithmetic written twice is two
/// answers. What a caller does with it is write it into a buffer and start a
/// `mt.curve` over it -- which is the only part that needs a running server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedCurve {
    /// The automation this is, by the identity the document gives it.
    pub id: NodeId,
    /// The port it drives, resolved against the instance it is on.
    pub port: String,
    /// Where the table's first sample sits on the transport, in frames.
    pub at: f64,
    /// How many frames one sample of the table covers.
    pub step: f64,
    /// The values, one per `step` frames. Before the first and after the last a
    /// reader clamps -- which is a curve holding its ends, and what every
    /// automation does.
    pub table: Vec<f32>,
}

/// One clip: a box, its strip and its readers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedClip {
    /// The region this plays, which is the identity the editor knows it by.
    pub region: NodeId,
    /// The slot of its track it is added to -- the source's width picks it.
    pub slot: String,
    /// Its own gain, before the track's.
    pub gain: f32,
    /// `1.0` when this box alone is silenced.
    pub mute: f32,
    /// One per channel of the source.
    pub readers: Vec<PlannedReader>,
    /// The curves over **this box alone** -- its own gain, its fades.
    pub curves: Vec<PlannedCurve>,
}

/// One track: its strip and the clips on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedTrack {
    /// The track this is, by the identity the document gives it.
    pub track: NodeId,
    /// How wide it is.
    pub channels: usize,
    /// Its fader, linear.
    pub gain: f32,
    /// `1.0` when the mixer's rule silences it -- its own mute, or somebody
    /// else's solo.
    pub mute: f32,
    /// The clips to add to it, in the order they are on the timeline.
    pub clips: Vec<PlannedClip>,
    /// The curves over the track.
    pub curves: Vec<PlannedCurve>,
}

/// The whole multitrack as instances: one graph, and everything else a slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// The graph to instantiate -- `mt.multitrack.<channels>`.
    pub graph: String,
    /// How wide the master is.
    pub channels: usize,
    /// The tracks, in the order the document shows them.
    pub tracks: Vec<PlannedTrack>,
    /// Every `(source width, track width)` pair the multitrack uses, which is what
    /// [`clausters_core::mixer::defs_for`] is handed.
    pub widths: Vec<(usize, usize)>,
}

/// **The port an automation drives**, or `None` for one that names nothing.
///
/// [`Automation::target`] is opaque -- "in the client's terms and never read
/// here" -- and that is right: what a curve drives is a name in the surface of
/// whatever it is on, and the crate does not own that vocabulary either. What it
/// *does* own is the shape the multitrack editor writes there, which is
/// `{"port": "gain"}` and nothing else. A target that says something else is a
/// curve this cannot hear, and it is left out rather than guessed at.
pub fn curve_port(automation: &Automation) -> Option<&str> {
    automation.target.0.get("port")?.as_str()
}

/// What a break-point curve says at `at`, holding its ends.
///
/// **Each segment as its first point shapes it**, through
/// [`clausters_core::envshape::shape_value`] -- the function the GUI host draws
/// the same curve with, so what is heard is what is on screen. The shape rides
/// in [`crate::Point::data`] as the multitrack editor writes it (`shape`, an
/// envelope shape number, and `curve`); a point that states none is linear.
///
/// It used to be linear always, on the grounds that `data` is opaque here. The
/// editor had already settled what those two keys mean -- its projection draws
/// them and its reading writes them -- so a bent segment was drawn bent and
/// heard straight (found 2026-09-13, by ear).
fn value_at(points: &[crate::Point], at: f64) -> f64 {
    let first = &points[0];
    if at <= first.at {
        return first.value;
    }
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if at < b.at {
            let span = b.at - a.at;
            if span <= 0.0 {
                return b.value;
            }
            let data = a.data.0.as_object();
            let number = |key: &str| {
                data.and_then(|d| d.get(key))
                    .and_then(serde_json::Value::as_f64)
            };
            let shape =
                number("shape").map_or(clausters_core::envshape::SHAPE_LINEAR, |s| s as i32);
            let curve = number("curve").unwrap_or(0.0) as f32;
            return f64::from(clausters_core::envshape::shape_value(
                shape,
                curve,
                a.value as f32,
                b.value as f32,
                ((at - a.at) / span) as f32,
            ));
        }
    }
    points[points.len() - 1].value
}

/// The curves over one thing, as tables on the **frame** axis.
///
/// `origin` is the second the curve's own axis starts at: a track's automation
/// is on the timeline and a clip's is the box's own time, which is the whole
/// difference between the two places a curve lives.
fn curves(automation: &[Automation], origin: f64, step: f64, rate: f64) -> Vec<PlannedCurve> {
    let frames = |secs: f64| secs * rate;
    let mut out = Vec::new();
    for curve in automation {
        if !curve.enabled || curve.points.is_empty() {
            continue;
        }
        let Some(port) = curve_port(curve) else {
            continue;
        };
        let first = frames(origin + curve.points[0].at);
        let last = frames(origin + curve.points[curve.points.len() - 1].at);
        let table: Vec<f32> = if last <= first || step <= 0.0 {
            vec![curve.points[0].value as f32]
        } else {
            let count = ((last - first) / step).ceil() as usize + 1;
            (0..count)
                .map(|i| {
                    let at = (first + i as f64 * step) / rate - origin;
                    value_at(&curve.points, at) as f32
                })
                .collect()
        };
        out.push(PlannedCurve {
            id: curve.id,
            port: port.to_string(),
            at: first,
            step,
            table,
        });
    }
    out
}

/// **What a track's fader is at**, as the mixer wants it.
///
/// [`Track::level`] is the number and this is only its width: a multitrack is read
/// in `f64` because that is what a document says and played in `f32` because
/// that is what a control is.
pub fn track_gain(track: &Track) -> f32 {
    crate::multitrack::picture::level_of(track) as f32
}

/// **What silences a track**: its own mute, or somebody else's solo.
///
/// The document records that a track was *marked* soloed and stops there,
/// because what a mark does to everything else is a rule about a mixer and not
/// a fact about a multitrack. This is that rule, in one place: with nothing soloed
/// every unmuted track plays; with anything soloed, only the soloed ones do,
/// and a track that is both soloed and muted is still muted -- a mute is a
/// statement about *this* track and a solo is a statement about the others.
pub fn track_mute(multitrack: &Multitrack, track: &Track) -> f32 {
    let soloing = multitrack.tracks.iter().any(|t| t.soloed);
    let silent = track.muted || (soloing && !track.soloed);
    if silent { 1.0 } else { 0.0 }
}

/// The tempo map a multitrack holds, as the shared core holds one: what a
/// ruler draws its beats and bars from and a snap to them reads. It places
/// nothing, since a multitrack is in seconds.
///
/// A multitrack that never said a tempo did not say one, and the document
/// refuses to invent one -- that would be the format deciding a musical
/// question. So the default arrives from the caller, in beats per second.
pub fn tempo_map(multitrack: &Multitrack, default_tempo: f64) -> TempoMap {
    let changes: Vec<clausters_core::tempomap::TempoChange> = multitrack
        .tempo
        .iter()
        .map(|t| clausters_core::tempomap::TempoChange {
            beats: t.at.get(),
            tempo: t.tempo,
            ramp: t.ramp,
        })
        .collect();
    TempoMap::from_changes(&changes, default_tempo).unwrap_or_else(|_| TempoMap::new(default_tempo))
}

/// **The plan for a multitrack**: what to instantiate, wired to what, at what frame.
///
/// `sources` answers where a source's samples are; a region whose source it
/// does not know is left out of the plan rather than planned as silence, so a
/// take that has not finished loading is simply not playing yet and the multitrack
/// is otherwise whole.
///
/// No tempo is asked for: every position is in seconds, so the plan is the
/// sample rate's alone.
pub fn plan(
    multitrack: &Multitrack,
    sample_rate: f64,
    sources: &HashMap<SourceId, SourceInfo>,
) -> Plan {
    let frames = |secs: f64| secs * sample_rate;
    let mut widths: Vec<(usize, usize)> = Vec::new();
    let mut tracks = Vec::new();

    for track in &multitrack.tracks {
        let channels = track.channels.max(1);
        let mut clips = Vec::new();
        let Some(lane) = track.active_lane() else {
            tracks.push(PlannedTrack {
                track: track.id,
                channels,
                gain: track_gain(track),
                mute: track_mute(multitrack, track),
                clips,
                curves: curves(&track.automation, 0.0, mixer::CURVE_STEP, sample_rate),
            });
            continue;
        };
        for region in &lane.regions {
            let Content::Window {
                window,
                looping,
                playrate,
                ..
            } = &region.content
            else {
                // A window onto a node of the document is content the multitrack
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
                    // The window's own start is in the source's **seconds**,
                    // the unit a recording measures in and no tempo scales --
                    // the same one `picture::Box::start` reports and both
                    // clients write -- and it crosses to a frame against the
                    // buffer's own rate, which is the reader's to ask.
                    start: window.start,
                    looping: *looping,
                    rate: *playrate,
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
                // A box's curves are the box's own time, so they start where it
                // does: a fade drawn at its beginning is at *its* zero and not
                // at the multitrack's.
                curves: curves(
                    &region.automation,
                    region.position.get(),
                    mixer::CURVE_STEP,
                    sample_rate,
                ),
            });
        }
        tracks.push(PlannedTrack {
            track: track.id,
            channels,
            gain: track_gain(track),
            mute: track_mute(multitrack, track),
            clips,
            // A track's curves are on the timeline, which is the whole
            // difference between the two places a curve lives.
            curves: curves(&track.automation, 0.0, mixer::CURVE_STEP, sample_rate),
        });
    }

    let channels = multitrack.channels.max(1);
    Plan {
        graph: mixer::multitrack_name(channels),
        channels,
        tracks,
        widths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multitrack::Tempo;
    use crate::multitrack::{Lane, Region};
    use crate::timebase::{Beat, Second};
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
        Region::new(NodeId(id), Second(at), Second(len), content)
    }

    fn multitrack() -> Multitrack {
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

    /// **A box lands at its seconds, in frames.** The second box starts two
    /// seconds in, which is two seconds of frames and not two of anything else.
    #[test]
    fn a_box_is_planned_in_frames_off_its_seconds() {
        let plan = plan(&multitrack(), 48_000.0, &sources());
        let clips = &plan.tracks[0].clips;
        assert_eq!(clips[0].readers[0].at, 0.0);
        assert_eq!(clips[0].readers[0].span, 2.0 * 48_000.0);
        assert_eq!(clips[1].readers[0].at, 2.0 * 48_000.0);
        assert_eq!(
            clips[0].readers[0].start, 0.0,
            "and the window's own start is in the source's seconds, crossed by the reader"
        );
    }

    /// **The window's start stays in seconds and the playrate reaches the
    /// reader.** The frame a second lands on depends on the rate those samples
    /// were written at, which the buffer knows and this does not: a plan that
    /// crossed it here would read a 44.1 kHz take from the wrong frame in a
    /// 48 kHz session, by 8.8%. What the plan does say is the box's own
    /// playrate, which is the half of the reading speed that is a decision.
    #[test]
    fn a_window_reaches_the_reader_in_seconds_with_its_playrate() {
        let mut multitrack = multitrack();
        let region = &mut multitrack.tracks[0].lanes[0].regions[0];
        let Content::Window {
            window, playrate, ..
        } = &mut region.content
        else {
            panic!("a window");
        };
        window.start = 1.5;
        *playrate = 2.0;

        let plan = plan(&multitrack, 48_000.0, &sources());
        let reader = &plan.tracks[0].clips[0].readers[0];
        assert_eq!(reader.start, 1.5, "a second and a half of the source");
        assert_eq!(reader.rate, 2.0, "read twice as fast");
        // And the placement is untouched: how fast a box reads its source says
        // nothing about where it sits or how long it lasts.
        assert_eq!(reader.at, 0.0);
        assert_eq!(reader.span, 2.0 * 48_000.0);
    }

    /// **A tempo moves no box.** The multitrack is in seconds, and the tempo map
    /// it holds is read by a ruler and a snap: a plan with a ritardando in the
    /// map is the plan without one.
    #[test]
    fn a_tempo_in_the_multitrack_moves_nothing_in_the_plan() {
        let mut slower = multitrack();
        slower.set_tempo(Tempo::at(Beat(0.0), 2.0));
        slower.set_tempo(Tempo::at(Beat(1.0), 0.5).ramping());
        assert_eq!(
            plan(&slower, 48_000.0, &sources()),
            plan(&multitrack(), 48_000.0, &sources())
        );
    }

    /// **The source's width picks the slot**, because a mono take is panned
    /// into the track and a stereo one is balanced -- one reader against two.
    #[test]
    fn the_source_width_picks_the_slot_and_the_readers() {
        let plan = plan(&multitrack(), 48_000.0, &sources());
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
    /// multitrack is whole.
    #[test]
    fn a_source_with_no_buffer_is_left_out() {
        let mut table = sources();
        table.remove(&SourceId(2));
        let plan = plan(&multitrack(), 48_000.0, &table);
        assert_eq!(plan.tracks[0].clips.len(), 1);
        assert_eq!(plan.widths, vec![(1, 2)]);
    }

    /// **A solo anywhere silences everything else**, which is the mixer's rule
    /// and the reason the document only records the mark.
    #[test]
    fn a_solo_anywhere_silences_the_tracks_that_are_not() {
        let mut p = multitrack();
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

    /// **A track that said nothing about its level is at full**, and a multitrack
    /// written before the fader was a field still plays at the level it was
    /// saved with.
    #[test]
    fn a_tracks_fader_is_its_own_field_and_an_old_one_still_reads() {
        let mut p = multitrack();
        assert_eq!(track_gain(&p.tracks[0]), 1.0);
        p.tracks[0].level = 0.25;
        assert_eq!(track_gain(&p.tracks[0]), 0.25);
        p.tracks[0].level = 1.0;
        p.tracks[0].config = crate::Opaque(serde_json::json!({"level": 0.5}));
        assert_eq!(track_gain(&p.tracks[0]), 0.5);
    }
}

#[cfg(test)]
mod json_tests {
    use super::*;

    /// **The plan reads the multitrack a client writes**, including one that left out
    /// a field it had nothing to say about.
    ///
    /// The two structures are one format in two languages, and a field that
    /// round-trips in the type and not through the JSON is exactly the kind of
    /// gap nothing else catches. This one is worth a test rather than a
    /// comment: [`SegmentSource`](crate::SegmentSource) is untagged, so a
    /// `SourceRef` missing a defaulted field does not fail -- the window quietly
    /// becomes opaque content, drawn as a box and played by nothing.
    #[test]
    fn a_multitrack_written_by_a_client_plans() {
        let written = r#"{"tracks":[{"id":1,"lanes":[{"id":2,"regions":[
            {"id":3,"position":0.0,"length":2.0,"content":{"fill":"window",
             "window":{"source":{"source":1,"lifetime":"session"},
                       "start":0.0,"duration":2.0}}}]}]}]}"#;
        let multitrack: Multitrack = serde_json::from_str(written).expect("it reads");
        assert_eq!(multitrack.tracks.len(), 1);
        assert_eq!(
            multitrack.tracks[0]
                .active_lane()
                .expect("a lane")
                .regions
                .len(),
            1
        );

        let table = HashMap::from([(
            crate::SourceId(1),
            SourceInfo {
                buffer: 7,
                channels: 1,
            },
        )]);
        let plan = plan(&multitrack, 48_000.0, &table);
        assert_eq!(plan.tracks[0].clips.len(), 1, "the box is planned");
        assert_eq!(plan.tracks[0].clips[0].readers[0].buffer, 7);
    }
}

#[cfg(test)]
mod curve_tests {
    use super::*;
    use crate::multitrack::{Automation, Lane, Track};
    use crate::timebase::Second;

    fn curve(id: u64, target: serde_json::Value, points: &[(f64, f64)]) -> Automation {
        Automation {
            target: crate::Opaque(target),
            points: points
                .iter()
                .map(|&(at, value)| crate::Point {
                    at,
                    value,
                    data: crate::Opaque::none(),
                })
                .collect(),
            ..Automation::new(NodeId(id), crate::Opaque::none())
        }
    }

    fn track_with(automation: Vec<Automation>) -> Multitrack {
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.lanes[0] = Lane::new(NodeId(2));
        track.automation = automation;
        Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        }
    }

    /// **A curve becomes a table on the frame axis, and it holds its ends.** A
    /// reader clamps past either end, which is what an automation does: before
    /// its first point it is at the first value and after its last at the last.
    #[test]
    fn a_curve_is_sampled_into_a_table_that_holds_its_ends() {
        let multitrack = track_with(vec![curve(
            10,
            serde_json::json!({"port": "gain"}),
            &[(0.0, 0.0), (1.0, 1.0)],
        )]);
        let plan = plan(&multitrack, 48_000.0, &HashMap::new());
        let [table] = &plan.tracks[0].curves[..] else {
            panic!("one curve, got {:?}", plan.tracks[0].curves.len())
        };
        assert_eq!(table.port, "gain");
        assert_eq!(table.at, 0.0, "it starts at its first point");
        assert_eq!(table.step, mixer::CURVE_STEP);
        // One second, sampled every 64 frames.
        assert_eq!(table.table.len(), 48_000 / 64 + 1);
        assert!(table.table[0].abs() < 1e-6, "it starts at its first value");
        assert!((table.table[table.table.len() - 1] - 1.0).abs() < 1e-3);
        // Linear between the two, so the middle is the middle.
        let middle = table.table[table.table.len() / 2];
        assert!(
            (middle - 0.5).abs() < 0.02,
            "linear between points: {middle}"
        );
    }

    /// **A bent segment is heard bent** (found 2026-09-13, by ear: an envelope
    /// bent on screen changed nothing in the sound). The segment takes the shape
    /// its first point states, through the function the host draws it with.
    #[test]
    fn a_curve_is_sampled_with_the_shape_its_points_state() {
        let mut bent = curve(
            10,
            serde_json::json!({"port": "gain"}),
            &[(0.0, 0.0), (1.0, 1.0)],
        );
        bent.points[0].data = crate::Opaque(serde_json::json!({"shape": 5, "curve": 4.0}));
        let multitrack = track_with(vec![bent]);
        let plan = plan(&multitrack, 48_000.0, &HashMap::new());
        let table = &plan.tracks[0].curves[0].table;
        let middle = table[table.len() / 2];
        let drawn = clausters_core::envshape::shape_value(5, 4.0, 0.0, 1.0, 0.5);
        assert!(
            (middle - drawn).abs() < 0.02,
            "the table follows the drawn shape: {middle} against {drawn}"
        );
        assert!((middle - 0.5).abs() > 0.1, "and not the straight line");
    }

    /// **A curve that names nothing is not heard.** The target is opaque and the
    /// crate owns only one shape in it; anything else is a curve this cannot
    /// resolve, and guessing at it would drive a port nobody asked for.
    #[test]
    fn a_curve_whose_target_names_no_port_is_left_out() {
        let multitrack = track_with(vec![
            curve(10, serde_json::json!({"plugin": 3}), &[(0.0, 1.0)]),
            curve(11, serde_json::Value::Null, &[(0.0, 1.0)]),
        ]);
        let plan = plan(&multitrack, 48_000.0, &HashMap::new());
        assert!(plan.tracks[0].curves.is_empty());
    }

    /// **A curve switched off is kept and not heard**, which is what
    /// arming one means: the multitrack still holds it, and nothing drives the port.
    #[test]
    fn a_disabled_curve_is_not_planned() {
        let mut held = curve(10, serde_json::json!({"port": "gain"}), &[(0.0, 1.0)]);
        held.enabled = false;
        let multitrack = track_with(vec![held]);
        assert!(
            plan(&multitrack, 48_000.0, &HashMap::new()).tracks[0]
                .curves
                .is_empty()
        );
    }

    /// **A box's curve is the box's own time.** A fade drawn at the start of a
    /// clip two seconds in begins two seconds in, not at the top of the timeline --
    /// which is the whole difference between the two places a curve lives.
    #[test]
    fn a_clips_curve_starts_where_the_clip_does() {
        let mut multitrack = track_with(Vec::new());
        let mut region = crate::multitrack::Region::new(
            NodeId(3),
            Second(2.0),
            Second(1.0),
            Content::Window {
                window: crate::SegmentRef {
                    source: crate::SegmentSource::Samples(crate::SourceRef {
                        source: crate::SourceId(1),
                        lifetime: crate::Lifetime::Session,
                        generation: 0,
                        range: None,
                    }),
                    start: 0.0,
                    duration: 1.0,
                },
                playrate: 1.0,
                args: crate::Opaque::none(),
                looping: false,
            },
        );
        region.automation = vec![curve(
            20,
            serde_json::json!({"port": "gain"}),
            &[(0.0, 0.0), (1.0, 1.0)],
        )];
        multitrack.tracks[0].lanes[0].regions = vec![region];
        let table = HashMap::from([(
            crate::SourceId(1),
            SourceInfo {
                buffer: 5,
                channels: 1,
            },
        )]);
        let plan = plan(&multitrack, 48_000.0, &table);
        let curve = &plan.tracks[0].clips[0].curves[0];
        assert_eq!(curve.at, 2.0 * 48_000.0, "two seconds in, in frames");
    }
}
