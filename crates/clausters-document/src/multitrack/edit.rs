//! What a multitrack editor **does**, written down — and the one place that
//! does it.
//!
//! [`crate::intent`] is the same discipline over the general tree, and this is
//! that discipline over [`Multitrack`]: an edit states the value it results
//! in, nothing else applies one, and what comes back is the **effective** edit
//! rather than a bare success. The rules are unchanged; only the vocabulary is
//! wider, because a DAW's gestures are wider than *place a node in an
//! aggregate*.
//!
//! # Absolute, here, means the address as well as the value
//!
//! A region belongs to a lane and a lane to a track, so *where a region is* is
//! three coordinates and not one. [`MultitrackIntent::PlaceRegion`] states all
//! of them together, which is why **moving a region to another track is one
//! edit and not a remove plus an add** — one intent, one entry in a log, one
//! undo. A vocabulary that spelled it as two would have a state between them
//! where the region is nowhere, and every reader that redrew in between would
//! see it.
//!
//! # Two verbs the crate cannot compute, and what it asks for instead
//!
//! Splitting and joining a region are the only edits here that change *how many
//! regions there are*, and they are also the only two this crate cannot work
//! out on its own. The split point is on the musical axis and a window into a
//! source is on the content's, and [`crate::timebase`] converts between the two
//! **never** — that is its whole premise, and it is not suspended because it
//! would be convenient here. So the caller, who holds the tempo map and knows
//! its own frames per beat, states the halves' content and the crate does the
//! rest.
//!
//! Both also invert as [`MultitrackIntent::SetLane`] — the lane's previous
//! contents, whole. Nothing smaller describes putting back a region that was
//! made out of two, and computing it back would be the same conversion refused
//! one paragraph ago.
//!
//! # What is *not* here
//!
//! No transport, no playback, no selection: what the piece holds is where the
//! loop is, never whether looping is on. And no verb reads a fade's shape, a
//! curve's interpolation or an automation's target — those travel opaquely for
//! the reason [`crate::points`] states, and an undo that straightened a curve
//! would be losing the client's data rather than declining to read it.

use serde::{Deserialize, Serialize};

use super::{Content, Fade, Marker, Meter, Multitrack, Region, Span, Tempo, Track};
use crate::history::{Applied, Editable};
use crate::intent::{Against, Outcome, Rules};
use crate::timebase::Beat;
use crate::{NodeId, Opaque, Point};

/// The domain name the piece's structure is registered under. See
/// [`crate::domain`].
pub const MULTITRACK: &str = "multitrack";

/// Which of the piece's two spans an edit names. See
/// [`MultitrackIntent::SetRange`].
///
/// Not `Range`, which this crate already spends on a span of frames
/// ([`crate::Range`]), and not `Span`, which is the arrangement's own
/// ([`Span`]): what this names is *which* of the two the piece holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanKind {
    /// Where the loop is.
    Loop,
    /// Where recording punches in and out.
    Punch,
}

/// An edit to the piece, in the owner's terms and stating the value it results
/// in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "lowercase")]
pub enum MultitrackIntent {
    /// What the piece's tracks are now, whole.
    ///
    /// Adding a track, removing one and reordering them are one verb, because
    /// all three state the same thing: *the tracks are now these, in this
    /// order*. Whole rather than three verbs for the reason
    /// [`crate::Intent::SetMembers`] is whole — a patch is a delta by another
    /// name — and it costs what that one costs: the inverse a log holds is a
    /// copy of the tracks.
    SetTracks {
        /// The tracks, in the order they are shown.
        tracks: Vec<Track>,
    },
    /// Which of a track's lanes plays.
    ///
    /// Comping's one verb. It names the lane rather than its index, so a
    /// choice survives the lanes being reordered underneath it.
    SetActiveLane {
        /// The track.
        track: NodeId,
        /// The lane that now plays.
        lane: NodeId,
    },
    /// What a lane holds now, whole.
    ///
    /// The lane's [`SetMembers`](crate::Intent::SetMembers): where a region is
    /// added, removed or pasted, and where a split and a join invert to. The
    /// regions keep their ids, so what survived an edit is still the same
    /// region to a log and to a view, and they are kept in position order
    /// whatever order they arrive in.
    SetLane {
        /// The lane being rewritten.
        lane: NodeId,
        /// Its regions.
        regions: Vec<Region>,
    },
    /// Where a region now sits: which track, which lane, which beat, which
    /// layer.
    ///
    /// **One edit, whatever moved.** A drag within a lane, a drag to another
    /// lane of the same track and a drag to another track are the same verb
    /// with different fields, so all three undo in one step. It never changes
    /// what the region reads — that is [`MultitrackIntent::TrimRegion`] — so a
    /// move cannot silently retime the material.
    PlaceRegion {
        /// The region being placed.
        region: NodeId,
        /// The track it now belongs to.
        track: NodeId,
        /// The lane of that track it now sits on.
        lane: NodeId,
        /// Where it now starts.
        position: Beat,
        /// Which of the overlapping regions is on top. See
        /// [`Region::layer`].
        #[serde(default)]
        layer: u32,
    },
    /// How much of a region shows, and from where.
    ///
    /// The other half of a region's geometry: a right-hand trim moves the
    /// length, a left-hand one moves the position and the length **and** the
    /// window into the source, which is why the resulting content is a field
    /// here. `None` leaves the content exactly as it is, which is what a
    /// right-hand trim and a composite region both want.
    TrimRegion {
        /// The region being trimmed.
        region: NodeId,
        /// Where it now starts.
        position: Beat,
        /// How long it now is.
        length: Beat,
        /// What it now reads, when the trim moved the window.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Content>,
    },
    /// One region becomes two, at a beat, with the two identities named.
    ///
    /// Naming the resulting ids is what keeps this absolute: applying it twice
    /// leaves the same piece, because the second time the halves are already
    /// there. The two contents are the caller's to state — see the module
    /// docs — and `None` leaves a half reading exactly what the region read.
    SplitRegion {
        /// The region being split. Gone when this applies.
        region: NodeId,
        /// Where it is cut, on the timeline.
        at: Beat,
        /// The identity of the half before the cut.
        left: NodeId,
        /// The identity of the half after it.
        right: NodeId,
        /// What the left half reads, when the caller knows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        left_content: Option<Content>,
        /// What the right half reads, when the caller knows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        right_content: Option<Content>,
    },
    /// Several regions of one lane become one, with the identity named.
    ///
    /// The join spans from the first region's position to the last one's end,
    /// keeps the first's fade in and the last's fade out, and reads what
    /// `content` says or else what the first one read.
    JoinRegions {
        /// The regions being joined, all on one lane. Gone when this applies.
        regions: Vec<NodeId>,
        /// The identity of the one that replaces them.
        into: NodeId,
        /// What it reads, when the caller knows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Content>,
    },
    /// A region's fades, both stated.
    ///
    /// A **crossfade** is this on two overlapping regions, in one transaction:
    /// the model has no third object for it, because a crossfade is what two
    /// fades over an overlap already are, and an object for it would be a
    /// second place for the same two numbers to live.
    FadeRegion {
        /// The region.
        region: NodeId,
        /// Its fade in, or `None` for none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fade_in: Option<Fade>,
        /// Its fade out, or `None` for none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fade_out: Option<Fade>,
    },
    /// What an automation curve holds now, whole.
    ///
    /// [`PointsIntent::SetPoints`](crate::PointsIntent::SetPoints) addressed to
    /// a curve that lives in the piece. The same verb rather than a second
    /// spelling of it: a curve edited in a window and a curve edited in a lane
    /// are the same edit, and the only thing this adds is which curve.
    SetAutomation {
        /// The curve.
        automation: NodeId,
        /// Its points, in order.
        points: Vec<Point>,
    },
    /// A marker is at this beat with this name — placed if it was not there,
    /// moved or renamed if it was.
    SetMarker {
        /// Its identity.
        marker: NodeId,
        /// Where it now is.
        at: Beat,
        /// What it is now called.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// A marker is gone.
    ///
    /// The one verb here that states an absence, because a marker is the one
    /// thing in the piece with no list to be stated whole against: the markers
    /// are the piece's and a `SetMarkers` would make every rename carry all of
    /// them.
    RemoveMarker {
        /// Its identity.
        marker: NodeId,
    },
    /// Where the loop or the punch span now is, or `None` for unset.
    SetRange {
        /// Which span.
        range: SpanKind,
        /// Where it now is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        span: Option<Span>,
    },
    /// What the tempo map is now, whole.
    ///
    /// The map is the piece's, so an edit to it is the piece's, and stating it
    /// whole is what makes adding, moving and removing an entry one verb. It is
    /// small — a piece has tempo changes, not tempo per beat — which is why
    /// this one is whole where a lane's regions get a verb of their own.
    SetTempoMap {
        /// The entries; kept in position order.
        tempo: Vec<Tempo>,
    },
    /// What the meter map is now, whole. See
    /// [`MultitrackIntent::SetTempoMap`].
    SetMeterMap {
        /// The entries; kept in position order.
        meter: Vec<Meter>,
    },
}

impl MultitrackIntent {
    /// The kind of edit this is, as the coalesce key spells it.
    fn kind(&self) -> &'static str {
        match self {
            Self::SetTracks { .. } => "settracks",
            Self::SetActiveLane { .. } => "setactivelane",
            Self::SetLane { .. } => "setlane",
            Self::PlaceRegion { .. } => "placeregion",
            Self::TrimRegion { .. } => "trimregion",
            Self::SplitRegion { .. } => "splitregion",
            Self::JoinRegions { .. } => "joinregions",
            Self::FadeRegion { .. } => "faderegion",
            Self::SetAutomation { .. } => "setautomation",
            Self::SetMarker { .. } => "setmarker",
            Self::RemoveMarker { .. } => "removemarker",
            Self::SetRange { .. } => "setrange",
            Self::SetTempoMap { .. } => "settempomap",
            Self::SetMeterMap { .. } => "setmetermap",
        }
    }

    /// What this edit addresses, when it addresses one thing by name.
    ///
    /// `None` for the edits that name the piece itself — the tracks, the two
    /// maps, a range — which is also what keeps them from coalescing with each
    /// other by accident.
    pub fn subject(&self) -> Option<NodeId> {
        match self {
            Self::SetActiveLane { track, .. } => Some(*track),
            Self::SetLane { lane, .. } => Some(*lane),
            Self::PlaceRegion { region, .. }
            | Self::TrimRegion { region, .. }
            | Self::SplitRegion { region, .. }
            | Self::FadeRegion { region, .. } => Some(*region),
            Self::JoinRegions { into, .. } => Some(*into),
            Self::SetAutomation { automation, .. } => Some(*automation),
            Self::SetMarker { marker, .. } | Self::RemoveMarker { marker } => Some(*marker),
            Self::SetTracks { .. } | Self::SetRange { .. } => None,
            Self::SetTempoMap { .. } | Self::SetMeterMap { .. } => None,
        }
    }
}

/// Apply an edit to a piece.
///
/// The only door, and the same shape as [`crate::intent::apply`]: the version
/// moves when the piece changed and stays where it is when it did not, a
/// refusal hands back what the piece says now, and an edit made against a
/// version the piece has left behind is refused as stale rather than applied
/// blind.
///
/// The **generation** half of [`Against`] is not read here, and that is not an
/// omission: no edit in this vocabulary writes samples, so a source being
/// rewritten underneath a region does not make a move of that region wrong.
pub fn apply(
    piece: &mut Multitrack,
    intent: &MultitrackIntent,
    against: &Against,
    rules: &Rules,
) -> Outcome<MultitrackIntent> {
    if against.is_stated() && against.version != piece.version {
        let reason = if against.version < piece.version {
            "the piece changed since this edit was made"
        } else {
            "this edit was made against a different piece"
        };
        if let Some(current) = current(piece, intent) {
            return Outcome::superseded(current, reason);
        }
    }
    let before = piece.version;
    let outcome = edit(piece, intent, rules);
    if outcome.applied && piece.version == before {
        piece.version += 1;
    }
    outcome
}

fn edit(
    piece: &mut Multitrack,
    intent: &MultitrackIntent,
    rules: &Rules,
) -> Outcome<MultitrackIntent> {
    match intent {
        MultitrackIntent::SetTracks { tracks } => set_tracks(piece, tracks),
        MultitrackIntent::SetActiveLane { track, lane } => set_active_lane(piece, *track, *lane),
        MultitrackIntent::SetLane { lane, regions } => set_lane(piece, *lane, regions),
        MultitrackIntent::PlaceRegion {
            region,
            track,
            lane,
            position,
            layer,
        } => place_region(piece, *region, *track, *lane, *position, *layer, rules),
        MultitrackIntent::TrimRegion {
            region,
            position,
            length,
            content,
        } => trim_region(piece, *region, *position, *length, content.as_ref(), rules),
        MultitrackIntent::SplitRegion {
            region,
            at,
            left,
            right,
            left_content,
            right_content,
        } => split_region(
            piece,
            *region,
            *at,
            *left,
            *right,
            left_content.as_ref(),
            right_content.as_ref(),
            rules,
        ),
        MultitrackIntent::JoinRegions {
            regions,
            into,
            content,
        } => join_regions(piece, regions, *into, content.as_ref()),
        MultitrackIntent::FadeRegion {
            region,
            fade_in,
            fade_out,
        } => fade_region(piece, *region, fade_in.as_ref(), fade_out.as_ref()),
        MultitrackIntent::SetAutomation { automation, points } => {
            set_automation(piece, *automation, points)
        }
        MultitrackIntent::SetMarker { marker, at, name } => {
            set_marker(piece, *marker, *at, name.as_deref(), rules)
        }
        MultitrackIntent::RemoveMarker { marker } => remove_marker(piece, *marker),
        MultitrackIntent::SetRange { range, span } => set_range(piece, *range, *span),
        MultitrackIntent::SetTempoMap { tempo } => set_tempo_map(piece, tempo),
        MultitrackIntent::SetMeterMap { meter } => set_meter_map(piece, meter),
    }
}

/// The edit describing what the piece says **now** about what `intent`
/// addresses — what a refusal of any kind hands back, and what a log records as
/// the inverse.
///
/// `None` when the piece cannot describe it: the region is gone, the lane is
/// not there. Those have their own refusals, with better reasons than
/// staleness.
pub fn current(piece: &Multitrack, intent: &MultitrackIntent) -> Option<MultitrackIntent> {
    match intent {
        MultitrackIntent::SetTracks { .. } => Some(MultitrackIntent::SetTracks {
            tracks: piece.tracks.clone(),
        }),
        MultitrackIntent::SetActiveLane { track, .. } => {
            let held = piece.track(*track)?;
            Some(MultitrackIntent::SetActiveLane {
                track: *track,
                lane: held.active_lane()?.id,
            })
        }
        MultitrackIntent::SetLane { lane, .. } => Some(lane_state(piece, *lane)?),
        MultitrackIntent::PlaceRegion { region, .. } => {
            let (track, lane, held) = piece.locate(*region)?;
            Some(MultitrackIntent::PlaceRegion {
                region: *region,
                track: track.id,
                lane: lane.id,
                position: held.position,
                layer: held.layer,
            })
        }
        MultitrackIntent::TrimRegion { region, .. } => {
            let (_, _, held) = piece.locate(*region)?;
            Some(MultitrackIntent::TrimRegion {
                region: *region,
                position: held.position,
                length: held.length,
                content: Some(held.content.clone()),
            })
        }
        // The two that change how many regions there are invert as the lane's
        // previous contents. See the module docs: nothing smaller describes it,
        // and reconstructing it would need the conversion this crate refuses.
        MultitrackIntent::SplitRegion { region, .. } => {
            let (_, lane, _) = piece.locate(*region)?;
            lane_state(piece, lane.id)
        }
        MultitrackIntent::JoinRegions { regions, .. } => {
            let (_, lane, _) = piece.locate(*regions.first()?)?;
            lane_state(piece, lane.id)
        }
        MultitrackIntent::FadeRegion { region, .. } => {
            let (_, _, held) = piece.locate(*region)?;
            Some(MultitrackIntent::FadeRegion {
                region: *region,
                fade_in: held.fade_in.clone(),
                fade_out: held.fade_out.clone(),
            })
        }
        MultitrackIntent::SetAutomation { automation, .. } => {
            let curve = piece
                .tracks
                .iter()
                .flat_map(|t| t.automation.iter())
                .find(|a| a.id == *automation)?;
            Some(MultitrackIntent::SetAutomation {
                automation: *automation,
                points: curve.points.clone(),
            })
        }
        // A marker that is not there is described by its absence, which is a
        // sentence this vocabulary has. Both verbs invert into the other.
        MultitrackIntent::SetMarker { marker, .. } | MultitrackIntent::RemoveMarker { marker } => {
            Some(match piece.markers.iter().find(|m| m.id == *marker) {
                Some(held) => MultitrackIntent::SetMarker {
                    marker: *marker,
                    at: held.at,
                    name: held.name.clone(),
                },
                None => MultitrackIntent::RemoveMarker { marker: *marker },
            })
        }
        MultitrackIntent::SetRange { range, .. } => Some(MultitrackIntent::SetRange {
            range: *range,
            span: match range {
                SpanKind::Loop => piece.loop_span,
                SpanKind::Punch => piece.punch,
            },
        }),
        MultitrackIntent::SetTempoMap { .. } => Some(MultitrackIntent::SetTempoMap {
            tempo: piece.tempo.clone(),
        }),
        MultitrackIntent::SetMeterMap { .. } => Some(MultitrackIntent::SetMeterMap {
            meter: piece.meter.clone(),
        }),
    }
}

fn lane_state(piece: &Multitrack, lane: NodeId) -> Option<MultitrackIntent> {
    let (_, held) = piece.lane(lane)?;
    Some(MultitrackIntent::SetLane {
        lane,
        regions: held.regions.clone(),
    })
}

// ---- the verbs ----

fn set_tracks(piece: &mut Multitrack, tracks: &[Track]) -> Outcome<MultitrackIntent> {
    let stated = MultitrackIntent::SetTracks {
        tracks: tracks.to_vec(),
    };
    if piece.tracks == tracks {
        return Outcome::unchanged(stated);
    }
    piece.tracks = tracks.to_vec();
    Outcome::changed(stated)
}

fn set_active_lane(
    piece: &mut Multitrack,
    track: NodeId,
    lane: NodeId,
) -> Outcome<MultitrackIntent> {
    let stated = MultitrackIntent::SetActiveLane { track, lane };
    let Some(held) = piece.track_mut(track) else {
        return Outcome::refused(stated, "no such track");
    };
    let Some(index) = held.lanes.iter().position(|l| l.id == lane) else {
        return Outcome::refused(stated, "that lane is not on that track");
    };
    if held.active == index {
        return Outcome::unchanged(stated);
    }
    held.active = index;
    Outcome::changed(stated)
}

fn set_lane(piece: &mut Multitrack, lane: NodeId, regions: &[Region]) -> Outcome<MultitrackIntent> {
    let mut ordered = regions.to_vec();
    ordered.sort_by(|a, b| {
        order(a)
            .partial_cmp(&order(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let stated = MultitrackIntent::SetLane {
        lane,
        regions: ordered.clone(),
    };
    let Some(held) = piece.lane_mut(lane) else {
        return Outcome::refused(
            MultitrackIntent::SetLane {
                lane,
                regions: Vec::new(),
            },
            "no such lane",
        );
    };
    if held.regions == ordered {
        return Outcome::unchanged(stated);
    }
    held.regions = ordered;
    if regions.windows(2).any(|w| order(&w[0]) > order(&w[1])) {
        return Outcome::transformed(stated, "put in position order");
    }
    Outcome::changed(stated)
}

fn order(region: &Region) -> (f64, u32) {
    (region.position.get(), region.layer)
}

#[allow(clippy::too_many_arguments)]
fn place_region(
    piece: &mut Multitrack,
    region: NodeId,
    track: NodeId,
    lane: NodeId,
    position: Beat,
    layer: u32,
    rules: &Rules,
) -> Outcome<MultitrackIntent> {
    let position = snap(rules, position);
    let stated = MultitrackIntent::PlaceRegion {
        region,
        track,
        lane,
        position,
        layer,
    };
    let Some((held_track, held_lane, held)) = piece.locate(region) else {
        return Outcome::refused(stated, "no such region");
    };
    if (held_track.id, held_lane.id, held.position, held.layer) == (track, lane, position, layer) {
        return Outcome::unchanged(stated);
    }
    let Some(target) = piece.tracks.iter().find(|t| t.id == track) else {
        return Outcome::refused(stated, "no such track");
    };
    if !target.lanes.iter().any(|l| l.id == lane) {
        return Outcome::refused(stated, "that lane is not on that track");
    }
    let mut moved = take_region(piece, region).expect("located a moment ago");
    moved.position = position;
    moved.layer = layer;
    piece
        .lane_mut(lane)
        .expect("checked a moment ago")
        .place(moved);
    if snapped(rules) {
        Outcome::transformed(stated, "snapped to the grid")
    } else {
        Outcome::changed(stated)
    }
}

fn trim_region(
    piece: &mut Multitrack,
    region: NodeId,
    position: Beat,
    length: Beat,
    content: Option<&Content>,
    rules: &Rules,
) -> Outcome<MultitrackIntent> {
    let position = snap(rules, position);
    let length = snap(rules, length);
    let stated = |content: Option<Content>| MultitrackIntent::TrimRegion {
        region,
        position,
        length,
        content,
    };
    if length <= Beat::ZERO {
        return Outcome::refused(
            stated(content.cloned()),
            "a region cannot be shorter than nothing",
        );
    }
    let Some(lane) = piece
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .find(|l| l.region(region).is_some())
        .map(|l| l.id)
    else {
        return Outcome::refused(stated(content.cloned()), "no such region");
    };
    let held = piece
        .lane_mut(lane)
        .and_then(|l| l.regions.iter_mut().find(|r| r.id == region))
        .expect("found a moment ago");
    let moved_content = content.is_some_and(|c| *c != held.content);
    if held.position == position && held.length == length && !moved_content {
        return Outcome::unchanged(stated(Some(held.content.clone())));
    }
    held.position = position;
    held.length = length;
    if let Some(content) = content {
        held.content = content.clone();
    }
    let effective = stated(Some(held.content.clone()));
    // The order the lane keeps is by position, and a left-hand trim moves one.
    reorder(piece, lane);
    if snapped(rules) {
        Outcome::transformed(effective, "snapped to the grid")
    } else {
        Outcome::changed(effective)
    }
}

#[allow(clippy::too_many_arguments)]
fn split_region(
    piece: &mut Multitrack,
    region: NodeId,
    at: Beat,
    left: NodeId,
    right: NodeId,
    left_content: Option<&Content>,
    right_content: Option<&Content>,
    rules: &Rules,
) -> Outcome<MultitrackIntent> {
    let at = snap(rules, at);
    let stated = MultitrackIntent::SplitRegion {
        region,
        at,
        left,
        right,
        left_content: left_content.cloned(),
        right_content: right_content.cloned(),
    };
    let Some((_, lane, held)) = piece.locate(region) else {
        // Already split: the halves are there and the region is not, which is
        // what applying this twice looks like. Absolute means this is not an
        // error.
        let done = piece.locate(left).is_some() && piece.locate(right).is_some();
        return if done {
            Outcome::unchanged(stated)
        } else {
            Outcome::refused(stated, "no such region")
        };
    };
    if at <= held.position || at >= held.end() {
        return Outcome::refused(stated, "the cut is not inside the region");
    }
    let lane = lane.id;
    let source = held.clone();
    let mut first = source.clone();
    first.id = left;
    first.length = at - source.position;
    first.fade_out = None;
    if let Some(content) = left_content {
        first.content = content.clone();
    }
    let mut second = source.clone();
    second.id = right;
    second.position = at;
    second.length = source.end() - at;
    second.fade_in = None;
    if let Some(content) = right_content {
        second.content = content.clone();
    }
    take_region(piece, region);
    let held = piece.lane_mut(lane).expect("located a moment ago");
    held.place(first);
    held.place(second);
    Outcome::changed(stated)
}

fn join_regions(
    piece: &mut Multitrack,
    regions: &[NodeId],
    into: NodeId,
    content: Option<&Content>,
) -> Outcome<MultitrackIntent> {
    let stated = MultitrackIntent::JoinRegions {
        regions: regions.to_vec(),
        into,
        content: content.cloned(),
    };
    if regions.len() < 2 {
        return Outcome::refused(stated, "a join needs two regions or more");
    }
    if regions.iter().all(|id| piece.locate(*id).is_none()) && piece.locate(into).is_some() {
        return Outcome::unchanged(stated);
    }
    let mut lane = None;
    let mut held = Vec::new();
    for id in regions {
        let Some((_, on, region)) = piece.locate(*id) else {
            return Outcome::refused(stated, "no such region");
        };
        if *lane.get_or_insert(on.id) != on.id {
            return Outcome::refused(stated, "those regions are not on one lane");
        }
        held.push(region.clone());
    }
    let lane = lane.expect("two regions at least");
    held.sort_by(|a, b| {
        a.position
            .partial_cmp(&b.position)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut joined = held[0].clone();
    joined.id = into;
    joined.length = held.iter().map(Region::end).fold(Beat::ZERO, Beat::max) - joined.position;
    joined.fade_out = held.last().expect("two at least").fade_out.clone();
    if let Some(content) = content {
        joined.content = content.clone();
    }
    for id in regions {
        take_region(piece, *id);
    }
    piece.lane_mut(lane).expect("located above").place(joined);
    Outcome::changed(stated)
}

fn fade_region(
    piece: &mut Multitrack,
    region: NodeId,
    fade_in: Option<&Fade>,
    fade_out: Option<&Fade>,
) -> Outcome<MultitrackIntent> {
    let stated = MultitrackIntent::FadeRegion {
        region,
        fade_in: fade_in.cloned(),
        fade_out: fade_out.cloned(),
    };
    let Some(held) = piece
        .tracks
        .iter_mut()
        .flat_map(|t| t.lanes.iter_mut())
        .flat_map(|l| l.regions.iter_mut())
        .find(|r| r.id == region)
    else {
        return Outcome::refused(stated, "no such region");
    };
    if held.fade_in.as_ref() == fade_in && held.fade_out.as_ref() == fade_out {
        return Outcome::unchanged(stated);
    }
    held.fade_in = fade_in.cloned();
    held.fade_out = fade_out.cloned();
    Outcome::changed(stated)
}

fn set_automation(
    piece: &mut Multitrack,
    automation: NodeId,
    points: &[Point],
) -> Outcome<MultitrackIntent> {
    let stated = MultitrackIntent::SetAutomation {
        automation,
        points: points.to_vec(),
    };
    let Some(curve) = piece.automation_mut(automation) else {
        return Outcome::refused(stated, "no such automation");
    };
    if curve.points == points {
        return Outcome::unchanged(stated);
    }
    curve.points = points.to_vec();
    Outcome::changed(stated)
}

fn set_marker(
    piece: &mut Multitrack,
    marker: NodeId,
    at: Beat,
    name: Option<&str>,
    rules: &Rules,
) -> Outcome<MultitrackIntent> {
    let at = snap(rules, at);
    let stated = MultitrackIntent::SetMarker {
        marker,
        at,
        name: name.map(str::to_string),
    };
    if let Some(held) = piece.markers.iter().find(|m| m.id == marker)
        && held.at == at
        && held.name.as_deref() == name
    {
        return Outcome::unchanged(stated);
    }
    piece.markers.retain(|m| m.id != marker);
    let mut placed = Marker::new(marker, at);
    placed.name = name.map(str::to_string);
    piece.add_marker(placed);
    if snapped(rules) {
        Outcome::transformed(stated, "snapped to the grid")
    } else {
        Outcome::changed(stated)
    }
}

fn remove_marker(piece: &mut Multitrack, marker: NodeId) -> Outcome<MultitrackIntent> {
    let stated = MultitrackIntent::RemoveMarker { marker };
    let before = piece.markers.len();
    piece.markers.retain(|m| m.id != marker);
    if piece.markers.len() == before {
        return Outcome::unchanged(stated);
    }
    Outcome::changed(stated)
}

fn set_range(
    piece: &mut Multitrack,
    range: SpanKind,
    span: Option<Span>,
) -> Outcome<MultitrackIntent> {
    let stated = MultitrackIntent::SetRange { range, span };
    let slot = match range {
        SpanKind::Loop => &mut piece.loop_span,
        SpanKind::Punch => &mut piece.punch,
    };
    if *slot == span {
        return Outcome::unchanged(stated);
    }
    *slot = span;
    Outcome::changed(stated)
}

fn set_tempo_map(piece: &mut Multitrack, tempo: &[Tempo]) -> Outcome<MultitrackIntent> {
    let mut ordered = tempo.to_vec();
    ordered.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let stated = MultitrackIntent::SetTempoMap {
        tempo: ordered.clone(),
    };
    if piece.tempo == ordered {
        return Outcome::unchanged(stated);
    }
    piece.tempo = ordered;
    Outcome::changed(stated)
}

fn set_meter_map(piece: &mut Multitrack, meter: &[Meter]) -> Outcome<MultitrackIntent> {
    let mut ordered = meter.to_vec();
    ordered.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    let stated = MultitrackIntent::SetMeterMap {
        meter: ordered.clone(),
    };
    if piece.meter == ordered {
        return Outcome::unchanged(stated);
    }
    piece.meter = ordered;
    Outcome::changed(stated)
}

// ---- the pieces the verbs share ----

/// Lifts a region out of whatever lane holds it.
fn take_region(piece: &mut Multitrack, region: NodeId) -> Option<Region> {
    for track in piece.tracks.iter_mut() {
        for lane in track.lanes.iter_mut() {
            if let Some(index) = lane.regions.iter().position(|r| r.id == region) {
                return Some(lane.regions.remove(index));
            }
        }
    }
    None
}

/// Puts a lane back in position order after an edit moved one of its regions.
fn reorder(piece: &mut Multitrack, lane: NodeId) {
    if let Some(held) = piece.lane_mut(lane) {
        held.regions.sort_by(|a, b| {
            order(a)
                .partial_cmp(&order(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
}

fn snap(rules: &Rules, beat: Beat) -> Beat {
    Beat(rules.snap(beat.get()))
}

fn snapped(rules: &Rules) -> bool {
    rules.quant > 0.0
}

// ---- the vocabulary as a history carries it ----

/// A piece's edit as a history carries it.
pub fn payload(intent: &MultitrackIntent) -> Opaque {
    Opaque(serde_json::to_value(intent).unwrap_or(serde_json::Value::Null))
}

/// The edit a payload holds, or `None` when it is written in another
/// vocabulary.
pub fn intent_of(payload: &Opaque) -> Option<MultitrackIntent> {
    serde_json::from_value(payload.0.clone()).ok()
}

/// What makes two of the piece's edits *the same thing done the same way*: the
/// kind of edit and what it addresses.
///
/// A hundred small drags of one region are one undo; a drag of the next one is
/// not. The edits that name the piece itself key on the kind alone, which is
/// right for them — a run of tempo adjustments is also one thing the person
/// did.
pub fn coalesce_key(intent: &MultitrackIntent) -> String {
    match intent.subject() {
        Some(id) => format!("{}:{}", intent.kind(), id.0),
        None => intent.kind().to_string(),
    }
}

/// The piece as an [`Editable`]: an multitrack, plus the two things an edit to
/// this domain needs and a curve does not.
///
/// The twin of [`crate::log::Tree`], and built for one call for the same
/// reason: `against` is the state the editor believed it was editing and
/// `rules` the grid this gesture snaps to, and neither is a property of the
/// piece.
pub struct Piece<'a> {
    /// The piece being edited.
    pub multitrack: &'a mut Multitrack,
    /// The state the edit was made against. See [`Against`].
    pub against: Against,
    /// How the owner transforms the edit as it applies it. See [`Rules`].
    pub rules: Rules,
}

impl<'a> Piece<'a> {
    /// The piece, edited against whatever it currently says and snapping to
    /// nothing — what a script that just read it wants.
    pub fn new(multitrack: &'a mut Multitrack) -> Self {
        Self {
            multitrack,
            against: Against::unstated(),
            rules: Rules::none(),
        }
    }
}

impl Editable for Piece<'_> {
    fn apply(&mut self, load: &Opaque) -> Applied {
        let Some(intent) = intent_of(load) else {
            return Applied::refused(
                load.clone(),
                "not an edit written in the piece's vocabulary",
            );
        };
        let outcome = apply(self.multitrack, &intent, &self.against, &self.rules);
        Applied {
            effective: payload(&outcome.effective),
            applied: outcome.applied,
            reason: outcome.reason,
            stale: outcome.stale,
        }
    }

    fn current(&self, load: &Opaque) -> Option<Opaque> {
        current(self.multitrack, &intent_of(load)?).map(|intent| payload(&intent))
    }

    fn coalesce_key(&self, load: &Opaque) -> Option<String> {
        Some(coalesce_key(&intent_of(load)?))
    }
}

#[cfg(test)]
mod tests;
