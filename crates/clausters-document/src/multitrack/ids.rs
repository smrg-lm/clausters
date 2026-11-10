//! **A multitrack's ids**: one space, and what comes in is checked against it.
//!
//! A multitrack's tracks, take lanes, regions, markers and curves share one id
//! space: an intent addresses each by its id and does not say what kind of
//! thing it expected ([`Multitrack::locate`], [`Multitrack::automation`]), a
//! client keeps an object per id, and a view draws and reports each under it.
//! So an id names one thing.
//!
//! **An id is minted past everything named** ([`fresh`]): the multitrack keeps
//! no counter, so the next id is one past the highest anything holds, markers
//! and curves included.
//!
//! **What comes from outside is not trusted.** A session read from a file (one
//! saved by an older writer, or written by hand) is repaired as it is
//! read ([`repair`]): the first holder of an id keeps it, in the order tracks,
//! take lanes, regions, markers, curves, and every later one is given a new
//! id. An intent that names ids it brings (a track list, a take lane stated
//! whole, a split's halves, a join's result, a new marker) is checked before
//! it applies ([`check`]) and refused when one of them already names something
//! else: those ids are the caller's own, minted through the handle's `mint`,
//! and an intent is absolute: renaming what it brought would apply something
//! other than what was sent, and the caller would hold objects under ids that
//! no longer name them.

use std::collections::{HashMap, HashSet};

use super::Multitrack;
use super::edit::MultitrackIntent;
use crate::NodeId;

/// **What holds an id** in a multitrack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Holder {
    /// A track.
    Track,
    /// A take lane.
    TakeLane,
    /// A region.
    Region,
    /// A marker.
    Marker,
    /// An automation curve, a track's or a region's.
    Curve,
}

impl Holder {
    fn what(self) -> &'static str {
        match self {
            Self::Track => "a track",
            Self::TakeLane => "a take lane",
            Self::Region => "a region",
            Self::Marker => "a marker",
            Self::Curve => "a curve",
        }
    }
}

/// The order a repair hands ids out in: what an id names first keeps it.
const PRIORITY: [Holder; 5] = [
    Holder::Track,
    Holder::TakeLane,
    Holder::Region,
    Holder::Marker,
    Holder::Curve,
];

/// **An id a repair took away**, and the one it gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Renumbered {
    /// The id it came with.
    pub was: u64,
    /// The id it holds now.
    pub now: u64,
    /// What it is.
    pub holder: Holder,
}

/// Visits every id of one kind, mutably, in the multitrack's order.
fn visit(multitrack: &mut Multitrack, kind: Holder, f: &mut dyn FnMut(&mut NodeId)) {
    match kind {
        Holder::Marker => multitrack.markers.iter_mut().for_each(|m| f(&mut m.id)),
        _ => {
            for track in &mut multitrack.tracks {
                match kind {
                    Holder::Track => f(&mut track.id),
                    Holder::Curve => track.automation.iter_mut().for_each(|a| f(&mut a.id)),
                    _ => {}
                }
                for lane in &mut track.take_lanes {
                    if kind == Holder::TakeLane {
                        f(&mut lane.id);
                    }
                    for region in &mut lane.regions {
                        match kind {
                            Holder::Region => f(&mut region.id),
                            Holder::Curve => {
                                region.automation.iter_mut().for_each(|a| f(&mut a.id));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// The ids of some tracks and of what they hold, and of the markers beside
/// them, repeats included, in [`PRIORITY`] order, `except` leaving out what
/// one take lane holds.
fn ids_of(
    tracks: &[super::Track],
    markers: &[super::Marker],
    except: Option<NodeId>,
) -> Vec<(u64, Holder)> {
    let lanes = || tracks.iter().flat_map(|t| t.take_lanes.iter());
    let regions = || {
        lanes()
            .filter(|l| Some(l.id) != except)
            .flat_map(|l| l.regions.iter())
    };
    let mut out: Vec<(u64, Holder)> = tracks.iter().map(|t| (t.id.0, Holder::Track)).collect();
    out.extend(lanes().map(|l| (l.id.0, Holder::TakeLane)));
    out.extend(regions().map(|r| (r.id.0, Holder::Region)));
    out.extend(markers.iter().map(|m| (m.id.0, Holder::Marker)));
    out.extend(
        tracks
            .iter()
            .flat_map(|t| t.automation.iter())
            .chain(regions().flat_map(|r| r.automation.iter()))
            .map(|a| (a.id.0, Holder::Curve)),
    );
    out
}

/// Every id the multitrack holds and what holds it, repeats included, in the
/// order a repair hands them out.
fn every(multitrack: &Multitrack) -> Vec<(u64, Holder)> {
    ids_of(&multitrack.tracks, &multitrack.markers, None)
}

/// Every id the multitrack holds and what holds it: the first holder, where
/// one not yet repaired has two.
pub fn holders(multitrack: &Multitrack) -> HashMap<u64, Holder> {
    let mut out = HashMap::new();
    for (id, holder) in every(multitrack) {
        out.entry(id).or_insert(holder);
    }
    out
}

/// **An id nothing in the multitrack names**: one past the highest of its
/// tracks, take lanes, regions, markers and curves.
pub fn fresh(multitrack: &Multitrack) -> u64 {
    every(multitrack)
        .into_iter()
        .map(|(id, _)| id)
        .max()
        .unwrap_or(0)
        + 1
}

/// **A multitrack read from outside, repaired**: every id names one thing;
/// the first holder keeps it, in the order tracks, take lanes, regions,
/// markers, curves, and every later one is given an id past everything.
/// Says what it renumbered, and logs it.
pub fn repair(multitrack: &mut Multitrack) -> Vec<Renumbered> {
    let mut next = fresh(multitrack);
    let mut claimed: HashSet<u64> = HashSet::new();
    let mut renumbered = Vec::new();
    for kind in PRIORITY {
        visit(multitrack, kind, &mut |id| {
            if !claimed.insert(id.0) {
                renumbered.push(Renumbered {
                    was: id.0,
                    now: next,
                    holder: kind,
                });
                *id = NodeId(next);
                claimed.insert(next);
                next += 1;
            }
        });
    }
    if !renumbered.is_empty() {
        let listed: Vec<String> = renumbered
            .iter()
            .map(|r| format!("{} {} -> {}", r.holder.what(), r.was, r.now))
            .collect();
        tracing::warn!(
            "a multitrack read with ids that named two things was repaired: {}",
            listed.join(", ")
        );
    }
    renumbered
}

/// The first id `ids` says twice, said as a refusal.
fn twice(ids: impl IntoIterator<Item = (u64, Holder)>, context: &str) -> Result<(), String> {
    let mut seen: HashMap<u64, Holder> = HashMap::new();
    for (id, holder) in ids {
        if let Some(first) = seen.insert(id, holder) {
            return Err(format!(
                "id {id} names {} and {} {context}: an id names one thing, \
                 so a new one is minted for it",
                first.what(),
                holder.what()
            ));
        }
    }
    Ok(())
}

/// The ids of a list of regions and their curves.
fn region_ids(regions: &[super::Region]) -> Vec<(u64, Holder)> {
    regions
        .iter()
        .flat_map(|r| {
            std::iter::once((r.id.0, Holder::Region))
                .chain(r.automation.iter().map(|a| (a.id.0, Holder::Curve)))
        })
        .collect()
}

/// **Whether the ids an intent brings are free**: `Err` with the reason when
/// one already names something else in the multitrack, or the intent says one
/// twice. An intent that names only what is already there is not checked.
pub fn check(multitrack: &Multitrack, intent: &MultitrackIntent) -> Result<(), String> {
    match intent {
        MultitrackIntent::SetTracks { tracks } => twice(
            ids_of(tracks, &multitrack.markers, None),
            "once the tracks are these",
        ),
        MultitrackIntent::SetTakeLane { take_lane, regions } => twice(
            // everything but what this take lane holds now, and what it will
            ids_of(&multitrack.tracks, &multitrack.markers, Some(*take_lane))
                .into_iter()
                .chain(region_ids(regions)),
            "once the take lane holds these regions",
        ),
        MultitrackIntent::SplitRegion {
            region,
            left,
            right,
            ..
        } => {
            if multitrack.locate(*region).is_none() {
                return Ok(());
            }
            if left == right {
                return Err(format!("a split's two halves are both called {}", left.0));
            }
            let held = holders(multitrack);
            for half in [left, right] {
                if half != region
                    && let Some(holder) = held.get(&half.0)
                {
                    return Err(format!(
                        "a split's half is called {}, which is {} here",
                        half.0,
                        holder.what()
                    ));
                }
            }
            Ok(())
        }
        MultitrackIntent::JoinRegions { regions, into, .. } => {
            if regions.contains(into) || regions.iter().all(|r| multitrack.locate(*r).is_none()) {
                return Ok(());
            }
            match holders(multitrack).get(&into.0) {
                Some(holder) => Err(format!(
                    "a join is called {}, which is {} here",
                    into.0,
                    holder.what()
                )),
                None => Ok(()),
            }
        }
        MultitrackIntent::SetMarker { marker, .. } => match holders(multitrack).get(&marker.0) {
            Some(Holder::Marker) | None => Ok(()),
            Some(holder) => Err(format!(
                "a marker is called {}, which is {} here",
                marker.0,
                holder.what()
            )),
        },
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::intent::{Against, Rules};
    use crate::multitrack::edit::apply;
    use crate::multitrack::{Automation, Content, Marker, Region, Track};
    use crate::timebase::Second;
    use crate::{Lifetime, Opaque, SourceId, SourceRef};

    fn region(id: u64, at: f64) -> Region {
        Region::new(
            NodeId(id),
            Second(at),
            Second(4.0),
            Content::window(crate::multitrack::Window {
                source: SourceRef {
                    source: SourceId(1),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                start: 0.0,
                duration: 8.0,
            }),
        )
    }

    fn curve(id: u64) -> Automation<Second> {
        Automation::new(NodeId(id), Opaque(json!({"port": "gain"})))
    }

    /// A track 1 on take lane 2 holding region 3 with curve 4, and marker 5.
    fn multitrack() -> Multitrack {
        let mut track = Track::new(NodeId(1), NodeId(2));
        let mut held = region(3, 0.0);
        held.automation.push(curve(4));
        track.take_lanes[0].place(held);
        let mut multitrack = Multitrack::new();
        multitrack.tracks = vec![track];
        multitrack.markers.push(Marker::new(NodeId(5), Second(1.0)));
        multitrack
    }

    fn unique(multitrack: &Multitrack) -> bool {
        let all = every(multitrack);
        let once: HashSet<u64> = all.iter().map(|(id, _)| *id).collect();
        once.len() == all.len()
    }

    fn apply_one(multitrack: &mut Multitrack, intent: MultitrackIntent) -> bool {
        apply(multitrack, &intent, &Against::unstated(), &Rules::none()).applied
    }

    /// **A session file that names an id twice opens repaired**: the track
    /// keeps its id, a region and a curve under the same one get new ones,
    /// and each is still where it was.
    #[test]
    fn a_multitrack_read_with_repeated_ids_is_repaired() {
        let mut written = serde_json::to_value(multitrack()).unwrap();
        let lane = &mut written["tracks"][0]["take_lanes"][0];
        lane["regions"][0]["id"] = json!(1);
        lane["regions"][0]["automation"][0]["id"] = json!(1);
        written["markers"][0]["id"] = json!(2);
        let read: Multitrack = serde_json::from_value(written).unwrap();
        assert!(unique(&read), "{:?}", every(&read));
        assert_eq!(read.tracks[0].id, NodeId(1), "the track keeps it");
        assert_eq!(
            read.tracks[0].take_lanes[0].id,
            NodeId(2),
            "the take lane keeps it"
        );
        assert_eq!(read.tracks[0].take_lanes[0].regions.len(), 1);
        assert_eq!(read.markers.len(), 1);
    }

    /// **An id is minted past the markers too**: a marker is looked up by its
    /// id like a region is.
    #[test]
    fn a_fresh_id_is_past_the_markers() {
        let mut multitrack = multitrack();
        multitrack.markers[0].id = NodeId(40);
        assert_eq!(fresh(&multitrack), 41);
        assert_eq!(super::super::picture::fresh_id(&multitrack), 41);
    }

    /// **A take lane stated with a region under an id held elsewhere is
    /// refused**, and the multitrack is as it was.
    #[test]
    fn a_take_lane_bringing_a_held_id_is_refused() {
        let mut multitrack = multitrack();
        let before = multitrack.clone();
        let intent = MultitrackIntent::SetTakeLane {
            take_lane: NodeId(2),
            regions: vec![region(3, 0.0), region(5, 8.0)],
        };
        assert!(!apply_one(&mut multitrack, intent), "5 is the marker");
        assert_eq!(multitrack, before);
        // what the take lane already held may be stated again, and moved
        let restated = MultitrackIntent::SetTakeLane {
            take_lane: NodeId(2),
            regions: vec![region(3, 2.0), region(6, 8.0)],
        };
        assert!(apply_one(&mut multitrack, restated));
        assert!(unique(&multitrack));
    }

    /// **A track list that says an id twice is refused.**
    #[test]
    fn a_track_list_saying_an_id_twice_is_refused() {
        let mut multitrack = multitrack();
        let copy = multitrack.tracks[0].clone();
        let tracks = vec![multitrack.tracks[0].clone(), copy];
        assert!(!apply_one(
            &mut multitrack,
            MultitrackIntent::SetTracks { tracks }
        ));
        assert_eq!(multitrack.tracks.len(), 1);
    }

    /// **A split leaves two halves whose curves are their own**, and the
    /// same split applied again changes nothing.
    #[test]
    fn a_split_gives_the_second_half_s_curves_ids_of_their_own() {
        let mut multitrack = multitrack();
        let split = MultitrackIntent::SplitRegion {
            region: NodeId(3),
            at: Second(2.0),
            left: NodeId(3),
            right: NodeId(7),
            left_content: None,
            right_content: None,
        };
        assert!(apply_one(&mut multitrack, split.clone()));
        assert!(unique(&multitrack), "{:?}", every(&multitrack));
        let regions = &multitrack.tracks[0].take_lanes[0].regions;
        assert_eq!(regions.len(), 2);
        assert!(regions.iter().all(|r| r.automation.len() == 1));
        let after = multitrack.clone();
        apply_one(&mut multitrack, split);
        assert_eq!(
            multitrack.tracks, after.tracks,
            "applied twice is applied once"
        );
    }

    /// **Names an intent brings that are something else here are refused**:
    /// a split's half, a join's result, a new marker.
    #[test]
    fn a_name_that_is_something_else_is_refused() {
        let mut multitrack = multitrack();
        let half = MultitrackIntent::SplitRegion {
            region: NodeId(3),
            at: Second(2.0),
            left: NodeId(3),
            right: NodeId(4),
            left_content: None,
            right_content: None,
        };
        assert!(!apply_one(&mut multitrack, half), "4 is the region's curve");
        let marker = MultitrackIntent::SetMarker {
            marker: NodeId(1),
            at: Second(2.0),
            name: None,
        };
        assert!(!apply_one(&mut multitrack, marker), "1 is the track");
        let moved = MultitrackIntent::SetMarker {
            marker: NodeId(5),
            at: Second(2.0),
            name: None,
        };
        assert!(apply_one(&mut multitrack, moved), "its own id moves it");
        assert!(unique(&multitrack));
    }
}
