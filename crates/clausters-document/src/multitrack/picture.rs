//! **The piece as a multitrack view holds it**: rows and boxes, and a hand's
//! answer read back.
//!
//! The mapping between the model and the picture, in one place because there is
//! one of it. A multitrack view — the standalone host's, the Python client's,
//! the web client's — draws a **row** per track and a **box** per region, and
//! reports the whole list after any gesture. Which verb that list stands for is
//! not obvious (a move and a trim are different edits; a box the piece has no
//! region for is a new one), and a reader written per client is a reader that
//! disagrees per client.
//!
//! # It is in seconds, and never in frames
//!
//! A timeline axis counts sample frames and this crate has no sample rate. So
//! everything here is in the unit the multitrack is written in, seconds, and a
//! caller crosses to its axis with the rate alone. No tempo map is involved: a
//! multitrack places things in physical time, and the tempo map it holds is a
//! structure a ruler and a snap read, which places nothing.
//!
//! # A row is a track, showing the lane it plays
//!
//! A track holds several lanes because that is what comping is made of, and
//! which one plays is the track's own choice ([`Track::active`]). So a row is a
//! track showing its active lane; the others are the takes behind it, and
//! showing them is an expansion a view keeps state for.
//!
//! # A name is an id
//!
//! A row is named by its track's id and a box by its region's, so a payload is
//! read with no map on the side. The one exception is the reason this module
//! has a `read` at all: **a box may come back under a name that is no id**,
//! because a split names its halves after the box they came from — and that is
//! exactly how a new box is told from a moved one.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::multitrack::edit::{MintedSource, MultitrackIntent};
use crate::multitrack::{Automation, Content, Lane, Multitrack, Region, Track};
use crate::session::{Location, Part, Source};
use crate::{
    Lifetime, NodeId, Opaque, Point, Range, Second, SegmentRef, SegmentSource, SourceId, SourceRef,
};

/// One row of the view: a track, and the strip that is drawn beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// The track it draws — its identity, and its name on the wire.
    pub track: NodeId,
    /// The lane of that track whose regions it shows: what a box joins when it
    /// lands here.
    pub lane: NodeId,
    /// What the header draws.
    pub label: String,
    /// Silenced.
    pub mute: bool,
    /// Soloed.
    pub solo: bool,
    /// The fader, read out of the track's own table — the document holds no
    /// mixer, so a level is a key a client wrote and this only carries it.
    pub gain: f64,
    /// **Whether this track's automation rows are shown**: true when any of its
    /// curves is visible, which is what one toggle over the set means.
    pub curves: bool,
}

/// One box: a region, where it sits and what it is a window onto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Box {
    /// The region it draws — its identity, and its name on the wire.
    pub region: NodeId,
    /// The row it sits on ([`Row::track`]).
    pub row: NodeId,
    /// Where it starts, in seconds.
    pub position: Second,
    /// How long it occupies, in seconds.
    pub length: Second,
    /// The frame of the source its own zero reads, **in seconds** — a
    /// recording's units, which no tempo scales.
    pub start: f64,
    /// How long the window lasts, in seconds. A region may be placed for longer
    /// than this; past it there is nothing to read.
    pub content: f64,
    /// What it is a window onto, when it is a window onto samples at all. A
    /// window onto notes and a composite both draw as a named box.
    pub source: Option<SourceId>,
    /// What the box draws as its name.
    pub label: String,
    /// Silenced by hand, and the region's own rather than its track's.
    pub muted: bool,
    /// Whether the window **wraps**: past the end of the source it begins
    /// again. What a box longer than what it reads means, and the only one of
    /// the three answers to that question which changes what *sounds* — so it
    /// is the piece's and travels with the box.
    pub looping: bool,
}

/// The key a client's fader is kept under in a track's opaque table.
///
/// The key a track's fader was carried under before it was a field.
///
/// Kept because a piece written by an older build has it in [`Track::config`],
/// and [`level_of`] still reads it when the field is at unity -- what a file
/// said is what a file meant. Nothing writes it any more: the fader is
/// [`Track::level`].
pub const LEVEL: &str = "level";

/// The rows a piece draws as, top to bottom.
pub fn rows(piece: &Multitrack) -> Vec<Row> {
    piece
        .tracks
        .iter()
        .filter_map(|track| {
            let lane = active_lane(track)?;
            Some(Row {
                track: track.id,
                lane: lane.id,
                label: track
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("track {}", track.id.0)),
                mute: track.muted,
                solo: track.soloed,
                gain: level_of(track),
                // **Shown when any of them is.** A track's automations are one
                // toggle in a header, and the piece records visibility per
                // curve -- so the row says what the header would draw, and the
                // header says what the whole set becomes.
                curves: track.automation.iter().any(|a| a.visible),
            })
        })
        .collect()
}

/// The boxes a piece draws as, in the order the rows hold them.
pub fn boxes(piece: &Multitrack) -> Vec<Box> {
    let mut out = Vec::new();
    for track in &piece.tracks {
        let Some(lane) = active_lane(track) else {
            continue;
        };
        for region in &lane.regions {
            let (source, start, content, looping) = window_of(region);
            out.push(Box {
                region: region.id,
                row: track.id,
                position: region.position,
                length: region.length,
                start,
                content,
                source,
                label: region
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("region {}", region.id.0)),
                muted: region.muted,
                looping,
            });
        }
    }
    out
}

/// One **curve** of the view: an automation, and where it hangs.
///
/// The same shape for both places a curve lives, because it is the same curve:
/// a track's automation is drawn as a **row of its own** under that track and
/// runs the whole timeline, a region's is drawn as a **layer inside that box**
/// and runs as long as the box does. `owner` says which — a track's id for a
/// row, a region's for a layer — and the two are handed out by two calls
/// ([`curves`] and [`layers`]) rather than one with a flag, since a caller
/// draws them in two different places and never mixes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    /// The automation it draws — its identity, and its name on the wire.
    pub automation: NodeId,
    /// What it hangs from: a track (a row) or a region (a layer).
    pub owner: NodeId,
    /// What is drawn on it.
    pub label: String,
    /// **What it automates**, in the client's own terms and never read here.
    ///
    /// It travels because the value *domain* is decided from it and the domain
    /// is the client's: a gain runs over one range and a pan over another, and
    /// which is which is a fact about the parameter, not about the curve.
    #[serde(default, skip_serializing_if = "Opaque::is_empty")]
    pub target: Opaque,
    /// The break-points. `at` is in seconds, like every placement here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Point>,
    /// Whether the row or layer is shown — the view's own state, kept in the
    /// piece because which curves a person had open is part of reopening it as
    /// they left it.
    pub visible: bool,
    /// Whether the curve is being applied.
    pub enabled: bool,
}

/// The **track automations**: one row of its own under each track that has one.
pub fn curves(piece: &Multitrack) -> Vec<Curve> {
    piece
        .tracks
        .iter()
        .flat_map(|track| track.automation.iter().map(|a| curve(a, track.id)))
        .collect()
}

/// The **region automations**: one layer inside each box that has one.
///
/// Only the boxes that are drawn — the active lane's — because a layer with no
/// box under it has nowhere to be.
pub fn layers(piece: &Multitrack) -> Vec<Curve> {
    piece
        .tracks
        .iter()
        .filter_map(active_lane)
        .flat_map(|lane| &lane.regions)
        .flat_map(|region| region.automation.iter().map(|a| curve(a, region.id)))
        .collect()
}

fn curve(automation: &Automation, owner: NodeId) -> Curve {
    Curve {
        automation: automation.id,
        owner,
        label: automation
            .name
            .clone()
            .unwrap_or_else(|| format!("automation {}", automation.id.0)),
        target: automation.target.clone(),
        points: automation.points.clone(),
        visible: automation.visible,
        enabled: automation.enabled,
    }
}

/// A curve as a hand left it — what [`read_points`] is given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curved {
    /// The name it came back under, which is its automation's id.
    pub name: String,
    /// Its break-points, in order.
    #[serde(default)]
    pub points: Vec<Point>,
}

/// **What a `"points"` report means**, as edits in the piece's own vocabulary.
///
/// The report is every curve there is, rows and layers alike, for the same
/// reason a box report is every box: applying what came back is the identity.
/// So what comes out is the difference — a
/// [`MultitrackIntent::SetAutomation`] per curve whose points actually moved,
/// and nothing at all for a hand that looked without editing.
///
/// A name that is no automation's id is dropped rather than minted: a curve is
/// declared by whoever holds the piece, and a hand that dragged a break-point
/// made no new one.
pub fn read_points(piece: &Multitrack, reported: &[Curved]) -> Vec<MultitrackIntent> {
    reported
        .iter()
        .filter_map(|curve| {
            let id = curve.name.parse::<u64>().ok().map(NodeId)?;
            let held = piece.automation(id)?;
            (!same_points(&held.points, &curve.points)).then(|| MultitrackIntent::SetAutomation {
                automation: id,
                points: curve.points.clone(),
            })
        })
        .collect()
}

/// Whether two runs of break-points say the same thing.
///
/// **A JSON number compares by value and not by spelling**, which derived
/// equality on [`Opaque`] cannot do. A point's `data` is opaque and travels
/// through whichever serializer the endpoint has, and JavaScript writes `0.0`
/// as `0` — so a curve reported back exactly as it was drawn came out as an
/// *edit* in the page and as nothing in a script, which is one report meaning
/// two things. Everything else compares as it always did.
fn same_points(held: &[Point], reported: &[Point]) -> bool {
    held.len() == reported.len()
        && held
            .iter()
            .zip(reported)
            .all(|(a, b)| a.at == b.at && a.value == b.value && same_json(&a.data.0, &b.data.0))
}

/// Two opaque values, compared with numbers read as numbers.
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, x)| y.get(key).is_some_and(|y| same_json(x, y)))
        }
        _ => a == b,
    }
}

/// A row as a hand left it — what [`read_rows`] is given, and the same shape
/// [`rows`] hands out with the two fields a picture answers for itself dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Strip {
    /// The name it came back under. **An id where the row is one the view was
    /// given, and anything at all where a hand made one**: a track added by a
    /// gesture is named by whoever added it, which is how a new row is told
    /// from a moved one — the same rule a box's name follows.
    pub name: String,
    /// Silenced.
    #[serde(default)]
    pub mute: bool,
    /// Soloed.
    #[serde(default)]
    pub solo: bool,
    /// The fader, in the client's own key of the track's table.
    #[serde(default = "unity")]
    pub gain: f64,
    /// **Whether this track's automation rows are shown.**
    ///
    /// One statement for however many curves the track has, because it is one
    /// toggle: what a header offers is *show me this track's automation*, and
    /// spelling it per curve would be a report of a control nobody drew.
    #[serde(default = "yes")]
    pub curves: bool,
}

fn yes() -> bool {
    true
}

fn unity() -> f64 {
    1.0
}

/// **What a `"rows"` report means**, as edits in the piece's own vocabulary.
///
/// The report is the *piece* — every row, in the order they are shown — for the
/// same reason a box report is every box: applying what came back is the
/// identity, and there is no gesture to ask about. What comes out is the
/// difference, and it is **one** [`MultitrackIntent::SetTracks`] whatever
/// changed, because the tracks are one list and a hand that adds, removes,
/// reorders or mutes did one thing to it:
///
/// - a name that is a track's id is **that track**, with the strip's mute, solo
///   and level written onto it;
/// - a name that is no track's id is a **new track**, minted here with one
///   empty lane, since a track that could hold nothing is not one;
/// - a track the report does not name is **gone**, and its lanes and regions
///   with it — which is what makes deleting a track one edit rather than a
///   removal per box on it;
/// - the order is the report's, so the rows are the tracks and moving one moves
///   the other.
///
/// The minting walks up from [`fresh_id`], two at a time: a track and its lane.
/// Unlike [`read`] this asks the piece for that itself — a row report carries
/// no ids a caller had to reserve, so there is nothing for one to say.
///
/// **The label is not read.** A row's label is the track's name where it has
/// one and a made-up `track N` where it has not ([`rows`]), so believing a
/// report would write that made-up string into the document the first time
/// anything else on the row moved. Renaming a track is its own verb, and the
/// wire has no gesture for it yet.
pub fn read_rows(piece: &Multitrack, reported: &[Strip]) -> Vec<MultitrackIntent> {
    let mut next = fresh_id(piece);
    // **How far the piece reaches**, for a curve that has to span it. A piece
    // with nothing on it has no extent, and a flat line over nothing would be a
    // row with one point in the corner.
    let span = piece
        .tracks
        .iter()
        .flat_map(|track| track.lanes.iter())
        .flat_map(|lane| lane.regions.iter())
        .map(|region| region.end().0)
        .fold(0.0f64, f64::max);
    let mut tracks: Vec<Track> = Vec::with_capacity(reported.len());
    for strip in reported {
        let held = strip
            .name
            .parse::<u64>()
            .ok()
            .map(NodeId)
            .and_then(|id| piece.tracks.iter().find(|t| t.id == id));
        let mut track = match held {
            Some(track) => track.clone(),
            None => {
                let made = Track::new(NodeId(next), NodeId(next + 1));
                next += 2;
                made
            }
        };
        track.muted = strip.mute;
        track.soloed = strip.solo;
        // **Only when the toggle actually moved.** A row reports its automation
        // as shown when *any* of its curves is, so writing every curve on every
        // report would flatten a track that shows one and hides another the
        // first time somebody moved its fader -- and would make a piece that
        // changed nothing look edited.
        if track.automation.iter().any(|a| a.visible) != strip.curves {
            // **Asking to see what is not there makes it.** A track with no
            // automation has nothing to show, and the toggle is how one is
            // added -- the same way a double click on a header adds a track
            // rather than opening a dialogue about one. It is the *gain*
            // curve, flat at unity across the piece, because that is the one
            // every track has a port for and the one a hand reaches for first.
            //
            // Provisional, and the shape rather than the design: what a track
            // may automate is its own question (a port list, a plugin's
            // parameters) and this is the smallest thing that makes an
            // arrangement with automation editable while that is worked out.
            if track.automation.is_empty() && strip.curves {
                let mut made = Automation::new(NodeId(next), Opaque(json!({"port": "gain"})));
                next += 1;
                made.name = Some("gain".into());
                made.points = vec![
                    Point {
                        at: 0.0,
                        value: 1.0,
                        data: Opaque::none(),
                    },
                    Point {
                        at: span.max(1.0),
                        value: 1.0,
                        data: Opaque::none(),
                    },
                ];
                track.automation.push(made);
            }
            for curve in &mut track.automation {
                curve.visible = strip.curves;
            }
        }
        // **Only when it moved, and moved is measured at the width the wire
        // carries** *(found 2026-09-12 by use: a window that had just opened
        // recorded an edit per track before a hand touched it)*. A fader is an
        // `f32` on the way out and an `f64` in the document, so a level of
        // `0.7` comes back as `0.699999988` — which is a different number by
        // `f64::EPSILON` and the same number to everything that will ever read
        // it. Compared at `f64` the report of an untouched header was an edit,
        // the answer rewrote the level, and the next report differed again.
        // A track that never named a level sits at unity, so writing one
        // unconditionally would make a piece that changed nothing look edited —
        // which is the same rule, at the precision it has to be read at.
        let gain = strip.gain.max(0.0);
        if gain as f32 != level_of(&track) as f32 {
            track.level = gain;
            // What an older piece carried in the table is now the field's, and
            // leaving it would be two answers to one question.
            if let Some(table) = track.config.0.as_object_mut() {
                table.remove(LEVEL);
            }
        }
        tracks.push(track);
    }
    if tracks == piece.tracks {
        return Vec::new();
    }
    vec![MultitrackIntent::SetTracks { tracks }]
}

/// A box as a hand left it — what [`read`] is given, and the same shape
/// [`boxes`] hands out with the two fields a picture cannot answer for dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// The name it came back under. **An id where the box is one the view was
    /// given, and anything at all where a hand made it**: a split names its
    /// halves after the box they came from, which is how a new box is told from
    /// a moved one.
    pub name: String,
    /// The row it is on, by its track's id. A row is never renamed, so this is
    /// always an id.
    pub row: NodeId,
    /// Where it now starts.
    pub position: Second,
    /// How long it now occupies.
    pub length: Second,
    /// The frame of the source its zero reads, in seconds.
    pub start: f64,
    /// How much of the source it shows, in seconds.
    pub content: f64,
    /// What it is a window onto, where the caller could resolve one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
}

/// **What a `"boxes"` report means**, as edits in the piece's own vocabulary.
///
/// The report is the *piece*, not the gesture — a move, a block drag, a trim, a
/// split, a delete and a paste all arrive as one list — so nothing here asks
/// which gesture ran. What comes out is the difference:
///
/// - a box that stayed on its row and changed length is a
///   [`MultitrackIntent::TrimRegion`], which is the verb that changes what a
///   region reads;
/// - a box that moved, or changed row, is a
///   [`MultitrackIntent::PlaceRegion`] — one verb for both, which is why a
///   crossing is not a second mechanism;
/// - a row that gained a box the piece has no region for, or lost one the
///   report no longer names, is stated **whole**
///   ([`MultitrackIntent::SetLane`]), which is the piece's own verb for a
///   lane's contents and what a split, a join and a paste all invert to.
///
/// `next_id` is where minted region ids start; a caller with nothing better to
/// say passes [`fresh_id`]. Ids are the piece's and a hand that made a box has
/// none to offer.
///
/// `reach_of` answers how long a source is, in seconds, where the caller knows:
/// a box a hand made is a window onto its **whole** source, so that is the
/// duration its window is written with (see [`crate::SegmentRef::duration`]).
/// Unknown, it falls back to what the box shows.
pub fn read(
    piece: &Multitrack,
    placed: &[Placed],
    next_id: u64,
    reach_of: &dyn Fn(SourceId) -> Option<f64>,
) -> Vec<MultitrackIntent> {
    let rows = rows(piece);
    let mut out = Vec::new();
    let mut seen: Vec<NodeId> = Vec::new();
    let mut fresh: Vec<(NodeId, Placed)> = Vec::new();
    for box_ in placed {
        // A box naming a row the piece has none of is **kept where it is**: the
        // view hands back what it could not place so it can be re-homed, and
        // acting on it would be moving a region onto a track that is not there.
        let Some(row) = rows.iter().find(|r| r.track == box_.row) else {
            continue;
        };
        let found = box_
            .name
            .parse::<u64>()
            .ok()
            .map(NodeId)
            .and_then(|id| find_region(piece, id).map(|f| (id, f)));
        let Some((region_id, (track, region))) = found else {
            fresh.push((row.lane, box_.clone()));
            continue;
        };
        seen.push(region_id);
        let crossed = track != box_.row;
        let moved = (box_.position - region.position).0.abs() > f64::EPSILON;
        let resized = (box_.length - region.length).0.abs() > f64::EPSILON;
        // **What the box reads, and from where.** A trim of the *left* edge
        // slides the window over the source -- that is what makes an edge drag
        // a trim and not a squeeze -- so a report whose `start` moved is saying
        // the box reads from somewhere else now, and dropping that left the
        // picture and the document disagreeing about which samples a box is
        // over.
        let (_, start, ..) = window_of(region);
        let rewound = (box_.start - start).abs() > f64::EPSILON;
        if resized || rewound {
            out.push(MultitrackIntent::TrimRegion {
                region: region_id,
                position: box_.position,
                length: box_.length,
                // Only what actually moved: the window's own **duration** is
                // left as the piece states it, since a flat report says how
                // long the box is and not how much of the source is behind it.
                content: rewound.then(|| rewound_content(region, box_)),
            });
        }
        if crossed || (moved && !resized && !rewound) {
            out.push(MultitrackIntent::PlaceRegion {
                region: region_id,
                track: box_.row,
                lane: row.lane,
                position: box_.position,
                layer: region.layer,
            });
        }
    }
    out.extend(lane_lists(
        piece, &rows, &seen, &fresh, next_id, placed, reach_of,
    ));
    out
}

/// An id past everything the piece already names — its tracks, its lanes, its
/// regions **and its automations**, which share one id space.
///
/// The curves are in the count because they are in the space: a piece looks an
/// id up by number ([`Multitrack::automation`]) and does not ask what kind of
/// thing it expected, so handing out an id a curve already holds is how two
/// things come to answer to one name.
pub fn fresh_id(piece: &Multitrack) -> u64 {
    piece
        .tracks
        .iter()
        .flat_map(|track| {
            std::iter::once(track.id.0)
                .chain(track.automation.iter().map(|a| a.id.0))
                .chain(track.lanes.iter().flat_map(|lane| {
                    std::iter::once(lane.id.0).chain(lane.regions.iter().flat_map(|r| {
                        std::iter::once(r.id.0).chain(r.automation.iter().map(|a| a.id.0))
                    }))
                }))
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// The seam between two spans that do not continue each other, in seconds.
///
/// A step is a click however well the frames are read, so a cut gets the few
/// milliseconds an editor puts on one. It is here rather than in a client
/// because it is part of what the join *is*: two clients that chose their own
/// would make the same edit sound different, which is the whole reason the
/// arithmetic lives in this crate.
///
/// Only at a seam that is one — two parts that *do* read on from each other are
/// left alone, where a fade would be audible damage to material that was
/// continuous.
pub const SEAM: f64 = 0.010;

/// A source id nothing is using — neither this piece **nor whoever holds the
/// samples**.
///
/// Minted the way a region's id is: from what is there, so the same piece
/// answers the same way twice and a test can say what a join will be called.
///
/// **`taken` is not an optimization and leaving it out was a defect** *(found
/// 2026-09-12 by the user: a second join left an empty box)*. A source stops
/// being named by the piece the moment nothing windows it — an undo, a box
/// deleted — while the client still holds the buffer it made for it. Minted off
/// the piece alone, the next join hands back an id that already has samples
/// behind it, and the box is then a window onto **the previous join**: the
/// client sees an id it knows, makes nothing, and the box draws and plays
/// whatever that was. The piece cannot see the table, so the table says.
pub fn fresh_source(piece: &Multitrack, taken: &[SourceId]) -> SourceId {
    let used = piece
        .tracks
        .iter()
        .flat_map(|track| track.lanes.iter())
        .flat_map(|lane| lane.regions.iter())
        .filter_map(|region| window_of(region).0)
        .chain(taken.iter().copied())
        .map(|id| id.0)
        .max()
        .unwrap_or(0);
    SourceId(used + 1)
}

/// **What a `"join"` report means**: these boxes become one.
///
/// The one verb of the multitrack that cannot be read out of the picture it
/// leaves. A move, a trim, a split and a delete are all differences — the
/// report is the piece and [`read`] says what changed — but a join and a
/// "delete one, lengthen the other" leave a lane holding exactly the same
/// thing, and a box in the report names **one** source and one start. So a join
/// is stated, and this is the statement.
///
/// Two answers, and which one it is depends on the material rather than on the
/// gesture:
///
/// - The boxes read **one run of one source, in order** — the halves of a cut
///   put back — so the join is a plain window over the whole of it, which is
///   what it always was. Nothing is minted: a source made of one span of one
///   take is a pseudobuffer that says nothing the take does not.
/// - They do not, which is the case a region **cannot state**: a region is one
///   window onto one source, so fragments in an order their source does not
///   have have no region that describes them. What is missing is the source, so
///   the join makes one ([`Location::Segments`]) and the box is then a plain
///   window onto it, from its zero, for the whole of it.
///
/// `rate` is frames per second on the shared axis: the parts of a join are
/// **frames**, which is what the server takes them in and what
/// [`SourceRef::range`] speaks, while a box's window is seconds.
///
/// The refusals are the three cases a join is not, and they are returned rather
/// than dropped: a verb that does nothing and says nothing is indistinguishable
/// from one that does not work.
pub fn read_join(
    piece: &Multitrack,
    names: &[String],
    rate: f64,
    taken: &[SourceId],
    parts_of: &dyn Fn(SourceId) -> Option<Vec<Part>>,
    frames_of: &dyn Fn(SourceId) -> Option<u64>,
) -> Result<Vec<MultitrackIntent>, &'static str> {
    let mut held: Vec<(NodeId, NodeId, &Region)> = Vec::new();
    for name in names {
        // **A box the hand is holding and the piece does not have is refused,
        // not dropped.** Joining the rest would leave that one where it is,
        // under the box that now spans over it -- and a verb that quietly acts
        // on less than it was given is the shape of every bug this seam has
        // produced.
        let found = name
            .parse::<u64>()
            .ok()
            .map(NodeId)
            .and_then(|id| lane_of_region(piece, id).map(|(lane, region)| (id, lane, region)));
        let Some(found) = found else {
            return Err("one of these boxes is not one the piece has");
        };
        held.push(found);
    }
    if held.len() < 2 {
        return Err("a join needs two boxes or more in hand");
    }
    if held.iter().any(|(_, lane, _)| *lane != held[0].1) {
        return Err("a join is a lane's, and these boxes are on two");
    }
    held.sort_by(|a, b| {
        a.2.position
            .partial_cmp(&b.2.position)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // **What each box reads**, beside where it sits. A box that is not a window
    // onto samples has nothing a part could name.
    let mut spans = Vec::new();
    for (_, _, region) in &held {
        let (Some(source), start, _, looping) = window_of(region) else {
            return Err("one of these boxes is not a window onto samples");
        };
        if looping {
            return Err("a box that wraps cannot be one span of a join");
        }
        // **What the box shows, not what its window claims.** A trim slides the
        // window's start and leaves its duration as the piece had it (`read`),
        // so after a left-hand trim or a split a window claims more of its
        // source than the box plays -- and a part built from that claim asked
        // the server for samples the take does not have, which refused the
        // whole stitch and left the joined box empty (found 2026-09-13). What
        // is heard is the box's length from its start, so that is the span.
        let shown = region.length.get();
        if shown <= 0.0 {
            return Err("one of these boxes reads nothing");
        }
        spans.push((source, start, shown, *region));
    }
    // **Where two boxes meet is asked in samples**, at the view's rate: a
    // second is a double, and an end and the next start that land on one
    // sample may still differ in their last bit.
    let sample = |secs: f64| {
        if rate > 0.0 {
            (secs * rate).round_ties_even()
        } else {
            secs
        }
    };
    for pair in held.windows(2) {
        let (end, next) = (sample(pair[0].2.end().0), sample(pair[1].2.position.0));
        if next > end {
            return Err("there is a gap between these boxes, and a join cannot state silence yet");
        }
        if end > next {
            return Err("these boxes overlap, and a join cannot state a mix yet");
        }
    }
    let regions: Vec<NodeId> = held.iter().map(|(id, ..)| *id).collect();
    let into = regions[0];
    // **One run of one source, in order**: the join it always was.
    let frame = if rate > 0.0 { 0.5 / rate } else { f64::EPSILON };
    let one_run = spans.windows(2).all(|pair| {
        let (a, b) = (&pair[0], &pair[1]);
        a.0 == b.0 && (b.1 - (a.1 + a.2)).abs() <= frame
    });
    if one_run {
        return Ok(vec![MultitrackIntent::JoinRegions {
            regions,
            into,
            content: None,
            source: None,
        }]);
    }
    let id = fresh_source(piece, taken);
    let frames = |secs: f64| (secs * rate).round().max(0.0) as u64;
    let seam = frames(SEAM);
    let mut parts = Vec::new();
    for (i, (source, start, duration, _)) in spans.iter().enumerate() {
        // A seam is a seam only where the material is cut. Two parts that read
        // on from each other are the same recording and are left alone.
        let cut_before = i > 0 && {
            let before = &spans[i - 1];
            before.0 != *source || (*start - (before.1 + before.2)).abs() > frame
        };
        let cut_after = i + 1 < spans.len() && {
            let after = &spans[i + 1];
            after.0 != *source || (after.1 - (start + duration)).abs() > frame
        };
        // **Flat, whatever the box windows.** A segment names a take and a
        // span of it, and a join is a new list of those -- never a list of
        // lists. A box over a join reads through to the takes that join is
        // made of, cut to what the box shows; built over the join itself, a
        // join of joins nested one source inside another until the server
        // refused it (found 2026-09-13: `sources are stitched more than 4
        // deep`).
        let mut pieces = segments_of(
            *source,
            frames(*start),
            frames(start + duration),
            parts_of,
            frames_of,
        )?;
        let last = pieces.len() - 1;
        for (k, piece) in pieces.iter_mut().enumerate() {
            if k == 0 {
                piece.fade_in = if cut_before { seam } else { 0 };
            }
            if k == last {
                piece.fade_out = if cut_after { seam } else { 0 };
            }
        }
        parts.extend(pieces);
    }
    let total: f64 = spans.iter().map(|(_, _, duration, _)| duration).sum();
    Ok(vec![MultitrackIntent::JoinRegions {
        regions,
        into,
        content: Some(Content::window(SegmentRef {
            source: SegmentSource::Samples(SourceRef {
                source: id,
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: total,
        })),
        source: Some(MintedSource {
            id,
            source: Source {
                location: Location::Segments { parts },
                lifetime: Lifetime::Session,
                generation: 0,
                // **Left unstated, and that is the honest answer.** How wide a
                // join is depends on the takes it is over, and this crate holds
                // source ids rather than sources: whoever has the samples fills
                // it in when it realizes the join.
                channels: None,
                frames: Some(frames(total)),
                sample_rate: Some(rate),
                provenance: None,
                editing: None,
                extra: Default::default(),
            },
        }),
    }])
}

/// **The segments frames `from`..`to` of `source` are**: one span of the
/// source itself when it is a take, or -- when it is a join `parts_of` knows --
/// the spans of the takes that join is made of that fall inside, each cut to
/// the span and keeping its own seam's fade only where that seam is kept.
///
/// Not recursive, and it does not need to be: a join's parts name takes, since
/// every join is made flat, so what this returns names takes too. A part that
/// states no range cannot be cut, and a span past the end of the join reads
/// nothing; both are refused rather than joined around.
fn segments_of(
    source: SourceId,
    from: u64,
    to: u64,
    parts_of: &dyn Fn(SourceId) -> Option<Vec<Part>>,
    frames_of: &dyn Fn(SourceId) -> Option<u64>,
) -> Result<Vec<Part>, &'static str> {
    let Some(parts) = parts_of(source) else {
        // **A box trimmed past the end of its take is refused here**, as an
        // edit with its reason, rather than minted into a source the server
        // then refuses to stitch -- which left the piece holding a joined box
        // over nothing (found 2026-09-13). A trim is not bounded by its take,
        // so this is the first place the two meet. One frame over is the
        // rounding of seconds to frames, and is cut rather than refused.
        let mut to = to;
        if let Some(length) = frames_of(source)
            && to > length
        {
            if from >= length || to - length > 1 {
                return Err("one of these boxes reads past the end of its take");
            }
            to = length;
        }
        return Ok(vec![Part {
            source: SourceRef {
                source,
                lifetime: Lifetime::Session,
                generation: 0,
                range: Some(Range {
                    start: from,
                    end: to,
                }),
            },
            fade_in: 0,
            fade_out: 0,
            channels: None,
        }]);
    };
    let mut out = Vec::new();
    let mut at = 0u64;
    for part in parts {
        let Some(range) = part.source.range else {
            return Err(
                "one of these boxes is a join whose parts do not say which frames they are",
            );
        };
        let (lo, hi) = (at, at + range.len());
        at = hi;
        let (a, b) = (from.max(lo), to.min(hi));
        if a >= b {
            continue;
        }
        let mut piece = part.clone();
        piece.source.range = Some(Range {
            start: range.start + (a - lo),
            end: range.start + (b - lo),
        });
        if a != lo {
            piece.fade_in = 0;
        }
        if b != hi {
            piece.fade_out = 0;
        }
        out.push(piece);
    }
    if out.is_empty() {
        return Err("one of these boxes reads past the end of the join it is a window onto");
    }
    Ok(out)
}

/// The lane a region is on, and the region.
fn lane_of_region(piece: &Multitrack, region: NodeId) -> Option<(NodeId, &Region)> {
    piece.tracks.iter().find_map(|track| {
        track.lanes.iter().find_map(|lane| {
            lane.regions
                .iter()
                .find(|r| r.id == region)
                .map(|found| (lane.id, found))
        })
    })
}

/// **What each lane now holds**, for the two changes a placement cannot state:
/// a region the report no longer names, and a box the piece has no region for.
fn lane_lists(
    piece: &Multitrack,
    rows: &[Row],
    seen: &[NodeId],
    fresh: &[(NodeId, Placed)],
    mut next: u64,
    placed: &[Placed],
    reach_of: &dyn Fn(SourceId) -> Option<f64>,
) -> Vec<MultitrackIntent> {
    let mut out = Vec::new();
    for row in rows {
        let Some(lane) = piece
            .tracks
            .iter()
            .find(|t| t.id == row.track)
            .and_then(active_lane)
        else {
            continue;
        };
        // **A box over samples nobody resolved is not invented**: the document
        // would name a source that cannot be opened, and a piece that will not
        // reopen is worse than a box that did not stick. Dropped here rather
        // than while building, so a lane that gained only such boxes is not
        // rewritten to say nothing.
        let added: Vec<&Placed> = fresh
            .iter()
            .filter(|(id, box_)| *id == lane.id && box_.source.is_some())
            .map(|(_, box_)| box_)
            .collect();
        let gone = lane.regions.iter().any(|r| !seen.contains(&r.id));
        if added.is_empty() && !gone {
            continue;
        }
        // **A kept region is kept as the report left it**, not as the piece
        // still holds it. A lane stated whole is stated *last*, so a clone of
        // what the piece says would undo the trim and the move the same report
        // asked for a moment earlier — which is what made a split leave its
        // first half at full length, playing over the second.
        let mut regions: Vec<Region> = lane
            .regions
            .iter()
            .filter(|r| seen.contains(&r.id))
            .map(|region| {
                let mut region = region.clone();
                if let Some(box_) = placed
                    .iter()
                    .find(|b| b.name.parse::<u64>().ok() == Some(region.id.0))
                {
                    region.position = box_.position;
                    region.length = box_.length;
                    if let Content::Window { window, .. } = &mut region.content {
                        window.start = box_.start;
                    }
                }
                region
            })
            .collect();
        for box_ in added {
            let Some(source) = box_.source else { continue };
            regions.push(Region::new(
                NodeId(next),
                box_.position,
                box_.length,
                Content::Window {
                    window: crate::SegmentRef {
                        source: crate::SegmentSource::Samples(crate::SourceRef {
                            source,
                            lifetime: crate::Lifetime::Session,
                            generation: 0,
                            range: None,
                        }),
                        start: box_.start,
                        // What the window reaches, not what the box shows: a
                        // box a hand made can be pulled out to its whole source.
                        duration: reach_of(source)
                            .filter(|secs| *secs > 0.0)
                            .unwrap_or(box_.content),
                    },
                    playrate: 1.0,
                    args: crate::Opaque::none(),
                    // A box a hand made does not loop: looping is what a box
                    // longer than its source means, and a new one is exactly
                    // as long as what it was given.
                    looping: false,
                },
            ));
            next += 1;
        }
        out.push(MultitrackIntent::SetLane {
            lane: lane.id,
            regions,
        });
    }
    out
}

/// The lane a track plays, which is the one a row draws.
fn active_lane(track: &Track) -> Option<&Lane> {
    track.active_lane().or_else(|| track.lanes.first())
}

/// The fader a track is at: its own field, falling back to the key an older
/// piece carried it under (see [`LEVEL`]).
pub fn level_of(track: &Track) -> f64 {
    if track.level != 1.0 {
        return track.level;
    }
    track
        .config
        .0
        .get(LEVEL)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0)
}

/// **The same content, over the part of the source the report names** — the
/// window slid to the reported `start`, looping as the report says, and every
/// other field of it untouched.
///
/// A region whose content is not a window (a composite) is handed back
/// unchanged: there is no window to slide, and inventing one would replace what
/// the box actually holds.
fn rewound_content(region: &Region, box_: &Placed) -> Content {
    let mut content = region.content.clone();
    if let Content::Window { window, .. } = &mut content {
        window.start = box_.start;
    }
    content
}

/// The samples a region is a window onto, where it opens, how much there is,
/// and whether the window wraps.
fn window_of(region: &Region) -> (Option<SourceId>, f64, f64, bool) {
    let Content::Window {
        window, looping, ..
    } = &region.content
    else {
        return (None, 0.0, 0.0, false);
    };
    (
        window.source.samples().map(|s| s.source),
        window.start,
        window.duration,
        *looping,
    )
}

/// The track a region is on, and the region itself.
fn find_region(piece: &Multitrack, region: NodeId) -> Option<(NodeId, &Region)> {
    piece.tracks.iter().find_map(|track| {
        track
            .lanes
            .iter()
            .find_map(|lane| lane.regions.iter().find(|r| r.id == region))
            .map(|found| (track.id, found))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multitrack::{Automation, Track};

    /// A piece of one track with one box on it, a track automation and a box
    /// envelope.
    fn piece() -> Multitrack {
        let mut region = Region::new(
            NodeId(3),
            Second(0.0),
            Second(4.0),
            Content::Unknown(serde_json::Value::Null),
        );
        region.automation.push(curve_at(NodeId(5), 0.25));
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.lanes[0].regions.push(region);
        track.automation.push(curve_at(NodeId(4), 0.5));
        let mut piece = Multitrack::default();
        piece.tracks.push(track);
        piece
    }

    fn curve_at(id: NodeId, value: f64) -> Automation {
        let mut a = Automation::new(id, Opaque::none());
        a.name = Some(format!("curve {}", id.0));
        a.points = vec![Point {
            at: 0.0,
            value,
            data: Opaque::none(),
        }];
        a
    }

    /// **The same curve in two places, handed out by two calls** — a track's is
    /// a row of its own, a region's a layer inside its box, and a caller draws
    /// them somewhere different, so it never has to tell them apart.
    #[test]
    fn a_track_curve_is_a_row_and_a_region_curve_is_a_layer() {
        let piece = piece();
        let rows = curves(&piece);
        let layers = layers(&piece);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].automation, NodeId(4));
        assert_eq!(rows[0].owner, NodeId(1), "the track it is under");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].owner, NodeId(3), "the box it is inside");
        assert_eq!(rows[0].label, "curve 4");
    }

    /// A curve with no name is still addressable: the label falls back to
    /// something a header can draw, and the identity stays the id.
    #[test]
    fn a_nameless_curve_is_labelled_by_its_id() {
        let mut piece = piece();
        piece.tracks[0].automation[0].name = None;
        assert_eq!(curves(&piece)[0].label, "automation 4");
    }

    /// **What a report of the points means**: one edit per curve that actually
    /// moved, and nothing at all for a hand that looked without editing.
    #[test]
    fn the_points_report_is_read_as_the_difference() {
        let piece = piece();
        let same: Vec<Curved> = curves(&piece)
            .into_iter()
            .chain(layers(&piece))
            .map(|c| Curved {
                name: c.automation.0.to_string(),
                points: c.points,
            })
            .collect();
        assert!(read_points(&piece, &same).is_empty(), "nothing moved");

        let mut moved = same.clone();
        moved[0].points[0].value = 0.9;
        let intents = read_points(&piece, &moved);
        assert_eq!(intents.len(), 1, "the one that moved");
        assert!(matches!(
            &intents[0],
            MultitrackIntent::SetAutomation { automation, points }
                if *automation == NodeId(4) && points[0].value == 0.9
        ));
    }

    /// A name that is no automation's id is dropped rather than minted: a
    /// curve is declared by whoever holds the piece.
    /// **A trim of the left edge slides the window over the source.** That is
    /// what makes an edge drag a trim and not a squeeze, so a report whose
    /// `start` moved is saying the box reads from somewhere else now — and
    /// dropping it left the picture and the document disagreeing about which
    /// samples a box is over, which is heard as both halves of a split playing
    /// the beginning.
    #[test]
    fn a_report_that_moved_the_window_says_what_the_box_now_reads() {
        let mut piece = piece();
        piece.tracks[0].lanes[0].regions[0].content = Content::window(crate::SegmentRef {
            source: crate::SegmentSource::Samples(crate::SourceRef {
                source: crate::SourceId(1),
                lifetime: crate::Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 8.0,
        });
        let held = boxes(&piece)[0].clone();
        let placed = |start: f64| Placed {
            name: held.region.0.to_string(),
            row: held.row,
            position: held.position,
            length: held.length,
            start,
            content: held.content,
            source: held.source,
        };
        // What is already true is no edit at all.
        let out = read(&piece, &[placed(0.0)], fresh_id(&piece), &|_| None);
        assert!(out.is_empty(), "nothing moved: {out:?}");

        // The window slid: the box reads from two seconds in.
        let out = read(&piece, &[placed(2.0)], fresh_id(&piece), &|_| None);
        let [MultitrackIntent::TrimRegion { content, .. }] = &out[..] else {
            panic!("one trim, carrying what it now reads: {out:?}");
        };
        let Some(Content::Window { window, .. }) = content else {
            panic!("the window, slid");
        };
        assert_eq!(window.start, 2.0);
        assert_eq!(window.duration, 8.0, "and nothing else");
    }

    /// **A box a hand made is a window onto its whole source** (decided
    /// 2026-09-13). The box shows two seconds of an eight-second take; its
    /// window reaches all eight, so pulling an edge out later shows the rest.
    /// A caller that does not know the source's length writes what the box
    /// shows, as before.
    #[test]
    fn a_new_box_windows_its_whole_source() {
        let piece = piece();
        let held = boxes(&piece)[0].clone();
        let made = Placed {
            name: "new".into(),
            row: held.row,
            position: Second(4.0),
            length: Second(2.0),
            start: 1.0,
            content: 2.0,
            source: Some(crate::SourceId(1)),
        };
        let window_of_new = |out: &[MultitrackIntent]| {
            out.iter()
                .find_map(|intent| match intent {
                    MultitrackIntent::SetLane { regions, .. } => regions
                        .iter()
                        .find(|r| r.position == Second(4.0))
                        .and_then(|r| r.content.as_window().cloned()),
                    _ => None,
                })
                .expect("the new box, windowing its source")
        };
        let known = read(
            &piece,
            std::slice::from_ref(&made),
            fresh_id(&piece),
            &|s| (s == crate::SourceId(1)).then_some(8.0),
        );
        let window = window_of_new(&known);
        assert_eq!((window.start, window.duration), (1.0, 8.0));

        let unknown = read(&piece, &[made], fresh_id(&piece), &|_| None);
        assert_eq!(window_of_new(&unknown).duration, 2.0);
    }

    /// **A lane stated whole is stated as the report left it**, not as the
    /// piece still holds it.
    ///
    /// A lane's whole list is the *last* intent a report produces, so a clone
    /// of what the piece says undoes the trim and the move the same report
    /// asked for a moment earlier. That is what made a split leave its first
    /// half at full length, playing over the second — the picture was right and
    /// the document was not.
    ///
    /// Found by use 2026-09-10.
    #[test]
    fn a_split_leaves_the_first_half_short() {
        let mut piece = piece();
        piece.tracks[0].lanes[0].regions[0].content = Content::window(crate::SegmentRef {
            source: crate::SegmentSource::Samples(crate::SourceRef {
                source: crate::SourceId(1),
                lifetime: crate::Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 8.0,
        });
        let held = boxes(&piece)[0].clone();
        let same = |name: &str, at: f64, len: f64, start: f64| Placed {
            name: name.into(),
            row: held.row,
            position: Second(at),
            length: Second(len),
            start,
            content: len,
            source: held.source,
        };
        // The report a split sends: the original shortened, and a tail beside
        // it under a name that is no region's id.
        let out = read(
            &piece,
            &[same("3", 0.0, 2.0, 0.0), same("3 2", 2.0, 2.0, 2.0)],
            fresh_id(&piece),
            &|_| None,
        );
        for intent in &out {
            crate::multitrack::edit::apply(
                &mut piece,
                intent,
                &Default::default(),
                &Default::default(),
            );
        }
        let lane = &piece.tracks[0].lanes[0];
        assert_eq!(
            lane.regions.len(),
            2,
            "the half that stayed and the new one"
        );
        let first = lane
            .regions
            .iter()
            .find(|r| r.id == NodeId(3))
            .expect("the original");
        assert_eq!(
            first.length,
            Second(2.0),
            "shortened, and it stayed shortened"
        );
        let tail = lane
            .regions
            .iter()
            .find(|r| r.id != NodeId(3))
            .expect("the tail");
        assert_eq!(tail.position, Second(2.0));
        assert_eq!(tail.content.as_window().map(|w| w.start), Some(2.0));
    }

    /// **A curve holds an id like anything else does.** `fresh_id` answers
    /// with an id past everything the piece names, and a piece looks one up by
    /// number without asking what kind of thing it expected — so a count that
    /// skipped the automations would hand out an id a curve already had.
    ///
    /// Found 2026-09-09 while adding a track from the header: the new track was
    /// minted onto the track curve's id.
    #[test]
    fn a_fresh_id_is_past_the_curves_too() {
        let piece = piece();
        // Track 1, lane 2, region 3, the track curve 4, the box envelope 5.
        assert_eq!(fresh_id(&piece), 6);
    }

    /// **A header nobody touched is not an edit** *(found 2026-09-12 by use: a
    /// window that had just opened recorded one edit per track before a hand
    /// reached it, and the answer to each made the next one differ again)*.
    ///
    /// A fader crosses the wire as an `f32` and lives in the document as an
    /// `f64`, so `0.7` comes back `0.699999988`: a different number by
    /// `f64::EPSILON` and the same number to everything that will ever read it.
    /// The comparison is at the width the wire carries, which is the only width
    /// the answer can be trusted to.
    #[test]
    fn a_level_that_only_crossed_an_f32_is_not_a_fader_that_moved() {
        let mut piece = piece();
        piece.tracks[0].level = 0.7;
        let held = Strip {
            name: "1".into(),
            mute: false,
            solo: false,
            gain: f64::from(0.7f32),
            curves: false,
        };
        assert!(
            read_rows(&piece, std::slice::from_ref(&held)).is_empty(),
            "the same level, through the width it was drawn at"
        );
        // And a fader that did move is still an edit, at the width a hand can
        // put it at.
        let moved = Strip { gain: 0.5, ..held };
        assert_eq!(read_rows(&piece, &[moved]).len(), 1);
    }

    /// **A rows report is the tracks, whole.** A name that is an id is that
    /// track; one that is not is a track a hand made; a track the report does
    /// not name is gone, and its boxes with it. All of it is one `SetTracks`,
    /// because the tracks are one list and a hand did one thing to it.
    #[test]
    fn the_rows_report_states_the_tracks_and_a_new_name_makes_one() {
        let piece = piece();
        let held = Strip {
            name: "1".into(),
            mute: false,
            solo: false,
            gain: 1.0,
            // What this piece's own row reports: its curve is not visible.
            curves: false,
        };
        // What is already true is no edit at all.
        assert!(read_rows(&piece, std::slice::from_ref(&held)).is_empty());

        // The mixer: one verb over a track, and the level lands in the config
        // table the client reads it out of.
        let muted = Strip {
            mute: true,
            gain: 0.5,
            ..held.clone()
        };
        let out = read_rows(&piece, &[muted]);
        let [MultitrackIntent::SetTracks { tracks }] = &out[..] else {
            panic!("one whole statement, whatever changed: {out:?}");
        };
        assert!(tracks[0].muted);
        assert_eq!(level_of(&tracks[0]), 0.5);

        // A name that is no track's id is a track a hand added — with a lane,
        // since a track that could hold nothing is not one — and the ids come
        // from the piece's own counter.
        let out = read_rows(
            &piece,
            &[
                held.clone(),
                Strip {
                    name: "new".into(),
                    ..held.clone()
                },
            ],
        );
        let [MultitrackIntent::SetTracks { tracks }] = &out[..] else {
            panic!("one statement: {out:?}");
        };
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].id, NodeId(6), "past everything the piece names");
        assert_eq!(tracks[1].lanes.len(), 1, "and it can hold a box");

        // A track the report leaves out is gone, and it takes its boxes.
        let out = read_rows(&piece, &[]);
        let [MultitrackIntent::SetTracks { tracks }] = &out[..] else {
            panic!("one statement: {out:?}");
        };
        assert!(tracks.is_empty());
    }

    /// **The label is not read.** A row's label is a made-up `track N` where
    /// the track has no name of its own, so believing the report would write
    /// that string into the document the first time anything else moved.
    #[test]
    fn a_rows_label_is_drawn_and_never_written_back() {
        let piece = piece();
        let out = read_rows(
            &piece,
            &[Strip {
                name: "1".into(),
                mute: false,
                solo: false,
                gain: 1.0,
                curves: false,
            }],
        );
        assert!(out.is_empty(), "the picture's own label is not a change");
        assert_eq!(rows(&piece)[0].label, "track 1", "which is what it draws");
        assert!(piece.tracks[0].name.is_none(), "and not what it holds");
    }

    #[test]
    fn a_curve_the_piece_never_declared_is_not_made_by_dragging_it() {
        let piece = piece();
        let stray = vec![Curved {
            name: "hello".into(),
            points: vec![Point {
                at: 0.0,
                value: 1.0,
                data: Opaque::none(),
            }],
        }];
        assert!(read_points(&piece, &stray).is_empty());
    }

    /// **A number compares by value, not by spelling.** A page writes `0.0` as
    /// `0` and a script writes it as `0.0`; a curve reported back exactly as it
    /// was drawn is no edit in either.
    #[test]
    fn a_point_s_data_compares_by_what_it_says() {
        let mut piece = Multitrack::default();
        let mut track = Track::new(NodeId(1), NodeId(2));
        let mut curve = Automation::new(NodeId(4), Opaque::none());
        curve.points = vec![Point {
            at: 0.0,
            value: 0.0,
            data: Opaque(serde_json::json!({ "shape": 1, "curve": 0.0 })),
        }];
        track.automation.push(curve);
        piece.tracks.push(track);

        let spelled = vec![Curved {
            name: "4".into(),
            points: vec![Point {
                at: 0.0,
                value: 0.0,
                data: Opaque(serde_json::json!({ "shape": 1.0, "curve": 0 })),
            }],
        }];
        assert!(read_points(&piece, &spelled).is_empty());

        let moved = vec![Curved {
            name: "4".into(),
            points: vec![Point {
                at: 0.0,
                value: 0.0,
                data: Opaque(serde_json::json!({ "shape": 1, "curve": 0.5 })),
            }],
        }];
        assert_eq!(read_points(&piece, &moved).len(), 1);
    }
}
