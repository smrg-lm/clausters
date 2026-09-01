//! The Clausters **document**: the single authoritative model of a composition.
//!
//! This crate is the owner every editing path talks to. A GUI host draws and
//! emits intents but holds no data; a client writes a composition through its
//! own idiomatic surface (`clausters.form` in Python) but does not define what
//! an edit *means*; the audio server stores sources and never edits at all.
//! What sits between them — the tree, what an edit does to it, and what an
//! edit's inverse is — is here, once, so that the three deployment modes
//! (client + host + server, the `standalone` host with no language at all, and
//! a headless client) bind one model instead of re-deriving it.
//!
//! `PLAN.md` beside this file carries the roadmap and the decisions behind it.
//! Three of them shape everything in this module and are worth having in view:
//!
//! - **A leaf is opaque.** The document holds a leaf as a kind plus a
//!   configuration it never interprets ([`Opaque`]), because a generator *is
//!   code* in the language of whoever wrote it and no crate in any language can
//!   own one. What it does own is where that leaf sits in time.
//! - **The tree stays general; a view carries its own restrictions.** There is
//!   no lane, no vertical position and no type-per-container here. A multitrack
//!   editor is a *projection* that may decline to show what its shape does not
//!   admit, the way an unknown widget is laid out and not painted.
//! - **Sources are never overwritten.** A [`SourceRef`] names samples and
//!   carries the [`Lifetime`] that says whether it outlives the session, which
//!   is what lets a save be honest about what it is about to promote.
//!
//! # The shape
//!
//! A [`Document`] is a version and a root [`Node`]. A node is temporal metadata
//! — an optional onset and duration — plus a [`Body`] saying what it is. The
//! onset is in **beats** and the duration is in the unit of the data it measures
//! ([`Body::duration_unit`]: seconds for a body that references samples, beats
//! for one made of events), which is the one thing about the shape a reader has
//! to know before reading a number off it. The five primitives are the arrangement's own and are documented on
//! [`Body`]; the sixth variant, [`Body::Unknown`], is what a document written
//! by a newer writer looks like to an older one, and it is preserved rather
//! than dropped.
//!
//! Two properties the shape has to admit, because they belong to the
//! arrangement rather than to the document: a generator's *code* is opaque but
//! **its output is ordinary tree**, so nothing about being generated makes a
//! subtree a second kind of thing; and a clang may **reference** a generator
//! to fire it live, so the document expresses structure resolved at run time
//! and not only at render time.
//!
//! Nothing derived is stored. The temporal character of a node and the temporal
//! relation of an aggregate are pure functions of what is already there
//! ([`Node::character`], [`Body::relation`]), exactly as they are in the
//! client, so no edit can leave them stale.

pub mod clipboard;
pub mod history;
pub mod intent;
pub mod log;
pub mod resolve;
pub mod selection;
pub mod session;

pub use clipboard::{Clipboard, Content};
pub use history::{History, StructureId};
pub use intent::{Against, Intent, Outcome, Rules, apply};
pub use log::{Entry, Log, MemorySpill, Spill, Step, apply_logged, inverse_of};
pub use resolve::{Mapping, Resolved, Unit, resolve, resolve_node};
pub use selection::{BinRange, Mask, Selection, ValueRange};
pub use session::{Location, OpenEdit, Session, Source};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Beats. The unit of every **onset** here — a member's offset, a node's own
/// onset, the grid an edit snaps to — because a placement is a musical decision
/// and takes the unit of what contains it.
pub type Beats = f64;

/// Seconds. The unit of a **duration** whose seconds were fixed before the
/// document saw them: a take's length is `frames / sample_rate`, a wall-clock
/// fact, and storing it in beats would claim it must be rewritten at every
/// tempo change, which nothing does.
pub type Seconds = f64;

/// Which unit a length is in.
///
/// Not a stored field: the body says it ([`Body::duration_unit`]), so no edit
/// can leave it stale and no writer can disagree with the reader about the
/// number it just wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit {
    /// Beats: the length follows the tempo, which is what a note wants.
    Beats,
    /// Seconds: the length is a wall-clock fact the tempo does not move.
    Seconds,
}

/// Converts a length in **seconds** starting at a given beat into beats.
///
/// The crate does not do this itself, and that is the point. A beat is a
/// logical coordinate, so under a changing tempo the same stretch of seconds
/// reaches a different beat depending on where it starts: the conversion needs
/// the piece's tempo map and a position, and the document transports
/// references without interpreting them. So it takes the onset and the length
/// and hands back the answer.
///
/// A length already in beats never reaches one of these.
pub type SecsToBeats<'a> = &'a dyn Fn(Beats, f64) -> Beats;

/// The conversion for a piece at **one constant tempo** — the only case where
/// a length in seconds is a multiplication.
///
/// Written out so a caller that genuinely has one number says so at the call
/// site, instead of a general converter quietly assuming it.
pub fn at_tempo(tempo: f64) -> impl Fn(Beats, f64) -> Beats {
    move |_at, secs| secs * tempo
}

/// A node's identity within a document. Client-allocated and stable across
/// edits, so an intent and a log entry can both name the same node after the
/// tree around it has moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// A source's identity: the samples a [`SourceRef`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub u64);

/// A leaf's configuration, carried and **never interpreted**.
///
/// This is the whole of what makes one document serve every language: a
/// generator is code, a def is a def, a pattern is a pattern, and the document
/// knows only that something is there and where it sits. A writer that does not
/// understand a payload preserves it — losing a generator's configuration on a
/// round trip through a host that cannot read it would lose the piece.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Opaque(pub serde_json::Value);

impl Opaque {
    /// An empty configuration — a leaf whose author had nothing to say.
    pub fn none() -> Self {
        Self(serde_json::Value::Null)
    }

    /// Whether there is nothing to carry.
    pub fn is_empty(&self) -> bool {
        self.0.is_null()
    }
}

/// How long a source outlives the work that made it.
///
/// The field a save reads. Without it, saving in the middle of a destructive
/// edit writes a reference to a file that is about to be deleted; with it, a
/// save knows what it has to promote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lifetime {
    /// The user's own file. Read-only, never written, never moved.
    External,
    /// Persisted beside the document, and saved with it.
    Session,
    /// A destructive edit's working copy. Dies with the edit session unless a
    /// save promotes it to [`Lifetime::Session`].
    Temporary,
}

/// A half-open range of frames within a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    /// First frame, inclusive.
    pub start: u64,
    /// One past the last frame.
    pub end: u64,
}

impl Range {
    /// Frames covered.
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// Whether the range covers nothing.
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// A reference to samples: which source, how long it lives, which generation
/// of it was last seen, and optionally which part of it.
///
/// The `generation` is the source half of the document's two counters. One
/// number cannot do both jobs: a destructive stroke changes a source's
/// *content* while its identity stays put, which the document's own version
/// cannot express, and a placement edit changes the document while every source
/// is untouched, which a source counter cannot. With the pair, a reader
/// invalidates only what actually moved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    /// The samples this points at.
    pub source: SourceId,
    /// Whether it outlives the session.
    pub lifetime: Lifetime,
    /// The content generation last seen. Bumped by a destructive edit.
    pub generation: u64,
    /// The part of the source used, or the whole of it when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

/// One window of a [`Body::Segments`]: which samples, from which frame, for how
/// long.
///
/// The length is in **seconds**, because these are samples: their seconds were
/// fixed when they were recorded and no tempo change moves them. The frame is
/// the client's own coordinate — the two are bridged by whoever knows the rate,
/// which is never this crate. [`SourceRef::range`] says the same thing in
/// frames alone and is what a writer that knows the frame count uses instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentRef {
    /// The samples this window is onto.
    pub source: SourceRef,
    /// The first frame it reads.
    #[serde(default)]
    pub start: f64,
    /// How long it lasts, in seconds.
    pub duration: Seconds,
}

/// How a [`Body::Aggregate`]'s members relate to each other.
///
/// Named `Grouping` rather than `AggregateKind` because `kind` is the body's own
/// discriminant on the wire, and one word meaning two things in one object is
/// how a format grows a bug nobody can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grouping {
    /// The members relate **in time** — a section holding clips, a melody
    /// holding note clangs. No processing relation.
    Concrete,
    /// The members relate by **processing or generation** — a bus-wired chain
    /// on the server, a generative dependency on the client.
    Logical,
}

/// One placed member of a [`Body::Aggregate`]: an element, and where it sits.
///
/// The offset is relative to the aggregate that holds it, which is what makes the
/// recursion work — a subtree can be moved by moving one number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    /// Start, in beats, relative to the enclosing aggregate.
    pub offset: Beats,
    /// Length in the placed node's own unit ([`Node::duration_unit`]), or the
    /// element's own length when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    /// The placed element.
    pub node: Node,
}

impl Member {
    /// The length this placement shows, in its own unit: what was written on
    /// the placement, else the element's own. `None` when neither says.
    pub fn length(&self) -> Option<f64> {
        self.dur.or(self.node.duration)
    }

    /// The unit [`Member::length`] is in — the placed node's
    /// ([`Node::duration_unit`]).
    pub fn duration_unit(&self) -> TimeUnit {
        self.node.duration_unit()
    }

    /// Where this placement ends, in the aggregate's beats: its offset plus its
    /// length. `None` when it has no length to end at.
    ///
    /// A length in beats is added; a length in seconds goes through
    /// `secs_to_beats` with **this placement's onset**, since that is what the
    /// answer depends on. [`at_tempo`] is the constant-tempo converter for a
    /// caller that has one.
    pub fn end(&self, secs_to_beats: SecsToBeats) -> Option<Beats> {
        self.length().map(|d| match self.duration_unit() {
            TimeUnit::Beats => self.offset + d,
            TimeUnit::Seconds => self.offset + secs_to_beats(self.offset, d),
        })
    }
}

/// What a node **is**.
///
/// The five primitives are the arrangement's, not this crate's invention, and
/// each names a way samples can be organized rather than a widget or a file
/// format:
///
/// - [`Body::Clang`] — parameters or actions that happen **together**. One or
///   more, simultaneous. A punctual clang (no duration) may reference a
///   generator and fire it live.
/// - [`Body::Sequence`] — a **fixed, non-simultaneous** succession. It may
///   contain aggregates, so a sequence of sections is a sequence.
/// - [`Body::Vector`] — a succession of data at **constant rate**.
///   [`Body::Segments`] is the same primitive assembled from several windows,
///   not a sixth kind.
///   Audio or control, and the only body that names samples directly.
/// - [`Body::Aggregate`] — the **recursive container**. Its job is to group elements,
///   of mixed kinds, and it is what a multitrack lane is a restricted
///   projection *of*.
/// - [`Body::Generator`] — a **program that produces** any of the others,
///   generators included: a def, a pattern, a routine. Its code is opaque; what
///   it produces is ordinary tree.
///
/// [`Body::Unknown`] is the sixth and is not a primitive: it is how a body this
/// build does not know survives a round trip intact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Body {
    /// Simultaneous parameters or actions.
    Clang {
        /// The clang itself, in the client's terms.
        #[serde(default, skip_serializing_if = "Opaque::is_empty")]
        config: Opaque,
        /// The generator this clang fires when it happens, if any — the
        /// reference that makes structure resolvable at run time rather than
        /// only at render time.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fires: Option<NodeId>,
    },
    /// A fixed succession, one thing after another.
    Sequence {
        /// The sequence in the client's terms (a list, a pattern).
        #[serde(default, skip_serializing_if = "Opaque::is_empty")]
        config: Opaque,
        /// Sequenced members, when the succession is of document elements
        /// rather than of the client's own values.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<Member>,
    },
    /// Data at constant rate: a vector of samples or control values.
    Vector {
        /// The samples.
        source: SourceRef,
        /// How these samples are meant to sound — a vector is *data*, so what
        /// plays it (an instrument, its controls) is configuration, and
        /// configuration is the client's to interpret.
        #[serde(default, skip_serializing_if = "Opaque::is_empty")]
        config: Opaque,
    },
    /// Data at constant rate, assembled from **several windows**: which source,
    /// from which frame, for how long — read back to back as one thing.
    ///
    /// It is the same primitive [`Body::Vector`] is, over more than one piece
    /// of samples: joining fragments of two files makes one, and cutting one
    /// apart gives back the windows it was made of. Nothing is copied, which is
    /// the whole point — the segments are references, exactly as a vector's own
    /// is.
    Segments {
        /// The samples, in reading order.
        segments: Vec<SegmentRef>,
        /// How it is meant to sound — one configuration for the whole of it,
        /// because what this element *is* is one thing to play.
        #[serde(default, skip_serializing_if = "Opaque::is_empty")]
        config: Opaque,
    },
    /// The recursive container: elements of mixed kinds, placed.
    Aggregate {
        /// Whether the members relate in time or by processing.
        grouping: Grouping,
        /// The placed members.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<Member>,
        /// The writer's own restrictions on this aggregate, carried and **never
        /// interpreted** — the same door a generator's code goes through.
        ///
        /// There is one aggregate kind here and there will go on being one: a
        /// multitrack's track is *an aggregate with the restrictions of a view*, and
        /// putting those restrictions in the tree is what the layer's own rule
        /// refuses ("the tree stays general; a view carries its own
        /// restrictions"). But a writer that has such an aggregate must be able to get
        /// it back, or a round trip through this format silently promotes a
        /// track to a plain aggregate and the piece reopens with a level of nesting
        /// nobody wrote. So the *restriction* travels as opaque configuration,
        /// exactly as a leaf's code does: the document knows something is
        /// there, and not what it means.
        #[serde(default, skip_serializing_if = "Opaque::is_empty")]
        config: Opaque,
    },
    /// A program that produces elements.
    Generator {
        /// The generator's own configuration — code, or a reference to it.
        /// Opaque by construction.
        #[serde(default, skip_serializing_if = "Opaque::is_empty")]
        config: Opaque,
        /// What this generator **last produced**, as ordinary tree.
        ///
        /// The change of state the arrangement already has a verb for: a
        /// generator element becoming a generated one, by being *rendered*.
        /// It is here rather than derived because a host with no language
        /// attached has nothing to derive it with — a generator is code, and
        /// the frozen result is the whole of what such a host can show.
        ///
        /// It is reachable by [`Node::walk`] and [`Node::find`], because a
        /// reader must see it, and **not** by an intent: a rendering is not
        /// the composition, and editing one would write over what the next
        /// render replaces.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rendered: Option<Box<Node>>,
    },
    /// A body this build does not know, preserved whole.
    ///
    /// The forward-compatibility door, and the same rule the widget protocol
    /// already runs on: what cannot be interpreted is carried, not dropped.
    #[serde(untagged)]
    Unknown(serde_json::Value),
}

/// How an aggregate's members relate in time. Derived, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// Duration-only members tiling contiguously.
    Successive,
    /// Every member starts and ends together — the container that can be
    /// reinterpreted, which is what enables the recursion.
    Simultaneous,
    /// Any other combination.
    Mixed,
}

/// What a node's onset and duration make it. Derived, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Character {
    /// Both an onset and a duration.
    Segment,
    /// An onset and no duration — a point in time.
    Punctual,
    /// A duration and no onset — a length waiting to be placed.
    Relative,
    /// Neither: a container that only a parent gives concrete time.
    Abstract,
}

/// An element: temporal metadata over what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Stable identity within the document.
    pub id: NodeId,
    /// A referenceable label, and **not** a second identity.
    ///
    /// The rule is the server's own, taken verbatim rather than invented here
    /// (`docs/schemas.md`, on `/group_new`'s name): the id remains what every
    /// intent addresses and every outcome reports, and the name is a second way
    /// to *refer* to the same node — one the author chooses, that says what the
    /// node is, and that survives being read back. A node is born named or
    /// stays anonymous; an anonymous one is reachable exactly as before,
    /// because nothing addresses by name.
    ///
    /// It is here rather than in a client because a name is what a **view**
    /// labels a lane with, and losing it on a round trip is what makes a
    /// reopened piece anonymous in every writer at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Start in beats relative to its context, when the element itself carries
    /// one. A placed element usually takes its onset from its [`Member`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onset: Option<Beats>,
    /// Length, when known, **in the unit of the body** — seconds for a body
    /// that references samples, beats for one made of events. Read it
    /// through [`Node::duration_unit`] rather than assuming one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Whether the samples are produced by a def running **on the server**
    /// rather than by messages the arrangement flattens. Such an element has no
    /// index: its position *is* its internal state, so a transport can stop it
    /// but cannot locate within it. It becomes locatable by being rendered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resident: bool,
    /// What this node is.
    #[serde(flatten)]
    pub body: Body,
}

impl Node {
    /// A node with neither onset nor duration, and anonymous.
    pub fn new(id: NodeId, body: Body) -> Self {
        Self {
            id,
            name: None,
            onset: None,
            duration: None,
            resident: false,
            body,
        }
    }

    /// The same node, labelled. A name says what the node is; it never says
    /// which node it is (see [`Node::name`]).
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The temporal character, derived from which of onset and duration are
    /// present.
    pub fn character(&self) -> Character {
        match (self.onset.is_some(), self.duration.is_some()) {
            (true, true) => Character::Segment,
            (true, false) => Character::Punctual,
            (false, true) => Character::Relative,
            (false, false) => Character::Abstract,
        }
    }

    /// Whether a position on this element means anything.
    ///
    /// A generated element has an index and can be located within; a resident
    /// generator has none, and the only thing a transport can do to it is stop
    /// it. Pause is symmetric for both; locate is not.
    pub fn locatable(&self) -> bool {
        !self.resident
    }

    /// This node's members, for the bodies that have them.
    pub fn members(&self) -> &[Member] {
        self.body.members()
    }

    /// The unit this node's `duration` — and the `dur` of any placement of it —
    /// is in. See [`Body::duration_unit`].
    pub fn duration_unit(&self) -> TimeUnit {
        self.body.duration_unit()
    }

    /// What a generator last produced, for the one body that has it.
    pub fn rendered(&self) -> Option<&Node> {
        match &self.body {
            Body::Generator { rendered, .. } => rendered.as_deref(),
            _ => None,
        }
    }

    /// Visits this node and every node below it, parents before children —
    /// including a generator's rendered result, which a reader must see.
    pub fn walk(&self, visit: &mut impl FnMut(&Node)) {
        visit(self);
        for member in self.members() {
            member.node.walk(visit);
        }
        if let Some(rendered) = self.rendered() {
            rendered.walk(visit);
        }
    }

    /// The node with this id, at or below here.
    pub fn find(&self, id: NodeId) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        self.members()
            .iter()
            .find_map(|m| m.node.find(id))
            .or_else(|| self.rendered().and_then(|r| r.find(id)))
    }
}

impl Body {
    /// What this body is called on the wire — its `kind` tag, and what a
    /// message about a node says it is.
    pub fn kind(&self) -> &'static str {
        match self {
            Body::Clang { .. } => "clang",
            Body::Sequence { .. } => "sequence",
            Body::Vector { .. } => "vector",
            Body::Segments { .. } => "segments",
            Body::Aggregate { .. } => "aggregate",
            Body::Generator { .. } => "generator",
            Body::Unknown(_) => "unknown",
        }
    }

    /// The unit a length of this body is in.
    ///
    /// **Seconds** for the bodies that reference samples ([`Body::Vector`],
    /// [`Body::Segments`]): their seconds were fixed before the document saw
    /// them, and a tempo change does not shorten a recording. **Beats** for
    /// everything else, where the length is musical and is supposed to follow
    /// the tempo. Derived from the body rather than stored, so a writer cannot
    /// disagree with a reader about the number it just wrote; an unknown body
    /// reads as beats, which is what the rest of the format defaults to.
    pub fn duration_unit(&self) -> TimeUnit {
        match self {
            Body::Vector { .. } | Body::Segments { .. } => TimeUnit::Seconds,
            _ => TimeUnit::Beats,
        }
    }

    /// The placed members, empty for the bodies that hold none.
    pub fn members(&self) -> &[Member] {
        match self {
            Body::Aggregate { members, .. } | Body::Sequence { members, .. } => members,
            _ => &[],
        }
    }

    /// How this body's members relate in time, or `None` for a body that holds
    /// none.
    ///
    /// Derived from the placements alone, so an edit cannot leave it stale.
    /// A single member, or several sharing a start and an end, read as
    /// [`Relation::Simultaneous`]; members that tile contiguously with no gap
    /// read as [`Relation::Successive`]; anything else is
    /// [`Relation::Mixed`].
    ///
    /// `secs_to_beats` is what puts a member's end on the same axis as its
    /// offset: an offset is in beats and a length is in the unit of the data it
    /// measures, so a lane of takes cannot be read against a lane of notes
    /// without it. A body whose members are all measured in beats never calls
    /// it. [`at_tempo`] is the converter for a piece at one constant tempo.
    pub fn relation(&self, secs_to_beats: SecsToBeats) -> Option<Relation> {
        let members = match self {
            Body::Aggregate { members, .. } | Body::Sequence { members, .. } => members,
            _ => return None,
        };
        if members.is_empty() {
            return None;
        }
        let starts: Vec<Beats> = members.iter().map(|m| m.offset).collect();
        let ends: Vec<Option<Beats>> = members.iter().map(|m| m.end(secs_to_beats)).collect();
        if all_close(&starts) && ends.iter().all(Option::is_some) {
            let ends: Vec<Beats> = ends.iter().map(|e| e.unwrap()).collect();
            if all_close(&ends) {
                return Some(Relation::Simultaneous);
            }
        }
        let mut ordered: Vec<(Beats, Option<Beats>)> =
            starts.iter().copied().zip(ends.iter().copied()).collect();
        ordered.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut cursor = ordered[0].0;
        for (start, end) in &ordered {
            let Some(end) = end else {
                return Some(Relation::Mixed);
            };
            if !close(*start, cursor) {
                return Some(Relation::Mixed);
            }
            cursor = *end;
        }
        Some(Relation::Successive)
    }
}

/// A composition, and the version that says which edit produced it.
///
/// The version is the document half of the two counters (the other is each
/// [`SourceRef`]'s `generation`). It is what lets an intent made against a
/// stale picture be reported as stale rather than applied blind — the case a
/// log alone cannot see, because the document can move by routes that are not
/// gestures: a script editing the arrangement, a second editor, a re-render.
///
/// **A version starts at one**, because zero is what an
/// [`intent::Against`] means by *unstated* — the same reservation the GUI
/// host's sequence numbers make, and for the same reason: an unedited document
/// is a real state that an editor must be able to name, so it cannot share a
/// number with "I cannot say".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawDocument")]
pub struct Document {
    /// Monotonic, bumped by every applied edit. Never zero.
    pub version: u64,
    /// The composition.
    pub root: Node,
}

/// What a document looks like on the way in, before [`Document`]'s own rule
/// about ids is checked. Every door into the crate deserializes a `Document`,
/// so putting the check here is what makes it one door rather than a call each
/// caller has to remember.
#[derive(Deserialize)]
struct RawDocument {
    version: u64,
    root: Node,
}

impl TryFrom<RawDocument> for Document {
    type Error = String;

    fn try_from(raw: RawDocument) -> Result<Self, String> {
        let document = Document {
            version: raw.version,
            root: raw.root,
        };
        match document.duplicate_id() {
            Some(message) => Err(message),
            None => Ok(document),
        }
    }
}

/// The version an unedited document carries. Zero is reserved for *unstated*.
pub const FIRST_VERSION: u64 = 1;

impl Document {
    /// A document that has not been edited.
    pub fn new(root: Node) -> Self {
        Self {
            version: FIRST_VERSION,
            root,
        }
    }

    /// The node with this id, anywhere in the tree.
    pub fn find(&self, id: NodeId) -> Option<&Node> {
        self.root.find(id)
    }

    /// The first node id that names two **different** nodes, described so the
    /// message says which two — or `None`, which is what a document must be.
    ///
    /// **An id names one node.** An intent addresses a node by its id, and an
    /// id that names two different things is applied to whichever the lookup
    /// reaches first while the client that sent it keeps the other: one
    /// gesture, two destinations, and on screen the thing the hand moved comes
    /// back to where it was. Nothing downstream can recover from it, so it is
    /// refused at the door — checked every time a document is deserialized,
    /// which is the one place every writer passes through.
    ///
    /// **A repeated id whose nodes are identical is carried, not refused**, and
    /// the line is deliberate. That is one element *placed twice*: the document
    /// is ambiguous — which placement does an intent name? — but it is
    /// consistent, and what an id identifies in that case is an open question
    /// with three answers, one of which is to forbid it. Refusing here would
    /// pick that answer by accident, from inside a check about something else.
    /// A repeated id whose nodes **differ** is not ambiguous but incoherent: no
    /// answer to that question makes it well-formed, because the id names two
    /// different things.
    pub fn duplicate_id(&self) -> Option<String> {
        fn walk<'a>(
            node: &'a Node,
            seen: &mut HashMap<NodeId, &'a Node>,
            clash: &mut Option<String>,
        ) {
            if clash.is_some() {
                return;
            }
            match seen.entry(node.id) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(node);
                }
                std::collections::hash_map::Entry::Occupied(slot) => {
                    let first = *slot.get();
                    if first != node {
                        *clash = Some(format!(
                            "node id {} names two different nodes, a {} and a {}: \
                             ids are unique within a document, because an intent \
                             addresses a node by its id",
                            node.id.0,
                            first.body.kind(),
                            node.body.kind()
                        ));
                        return;
                    }
                }
            }
            for member in node.members() {
                walk(&member.node, seen, clash);
            }
            if let Some(rendered) = node.rendered() {
                walk(rendered, seen, clash);
            }
        }

        let mut seen = HashMap::new();
        let mut clash = None;
        walk(&self.root, &mut seen, &mut clash);
        clash
    }

    /// The highest node id in use, so a client can continue allocating past a
    /// document it did not author.
    pub fn max_id(&self) -> NodeId {
        let mut max = self.root.id;
        self.root.walk(&mut |node| {
            if node.id > max {
                max = node.id;
            }
        });
        max
    }
}

/// Beats compare with a tolerance, since a placement round-trips through the
/// wire's floats and an exact equality would call a simultaneous aggregate mixed.
const EPSILON: Beats = 1e-9;

fn close(a: Beats, b: Beats) -> bool {
    (a - b).abs() <= EPSILON
}

fn all_close(values: &[Beats]) -> bool {
    values.windows(2).all(|w| close(w[0], w[1]))
}

#[cfg(test)]
mod tests;
