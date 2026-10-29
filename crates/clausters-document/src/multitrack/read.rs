//! **A track as its reader reads it**: the regions of its active take lane,
//! each placed on the track's frames at a rate, as the arithmetic the core
//! reads a region with ([`clausters_core::regions`]).
//!
//! A multitrack plays each track as one take the application reads from
//! disk: every region that sounds, summed where they overlap, through its
//! window, its playrate, its loop, its edges and its gain curve. This is
//! where a region becomes those numbers, once, at the rate the reader runs
//! at; what a source is on disk (a file, the parts of a join) is the
//! reader's to know, and a source this is not told the shape of is left out,
//! as a take still being measured is not playing yet.
//!
//! **A track is as wide as its widest region**: a region plays by the
//! implicit map, a mono source on every channel of its track and a wider one
//! channel by channel, so the track is as many channels as the widest source
//! it plays ([`track_width`]).

use std::collections::HashMap;

use clausters_core::mixer;
use clausters_core::regions::{Curve, RegionCut};
use clausters_core::segments::FadeShape;

use crate::multitrack::{Content, Multitrack, Track};
use crate::{NodeId, SourceId};

/// **What a reader knows of a source**: how wide it is, how many frames it
/// holds and the rate they were written at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceShape {
    /// How many channels.
    pub channels: usize,
    /// How many frames, at its own rate.
    pub frames: u64,
    /// The rate it was written at.
    pub rate: f64,
}

/// **One region, placed on its track's frames**: the source it reads and
/// what it reads of it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedRegion {
    /// The region, by its identity.
    pub region: NodeId,
    /// The source it reads.
    pub source: SourceId,
    /// What it reads of it, on the track's frames.
    pub cut: RegionCut,
}

/// **A track's take**: its width, its regions in the order they start, and
/// how long it lasts on the track.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackRead {
    /// The track.
    pub track: NodeId,
    /// How wide it is.
    pub channels: usize,
    /// The regions that sound, by where they start.
    pub regions: Vec<PlacedRegion>,
    /// One past the last frame any of them sounds on.
    pub frames: u64,
}

/// **How wide `track` is**: as wide as the widest source its active take
/// lane plays, as `shapes` knows them; what it says it is when it plays none
/// this knows.
pub fn track_width(track: &Track, shapes: &HashMap<SourceId, SourceShape>) -> usize {
    track
        .active_take_lane()
        .into_iter()
        .flat_map(|lane| &lane.regions)
        .filter_map(|region| match &region.content {
            Content::Window { window, .. } => shapes.get(&window.source.source),
            _ => None,
        })
        .map(|shape| shape.channels.max(1))
        .max()
        .unwrap_or_else(|| track.channels.max(1))
}

/// **Every track of `multitrack` as a take**, at `rate` frames a second.
///
/// A region reads its source from its window's start (a second of the
/// source), at the source's rate over `rate` times its playrate, wrapping
/// when it loops; a region that does not loop ends where its source does. Its
/// edges are its own or the multitrack's default, each at most its span, and
/// its gain follows its own gain curve. A muted region, a box of notes (in
/// `voiced`), a fill this build does not read and a source `shapes` does not
/// know are left out.
pub fn track_reads(
    multitrack: &Multitrack,
    rate: f64,
    shapes: &HashMap<SourceId, SourceShape>,
    voiced: &std::collections::BTreeSet<NodeId>,
) -> Vec<TrackRead> {
    let frames = |secs: f64| secs * rate;
    multitrack
        .tracks
        .iter()
        .map(|track| {
            let channels = track_width(track, shapes);
            let mut regions = Vec::new();
            if let Some(lane) = track.active_take_lane() {
                for region in &lane.regions {
                    if region.muted || voiced.contains(&region.id) {
                        continue;
                    }
                    let Content::Window {
                        window,
                        looping,
                        playrate,
                        ..
                    } = &region.content
                    else {
                        continue;
                    };
                    let source = window.source.source;
                    let Some(shape) = shapes.get(&source).copied() else {
                        continue;
                    };
                    let source_rate = if shape.rate > 0.0 { shape.rate } else { rate };
                    let at = frames(region.position.get());
                    let mut span = frames(region.length.get()).max(0.0);
                    // **The window ends where the source does**, when it does
                    // not loop: what is left past the window's start, read at
                    // the playrate, is as long as the region may sound.
                    if !*looping && *playrate > 0.0 {
                        let left = (shape.frames as f64 / source_rate - window.start).max(0.0);
                        span = span.min(frames(left / *playrate));
                    }
                    let (edge_in, edge_out) = lane.edges(region, &multitrack.defaults);
                    let gain = crate::multitrack::nodes::curves(
                        &region.automation,
                        region.position.get(),
                        mixer::CURVE_STEP,
                        rate,
                    )
                    .into_iter()
                    .find(|curve| curve.port == mixer::GAIN)
                    .map(|curve| Curve {
                        at: curve.at,
                        step: curve.step,
                        table: curve.table.into(),
                    });
                    let cut = RegionCut {
                        at,
                        span,
                        start: window.start * source_rate,
                        step: source_rate / rate * *playrate,
                        looping: *looping,
                        fade_in: frames(edge_in.length).min(span),
                        fade_out: frames(edge_out.length).min(span),
                        fade_in_shape: FadeShape {
                            shape: edge_in.shape,
                            curve: edge_in.curve as f32,
                        },
                        fade_out_shape: FadeShape {
                            shape: edge_out.shape,
                            curve: edge_out.curve as f32,
                        },
                        map: RegionCut::implicit_map(shape.channels.max(1), channels),
                        gain,
                    };
                    regions.push(PlacedRegion {
                        region: region.id,
                        source,
                        cut,
                    });
                }
            }
            regions.sort_by(|a, b| a.cut.at.total_cmp(&b.cut.at));
            let end = regions
                .iter()
                .map(|r| (r.cut.at + r.cut.span).max(0.0).ceil() as u64)
                .max()
                .unwrap_or(0);
            TrackRead {
                track: track.id,
                channels,
                regions,
                frames: end,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multitrack::{Region, TakeLane, Window};
    use crate::timebase::Second;
    use crate::{Lifetime, SourceRef};

    fn region(id: u64, source: u64, at: f64, len: f64) -> Region {
        let content = Content::Window {
            window: Window {
                source: SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                start: 0.5,
                duration: len,
            },
            playrate: 1.0,
            args: crate::Opaque::none(),
            looping: false,
        };
        Region::new(NodeId(id), Second(at), Second(len), content)
    }

    fn shapes() -> HashMap<SourceId, SourceShape> {
        HashMap::from([
            (
                SourceId(1),
                SourceShape {
                    channels: 1,
                    frames: 48_000,
                    rate: 48_000.0,
                },
            ),
            (
                SourceId(2),
                SourceShape {
                    channels: 2,
                    frames: 44_100 * 10,
                    rate: 44_100.0,
                },
            ),
        ])
    }

    /// A track is as wide as its widest region, and each region is placed
    /// in frames of the engine, its source read at the ratio of the rates and
    /// cut where its source ends.
    #[test]
    fn a_track_reads_its_regions_at_the_engine_s_rate() {
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.channels = 2;
        track.take_lanes[0] = TakeLane {
            regions: vec![region(4, 2, 2.0, 2.0), region(3, 1, 0.0, 2.0)],
            ..TakeLane::new(NodeId(2))
        };
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let reads = track_reads(&multitrack, 48_000.0, &shapes(), &Default::default());
        let read = &reads[0];
        assert_eq!(read.channels, 2);
        assert_eq!(read.regions[0].region, NodeId(3), "by where they start");
        let mono = &read.regions[0].cut;
        // half a second of a one-second source is left past its window's start
        assert_eq!(mono.span, 24_000.0);
        assert_eq!(mono.map, [0, 0], "a mono source on both channels");
        let stereo = &read.regions[1].cut;
        assert_eq!(stereo.at, 96_000.0);
        assert_eq!(stereo.start, 22_050.0);
        assert!((stereo.step - 44_100.0 / 48_000.0).abs() < 1e-12);
        assert_eq!(read.frames, 192_000);
    }

    /// A track of mono regions is mono.
    #[test]
    fn a_track_of_mono_regions_is_mono() {
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.take_lanes[0] = TakeLane {
            regions: vec![region(3, 1, 0.0, 1.0)],
            ..TakeLane::new(NodeId(2))
        };
        assert_eq!(track_width(&track, &shapes()), 1);
        let empty = Track::new(NodeId(5), NodeId(6));
        assert_eq!(track_width(&empty, &shapes()), empty.channels);
    }
}
