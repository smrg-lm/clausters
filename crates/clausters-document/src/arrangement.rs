//! The arrangement: what a multitrack editor edits, written down.
//!
//! Source, **region**, **lane**, **track**, **automation** — the field's own
//! vocabulary, not this project's invention, and the layer that was missing.
//! Until now a multitrack was a *projection* out of a general tree: a lane was
//! what a view made of an aggregate, and the state a multitrack actually has —
//! which track a thing is on, its order, its layer, its identity — had nowhere
//! to live but the widget tree, which is drawn, and drawing frees.
//!
//! # A region is one object
//!
//! REAPER splits the slot in time (`MediaItem`: position, length, fades) from
//! what fills it (`MediaItem_Take`: the source, its offset, its playrate). We
//! do not, and the reason is not the cost of the extra level: **a track holding
//! several lanes is already the comping mechanism**, so the split would give a
//! second one at a different level for the same job. REAPER 7 itself added
//! fixed item lanes as the alternative to recording into takes, with an action
//! named *convert takes to lanes*.
//!
//! So a region carries both halves as **fields on one object**: [`Region`] is
//! the span on the timeline, and [`Content`] is what fills it. The distinction
//! REAPER draws is kept — as types rather than as a nesting, which is where
//! [`crate::timebase`] does its work, since the two halves are measured on two
//! different axes and nothing used to say so.
//!
//! What that costs is stated rather than discovered: swapping what fills a
//! region does not keep its fades, because there is no slot to keep them in.
//!
//! # A region is not a clip
//!
//! `Region` is this model's word and **clip** is the picture's. A clip, a lane
//! row, a waveform are what a host draws; a region is what an intent names.
//! Zrythm made this same turn and merged the two, renaming `Region` to `Clip`;
//! they are kept apart here, because the multitrack's defects came precisely
//! from the thing drawn and the thing addressed being one object.
//!
//! # Identity, and what an id identifies
//!
//! Every region has its own [`NodeId`], separate from the source's. That is the
//! answer to the oldest open decision in `PLAN.md` — *may one element be placed
//! twice* — and it is what makes a source referenced from six places
//! *referenced* rather than copied. Six regions, one source, six identities: an
//! intent naming one names one appearance.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::timebase::Beat;
use crate::{Node, NodeId, Opaque, SegmentRef};

/// Anything a newer writer wrote that this build has no field for, carried so a
/// round trip through an older reader does not lose it.
///
/// The struct-level twin of [`crate::Body::Unknown`], and the same rule: what
/// cannot be interpreted is carried, not dropped. Serde's default is to discard
/// unknown fields silently, which for a format with two writers in two
/// languages is a way to lose a piece.
pub type Extra = Map<String, Value>;

/// The shape of a fade, carried and never interpreted.
///
/// A length plus whatever the client says about the curve, for the reason
/// [`crate::points`] refuses to name interpolation shapes: what an exponential
/// fade *is* belongs to whoever renders it, and guessing here would decide a
/// question nobody has asked. Losing it would straighten every fade on a
/// reopen, which is a different act from declining to interpret it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Fade {
    /// How long the fade lasts, on the timeline's musical axis.
    pub length: Beat,
    /// The curve, in the client's terms.
    #[serde(default, skip_serializing_if = "Opaque::is_empty")]
    pub shape: Opaque,
}

impl Fade {
    /// A fade of this length, with nothing said about its curve.
    pub fn of(length: Beat) -> Self {
        Self {
            length,
            shape: Opaque::none(),
        }
    }
}

/// What fills a region.
///
/// The half REAPER puts in a `Take`. Three kinds, and the third is the door the
/// tree walks through rather than a special case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fill", rename_all = "lowercase")]
pub enum Content {
    /// A **window** onto a source — samples, or a node this document holds.
    ///
    /// [`SegmentRef`] already says which source, where the window opens and how
    /// long it lasts, in the units that source is addressed and measured in. A
    /// region adds the two things a *placement* of that window has and the
    /// window itself does not.
    Window {
        /// Which source, from where, for how long.
        window: SegmentRef,
        /// How fast the window is read, `1.0` being as recorded. A property of
        /// this placement: two regions over one source may play it at two
        /// rates, which is the ordinary case for a sampled instrument and the
        /// reason it is not on the source.
        #[serde(default = "one", skip_serializing_if = "is_one")]
        playrate: f64,
        /// The arguments of **this** evaluation, for a window onto something
        /// generated: a function placed twice is two evaluations, possibly with
        /// different arguments, and the document carries them without reading
        /// them. Empty for a window onto a recording, which evaluates nothing.
        #[serde(default, skip_serializing_if = "Opaque::is_empty")]
        args: Opaque,
    },
    /// A **composite**: the general tree, placed as one region.
    ///
    /// A section, a nested arrangement, anything the five primitives can build.
    /// It carries a [`Node`] unchanged, which is what keeps everything the
    /// document already models reachable from a session without restating it —
    /// and what makes "an arrangement of arrangements" cost nothing.
    Composite {
        /// The tree this region places.
        node: Box<Node>,
    },
    /// A fill this build does not know, preserved whole.
    #[serde(untagged)]
    Unknown(Value),
}

fn one() -> f64 {
    1.0
}

fn is_one(rate: &f64) -> bool {
    *rate == 1.0
}

impl Content {
    /// A window onto a source, read as recorded and evaluating nothing.
    pub fn window(window: SegmentRef) -> Self {
        Self::Window {
            window,
            playrate: 1.0,
            args: Opaque::none(),
        }
    }

    /// The window this content is, when it is one.
    pub fn as_window(&self) -> Option<&SegmentRef> {
        match self {
            Content::Window { window, .. } => Some(window),
            _ => None,
        }
    }

    /// The tree this content places, when it is a composite.
    pub fn as_node(&self) -> Option<&Node> {
        match self {
            Content::Composite { node } => Some(node),
            _ => None,
        }
    }
}

/// One placed thing on a lane: a span of the timeline, and what fills it.
///
/// The span is the region's own — position, length, fades, layer — and it is
/// measured on the **musical** axis, because where a thing sits in a piece is a
/// musical decision. What fills it is measured in its own source's units, which
/// is why the two halves cannot be added and why they are two types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    /// Its identity, and not its source's.
    pub id: NodeId,
    /// A referenceable label — the same rule as [`Node::name`]: a second way to
    /// refer to the region, never a second identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where it starts on the timeline.
    pub position: Beat,
    /// How long it occupies. Not the content's length: a region may show part
    /// of what it holds, and trimming moves this without touching the source.
    pub length: Beat,
    /// Which of the overlapping regions on this lane draws and plays on top.
    ///
    /// Overlap is legal and ordinary — a crossfade *is* an overlap — so the
    /// stack needs an order that survives a save. Higher is nearer the front.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub layer: u32,
    /// The fade in from the region's start, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_in: Option<Fade>,
    /// The fade out ending at the region's end, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_out: Option<Fade>,
    /// Silenced without being removed. The region's own, not its track's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
    /// What fills it.
    pub content: Content,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Extra,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

impl Region {
    /// A region placed at `position`, `length` long, filled with `content`.
    pub fn new(id: NodeId, position: Beat, length: Beat, content: Content) -> Self {
        Self {
            id,
            name: None,
            position,
            length,
            layer: 0,
            fade_in: None,
            fade_out: None,
            muted: false,
            content,
            extra: Extra::new(),
        }
    }

    /// Names it. A label, never an identity.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Where it ends: its position plus its length, on the same axis.
    pub fn end(&self) -> Beat {
        self.position + self.length
    }

    /// Whether the two occupy any of the same time. Half-open, so a region
    /// ending exactly where the next begins does not overlap it — which is what
    /// makes a cut into two regions not a crossfade.
    pub fn overlaps(&self, other: &Region) -> bool {
        self.position < other.end() && other.position < self.end()
    }
}

/// One of a track's several contents: an ordered list of regions.
///
/// Ardour's structure and our name — its `Playlist` is this, and *playlist* is
/// a word every other program spends on something else. A track holds several
/// and plays one, which is what comping is: record six passes into six lanes,
/// then take from each.
///
/// The regions are kept in **position order**, so a written session is stable
/// under re-saving and a diff of two saves is the edits rather than the
/// iteration order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    /// Its identity.
    pub id: NodeId,
    /// A label — "take 3", "comp", "verse".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The regions on it, in position order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<Region>,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Extra,
}

impl Lane {
    /// An empty lane.
    pub fn new(id: NodeId) -> Self {
        Self {
            id,
            name: None,
            regions: Vec::new(),
            extra: Extra::new(),
        }
    }

    /// Names it.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Places a region and keeps the list in position order.
    pub fn place(&mut self, region: Region) {
        let at = self
            .regions
            .partition_point(|r| (r.position, r.layer) <= (region.position, region.layer));
        self.regions.insert(at, region);
    }

    /// The region with this id, if it is here.
    pub fn region(&self, id: NodeId) -> Option<&Region> {
        self.regions.iter().find(|r| r.id == id)
    }

    /// Where the last region ends, or the origin when there are none.
    pub fn end(&self) -> Beat {
        self.regions
            .iter()
            .map(Region::end)
            .fold(Beat::ZERO, Beat::max)
    }
}

/// A curve over one parameter, in the arrangement's own time.
///
/// The points are [`crate::Point`]s and this crate reads nothing about their
/// shape, for the reason that module states. What is *here* rather than there
/// is the placement: which parameter, whose track, and whether the lane is
/// showing — because a curve with no arrangement around it has no parameter to
/// be about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    /// Its identity.
    pub id: NodeId,
    /// A label, when the target's own name is not what the user calls it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// **What this automates**, in the client's terms and never read here: a
    /// control name, a bus, a plugin's parameter index. The same door a leaf's
    /// configuration is, and for the same reason — the parameters of a def
    /// belong to whoever wrote the def.
    #[serde(default, skip_serializing_if = "Opaque::is_empty")]
    pub target: Opaque,
    /// The curve. `at` is on the musical axis, like every other placement here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<crate::Point>,
    /// Whether the lane is shown. **The view's**, and here rather than in the
    /// host because which curves a person had open is part of reopening the
    /// piece as they left it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub visible: bool,
    /// Whether the curve is being applied. A curve can be kept and switched off
    /// without being deleted, which is what an arm/bypass is.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub enabled: bool,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Extra,
}

fn yes() -> bool {
    true
}

fn is_yes(b: &bool) -> bool {
    *b
}

impl Automation {
    /// A curve over `target`, with no points yet.
    pub fn new(id: NodeId, target: Opaque) -> Self {
        Self {
            id,
            name: None,
            target,
            points: Vec::new(),
            visible: false,
            enabled: true,
            extra: Extra::new(),
        }
    }
}

/// A row of the arrangement: several lanes, one of them playing, plus the
/// curves over it and whatever the client says it is.
///
/// **What a track *is* — an instrument, a bus, a folder — is not here.** That
/// is `config`, carried and never interpreted, for the reason a leaf is opaque:
/// a def is code in the language of whoever wrote it. What the document owns is
/// the structure: which lanes, which one plays, what is placed on them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Its identity.
    pub id: NodeId,
    /// What it is called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its contents. A track always has at least one lane; the several are what
    /// comping is made of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<Lane>,
    /// Which lane plays, as an index into [`Track::lanes`].
    ///
    /// An index and not an id because it is a *choice among these*, and a
    /// choice that names something absent is a state the format should not be
    /// able to express. [`Track::active_lane`] answers `None` when it does
    /// anyway, rather than panicking on a file somebody hand-edited.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub active: usize,
    /// The curves over it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<Automation>,
    /// Silenced.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
    /// Soloed. Whether a solo anywhere silences everything else is the mixer's
    /// rule and not the document's; what the document holds is that this one
    /// was marked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub soloed: bool,
    /// What this track is, in the client's terms: its instrument, its routing,
    /// its plugins. Carried, never interpreted.
    #[serde(default, skip_serializing_if = "Opaque::is_empty")]
    pub config: Opaque,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Extra,
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

impl Track {
    /// A track with one empty lane, which is the smallest one that means
    /// anything.
    pub fn new(id: NodeId, lane: NodeId) -> Self {
        Self {
            id,
            name: None,
            lanes: vec![Lane::new(lane)],
            active: 0,
            automation: Vec::new(),
            muted: false,
            soloed: false,
            config: Opaque::none(),
            extra: Extra::new(),
        }
    }

    /// Names it.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The lane that plays, or `None` when [`Track::active`] names one that is
    /// not there.
    pub fn active_lane(&self) -> Option<&Lane> {
        self.lanes.get(self.active)
    }

    /// The lane that plays, to be edited.
    pub fn active_lane_mut(&mut self) -> Option<&mut Lane> {
        self.lanes.get_mut(self.active)
    }

    /// Where the track's last region ends, across **every** lane — what it
    /// spans, not what it plays, since an alternate take is still part of the
    /// piece.
    pub fn end(&self) -> Beat {
        self.lanes.iter().map(Lane::end).fold(Beat::ZERO, Beat::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lifetime, SegmentSource, SourceId, SourceRef};

    fn window(source: u64) -> SegmentRef {
        SegmentRef {
            source: SegmentSource::Samples(SourceRef {
                source: SourceId(source),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 2.0,
        }
    }

    fn region(id: u64, at: f64, len: f64) -> Region {
        Region::new(NodeId(id), Beat(at), Beat(len), Content::window(window(1)))
    }

    #[test]
    fn a_region_ends_where_its_span_ends_and_not_where_its_content_does() {
        // The window is two seconds of the source; the region shows one beat of
        // it. Trimming moves the region and never the source.
        let r = region(1, 4.0, 1.0);
        assert_eq!(r.end(), Beat(5.0));
        assert_eq!(r.content.as_window().unwrap().duration, 2.0);
    }

    #[test]
    fn regions_that_touch_do_not_overlap_and_regions_that_share_time_do() {
        assert!(!region(1, 0.0, 4.0).overlaps(&region(2, 4.0, 4.0)));
        assert!(region(1, 0.0, 4.0).overlaps(&region(2, 3.0, 4.0)));
    }

    #[test]
    fn one_source_under_six_regions_is_referenced_and_not_copied() {
        // The oldest open decision, as a test: six placements, six identities,
        // one source.
        let mut lane = Lane::new(NodeId(10));
        for i in 0..6 {
            lane.place(region(100 + i, i as f64 * 4.0, 4.0));
        }
        let sources: Vec<_> = lane
            .regions
            .iter()
            .map(|r| {
                r.content
                    .as_window()
                    .unwrap()
                    .source
                    .samples()
                    .unwrap()
                    .source
            })
            .collect();
        assert!(sources.iter().all(|s| *s == SourceId(1)));
        assert_eq!(lane.regions.len(), 6);
        assert_eq!(lane.region(NodeId(103)).unwrap().position, Beat(12.0));
    }

    #[test]
    fn placing_keeps_a_lane_in_position_order() {
        let mut lane = Lane::new(NodeId(10));
        lane.place(region(3, 8.0, 2.0));
        lane.place(region(1, 0.0, 2.0));
        lane.place(region(2, 4.0, 2.0));
        let at: Vec<_> = lane.regions.iter().map(|r| r.position).collect();
        assert_eq!(at, vec![Beat(0.0), Beat(4.0), Beat(8.0)]);
        assert_eq!(lane.end(), Beat(10.0));
    }

    #[test]
    fn a_track_spans_every_lane_and_plays_one() {
        let mut track = Track::new(NodeId(1), NodeId(10));
        track.lanes.push(Lane::new(NodeId(11)).named("take 2"));
        track
            .active_lane_mut()
            .unwrap()
            .place(region(100, 0.0, 4.0));
        track.lanes[1].place(region(200, 0.0, 16.0));
        assert_eq!(track.active_lane().unwrap().id, NodeId(10));
        assert_eq!(track.active_lane().unwrap().end(), Beat(4.0));
        // An alternate take is still part of the piece.
        assert_eq!(track.end(), Beat(16.0));
    }

    #[test]
    fn an_active_lane_that_is_not_there_answers_nothing() {
        let mut track = Track::new(NodeId(1), NodeId(10));
        track.active = 7;
        assert!(track.active_lane().is_none());
    }

    #[test]
    fn a_region_round_trips_and_writes_only_what_was_said() {
        let r = region(1, 4.0, 2.0).named("verse");
        let json = serde_json::to_string(&r).unwrap();
        // Defaults stay out of the file: no layer, no fades, no mute, no
        // playrate, no args.
        assert!(!json.contains("layer"), "{json}");
        assert!(!json.contains("playrate"), "{json}");
        assert!(!json.contains("muted"), "{json}");
        assert_eq!(serde_json::from_str::<Region>(&json).unwrap(), r);
    }

    #[test]
    fn a_composite_region_carries_the_general_tree_unchanged() {
        use crate::{Body, Grouping};
        let node = Node::new(
            NodeId(50),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members: Vec::new(),
                config: Opaque::none(),
            },
        );
        let r = Region::new(
            NodeId(1),
            Beat(0.0),
            Beat(8.0),
            Content::Composite {
                node: Box::new(node.clone()),
            },
        );
        let back: Region = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.content.as_node(), Some(&node));
    }

    #[test]
    fn a_field_a_newer_writer_added_survives_a_load_and_a_save() {
        // The acceptance this milestone owes: a session written by a build that
        // knows more than this one must come back out with what it came in
        // with. Serde drops unknown fields by default, which for a format with
        // two writers in two languages is how a piece gets lost.
        let json = r#"{"id":1,"position":0.0,"length":4.0,
                       "content":{"fill":"window",
                                  "window":{"source":{"node":7},"start":0.0,"duration":2.0}},
                       "warp":{"mode":"beats","markers":[1,2,3]}}"#;
        let region: Region = serde_json::from_str(json).unwrap();
        assert!(region.extra.contains_key("warp"));
        let back = serde_json::to_value(&region).unwrap();
        assert_eq!(back["warp"]["mode"], "beats");
        assert_eq!(back["warp"]["markers"][2], 3);
    }

    #[test]
    fn a_fill_this_build_does_not_know_is_carried_whole() {
        let json = r#"{"fill":"video","clip":"take1.mov","offset":0}"#;
        let content: Content = serde_json::from_str(json).unwrap();
        assert!(matches!(content, Content::Unknown(_)));
        let back = serde_json::to_value(&content).unwrap();
        assert_eq!(back["clip"], "take1.mov");
    }
}
