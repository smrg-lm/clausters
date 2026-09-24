//! Drawing a document as a multitrack -- the host's own `Editor`.
//!
//! The Python client has one and it is the reference; this is the port a
//! standalone host needs, because a host with no language client still has to
//! show what it holds. It is deliberately the **same picture**: lanes of clips
//! over one shared time axis, a beat ruler under them, ids allocated as it goes
//! and every clip bound to the node it draws.
//!
//! # One container name, three widgets
//!
//! A lane, a clip and a time ruler are all **`field`** on the wire, told apart
//! by the props they carry (`dur` makes a clip, a bare ruler makes a ruler,
//! anything else is a lane) -- the protocol's "generic on the wire, typed in the
//! renderer" invariant, which the Python builders' names hide and this had to
//! learn the hard way: a tree that says `"type": "track"` builds nothing, and
//! an empty window is all it says about it.
//!
//! # It builds a GuiDef, and nothing else
//!
//! What comes out is the ordinary `{id, type, props, children}` tree the host
//! already parses -- no new widget, no new prop, no path into the tree that a
//! script could not take. That is the point: a standalone editor is this host
//! driven by itself, so anything it can draw a script can draw too, and
//! anything it cannot is missing for both.
//!
//! # A clip's body, and the one thing this cannot know
//!
//! An aggregate of pitched clangs draws as a **roll** from the tree alone,
//! because the pitches are in the tree. A **take** cannot: the document names a
//! source and never says where the samples are, so drawing one needs the
//! session's table resolved to something a host can read -- which is
//! [`super::sources`], and is why `Look` takes the resolved [`Takes`] rather
//! than the session. Given none, a take is still drawn: its placement and its
//! name, honest about the rest.

use clausters_document::view::catalogue;
use clausters_document::{Beats, Body, Document, Member, Node, NodeId, TimeUnit};
use serde_json::{Map, Value, json};

use super::sources::Takes;

/// One clip or lane the tree drew, and the node it draws.
///
/// The binding is the whole reason this returns anything besides JSON: an
/// intent names a node, a gesture names a widget, and only what built the tree
/// knows which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    pub widget: i32,
    pub node: NodeId,
}

/// How the picture is scaled and labelled.
#[derive(Debug, Clone, Copy)]
pub struct Look<'a> {
    /// Samples per beat -- what a clip's `offset` is drawn with, since the
    /// shared time axis measures samples and a placement is in beats.
    pub units_per_beat: f64,
    /// Samples per second -- what a clip's `dur` is drawn with when the data
    /// it shows is measured in seconds ([`clausters_document::Body::duration_unit`]:
    /// a take's length is a wall-clock fact, so no tempo scales it).
    pub units_per_second: f64,
    /// The rate the ruler names its ticks in.
    pub sample_rate: f64,
    /// Beats per second, for the ruler's bar and beat lines.
    pub tempo: f64,
    /// The grid a placement snaps to, in beats; 0 snaps nothing.
    pub quant: Beats,
    /// The first widget id to allocate. Ids are the host's own namespace, and a
    /// caller that already used some says where to carry on from.
    ///
    /// **It must clear the GuiDef's own id**, because a def's id is its root
    /// widget's: a tree numbered from 1 handed to `/gui_def 1` collides with
    /// itself, and the registry drops the whole subtree -- which looks like an
    /// empty window and one line in the log.
    pub first_id: i32,
    /// The document's **content**: the nodes a window names
    /// ([`clausters_document::SegmentSource::Node`]).
    ///
    /// A cut over notes leaves two windows onto one timeline, and the timeline
    /// is here rather than in either of them -- so without this a split roll clip
    /// draws as an empty rectangle, which is the picture for *the samples are
    /// elsewhere* and a lie about notes the document is holding.
    pub content: Option<&'a [Node]>,
    /// The session's samples, once somebody has resolved it to buffers.
    ///
    /// `None` -- or a source missing from it -- draws a take as its placement and
    /// its name, which is what a host with no server can honestly show: the
    /// document holds no samples, so an empty clip here means *the samples are
    /// elsewhere*, not that there is none.
    pub takes: Option<&'a Takes>,
}

impl Default for Look<'_> {
    fn default() -> Self {
        Self {
            units_per_beat: 48_000.0,
            units_per_second: 48_000.0,
            sample_rate: 48_000.0,
            tempo: 1.0,
            quant: 0.0,
            first_id: 1,
            content: None,
            takes: None,
        }
    }
}

/// The window a document draws as, plus what it drew it from.
pub struct Drawn {
    /// The GuiDef, ready for `/gui_def`.
    pub def: Value,
    /// The `multitrack` widget's id -- the **one** widget the whole multitrack is
    /// drawn by, and the one an edit-back arrives on.
    pub widget: i32,
    /// The multitrack as the widget's two props, and the nodes behind them.
    pub picture: Picture,
    /// Every take editor's widget, and the node it draws.
    ///
    /// Only the editors: a clip is not a widget any more, so there is nothing
    /// per clip to bind. What is left here is the pane under the stack, which
    /// *is* its own widget because it is a different view of the same node.
    pub bindings: Vec<Bound>,
    /// The next free widget id, so a caller can keep allocating after it.
    pub next_id: i32,
}

/// One lane of the multitrack, and the two things a lane change has to know
/// about it.
///
/// A lane is not a container in the document -- the document has aggregates --
/// so a picture of one has to record which aggregate its clips are *members
/// of*, and where that aggregate sits on the shared axis. Without the first, a
/// clip that crossed the stack has nowhere to be moved to; without the second,
/// it arrives at the right pixel and the wrong beat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneRow {
    /// The node the lane draws. Its **identity on the wire is its number**, as
    /// a string, which is what makes a `"clips"` payload readable with no map
    /// on the side: a name is a node id and nothing else.
    pub node: NodeId,
    /// The aggregate whose members are this lane's clips.
    pub holder: NodeId,
    /// Where that aggregate starts, in beats -- the offsets its members are
    /// relative to.
    pub base: Beats,
}

/// One clip, and the lane it was drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipRow {
    /// The node the box draws; its name on the wire is this number.
    pub node: NodeId,
    /// The lane it sits on ([`LaneRow::node`]).
    pub lane: NodeId,
}

/// The multitrack as the `multitrack` widget takes it, and as the owner reads it
/// back.
///
/// Both halves come out of one walk, because they have to agree: the props are
/// what the hand moves and the rows are what an edit-back is resolved against,
/// and deriving them separately is how a name comes to mean two nodes.
#[derive(Debug, Clone, Default)]
pub struct Picture {
    /// The lanes, top to bottom.
    pub lanes: Vec<LaneRow>,
    /// The clips, in document order.
    pub clips: Vec<ClipRow>,
    /// **The picture, as props** -- every key the widget draws from, exactly as
    /// the projection wrote it.
    ///
    /// A map rather than the two fields this was, and the difference is the
    /// whole of a defect: the projection produces **seven** props (`lanes`,
    /// `clips`, `curves`, `layers`, `points`, `hidden`, `loops`) and the host
    /// kept two, so in a host with no client attached a curve was never drawn
    /// and a curve the multitrack *minted* -- the `A` toggle's whole purpose --
    /// reached the widget as nothing at all. Whatever the projection says is
    /// what gets drawn, and a key it grows arrives here without anyone
    /// remembering to pass it along.
    pub props: Map<String, Value>,
}

impl Picture {
    /// The lane of this node, if the multitrack has one.
    pub fn lane(&self, node: NodeId) -> Option<&LaneRow> {
        self.lanes.iter().find(|l| l.node == node)
    }

    /// The lane a clip is drawn on, if the multitrack holds it.
    pub fn lane_of(&self, clip: NodeId) -> Option<&LaneRow> {
        let row = self.clips.iter().find(|c| c.node == clip)?;
        self.lane(row.lane)
    }
}

/// A node's name on the wire: its number, which is its identity.
fn name_of(node: NodeId) -> String {
    node.0.to_string()
}

/// The node a name on the wire stands for. A name this host did not write is
/// `None` rather than a guess -- the same rule [`super::Owner::read_event`]
/// follows for a payload it does not recognize.
pub(crate) fn node_named(name: &str) -> Option<NodeId> {
    name.parse::<u64>().ok().map(NodeId)
}

/// The lane height a lane is drawn at, in logical pixels.
const LANE_H: f64 = 96.0;

/// The lanes and clips a document draws as -- the whole multitrack, in the widget's
/// own vocabulary.
///
/// One lane per top-level member: an **aggregate** becomes a lane of its
/// members' clips (which is what a track is), and anything else becomes a lane
/// holding one clip. Nesting deeper than that is drawn flat for now -- an
/// aggregate inside an aggregate is one lane of its own, in document order --
/// because an expanded/collapsed state is a thing the *editor* holds and this
/// has nowhere yet to keep one.
pub fn multitrack(document: &Document, look: &Look<'_>) -> Picture {
    let look = &Look {
        content: Some(&document.content),
        ..*look
    };
    let mut built: Vec<LaneBuild> = Vec::new();
    match &document.root.body {
        Body::Aggregate { members, .. } => {
            for member in members {
                lane_of(member, document.root.id, 0.0, look, &mut built);
            }
        }
        // A document that is one thing is one lane holding it.
        _ => {
            let member = Member {
                offset: 0.0,
                dur: None,
                node: document.root.clone(),
            };
            lane_of(&member, document.root.id, 0.0, look, &mut built);
        }
    }
    let mut lanes = Vec::with_capacity(built.len());
    let mut clips = Vec::new();
    let mut lanes_prop = Vec::with_capacity(built.len() * 7);
    let mut clips_prop = Vec::new();
    for lane in built {
        lanes.push(LaneRow {
            node: lane.node,
            holder: lane.holder,
            base: lane.base,
        });
        lanes_prop.extend([
            json!(name_of(lane.node)),
            json!(lane.label),
            json!(LANE_H),
            json!(lane.mute),
            json!(lane.solo),
            json!(lane.gain),
            // **A tree's lane has no automation rows under it**, so the toggle
            // is not offered at all and what this says is never drawn. Shown is
            // the default a row carries when nothing hid it.
            json!(true),
        ]);
        for clip in lane.clips {
            clips.push(ClipRow {
                node: clip.node,
                lane: lane.node,
            });
            clips_prop.extend([
                json!(name_of(clip.node)),
                json!(name_of(lane.node)),
                json!(clip.offset),
                json!(clip.dur),
                json!(clip.start),
                json!(clip.label),
                json!(clip.source),
            ]);
        }
    }
    // The tree's own description has rows and boxes and nothing else -- no
    // automation hangs on it -- so its picture is the two props the multitrack's
    // projection starts from.
    let mut props = Map::new();
    props.insert("lanes".into(), Value::Array(lanes_prop));
    props.insert("clips".into(), Value::Array(clips_prop));
    Picture {
        lanes,
        clips,
        props,
    }
}

/// Draws `document` as a window: **one `multitrack`**, and the take editors
/// under it.
///
/// One widget for the whole multitrack rather than a lane per track and a clip per
/// element, because the multitrack is what a gesture reports: a block move and a
/// lane change say *the clips are now these*, and nothing has to say which of
/// them the hand touched. It is the same shape a `pianoroll` has always had,
/// and the reason this driver had a bug the roll never could.
///
/// It draws a document written **before the turn**, whose description is the
/// tree. A multitrack opens in the multitrack editor's own window instead, which is
/// the applications crate's (`clausters_apps::multitrack::window`).
pub fn draw(document: &Document, look: &Look<'_>, title: &str) -> Drawn {
    let mut ids = Ids {
        next: look.first_id,
    };
    let mut bindings = Vec::new();
    let look = &Look {
        content: Some(&document.content),
        ..*look
    };
    let picture = multitrack(document, look);
    let widget = ids.take();
    let mut props = Map::new();
    props.insert("id".into(), json!(widget));
    props.insert("type".into(), json!("multitrack"));
    props.insert("weight".into(), json!(1.0));
    props.insert("ruler".into(), json!("beats"));
    props.insert("sample_rate".into(), json!(look.sample_rate));
    props.insert("tempo".into(), json!(look.tempo));
    if look.quant > 0.0 {
        props.insert("snap".into(), json!(look.quant * look.units_per_beat));
    }
    // **The window is the reader's.** A session host is an editor, and in an
    // editor a content change is mostly the reader's own edit -- undoing a
    // trim, splitting a clip, dragging one onto another lane -- so the axis
    // does not re-frame itself on one. The extent is still registered; only the
    // window stays put.
    props.insert("autofit".into(), json!(false));
    // **The head is anchored at 0, and that is the whole of drawing it.** A
    // session's clock is the *transport's position* rather than the device's
    // (`HeadClock::Transport`), so the sweep from an anchor of 0 is the position
    // itself: it stands still while the transport is stopped, jumps where a
    // locate puts it and wraps where the engine wraps it. Without the anchor
    // there is no line at all -- which is what a multitrack that plays with
    // nothing moving on screen looks like.
    props.insert("playhead_at".into(), json!(0.0));
    // **Every prop the picture has**, not the two this used to name: a session
    // with automation opened with no curve drawn at all, because the def was
    // written from a hand-listed pair while the projection had five more.
    for (key, value) in &picture.props {
        props.insert(key.clone(), value.clone());
    }

    let mut children = vec![Value::Object(props)];
    children.extend(take_editors(document, look, &mut ids, &mut bindings));

    let def = json!({
        "type": "window",
        "title": title,
        "layout": "col",
        "w": 1000,
        "h": 640,
        "children": children,
    });
    Drawn {
        def,
        widget,
        picture,
        bindings,
        next_id: ids.next,
    }
}

/// The height one channel's row gets in a take's editor, in logical pixels:
/// enough for a trace to have a shape, and small enough that the tracks above
/// stay the picture.
const EDITOR_ROW_H: f64 = 120.0;

/// The tallest a take's editor is built, however many channels it holds. Past
/// this the lanes get thinner instead -- a pane taller than the window would
/// push the arrangement off the screen, which is worse than a cramped lane.
const EDITOR_MAX_H: f64 = 360.0;

/// **The pane a take opens in is as tall as the take is wide**: samples with
/// four channels is four rows, and a view that shows two of them is a picture
/// of half the file -- the same argument that makes a clip draw every channel.
///
/// It is declared **here, by what built the tree**, and not asked of the widget:
/// a natural size that followed its data would relayout the window whenever a
/// `/gui_set` landed, so the rule is that the driver knows the shape and says
/// so. This driver reads it from the session's own source table, which is where
/// a file's channel count is written down before anything is drawn.
///
/// It stops growing at [`EDITOR_MAX_H`], and what that costs is stated rather
/// than hidden: an ambisonic take's sixteen rows are drawn, at sixteenths of
/// that height. Making many channels *readable* -- scrolling the pane, folding
/// rows, choosing which to show -- is a design this does not have yet
/// (`clients/gui/PLAN.md`, "Future directions").
fn editor_height(channels: Option<u32>) -> f64 {
    let rows = channels.unwrap_or(1).max(1) as f64;
    (EDITOR_ROW_H * rows).min(EDITOR_MAX_H)
}

struct Ids {
    next: i32,
}

impl Ids {
    fn take(&mut self) -> i32 {
        let id = self.next;
        self.next += 1;
        id
    }
}

/// One lane under construction: the row, plus what the props need.
struct LaneBuild {
    node: NodeId,
    holder: NodeId,
    base: Beats,
    label: String,
    mute: bool,
    solo: bool,
    gain: f64,
    clips: Vec<ClipBuild>,
}

/// One box under construction, in **timeline units** -- the axis' own -- because
/// that is what the widget takes.
struct ClipBuild {
    node: NodeId,
    offset: f64,
    dur: f64,
    start: f64,
    label: String,
    source: i32,
}

/// Turns one member into lanes, **recursing while it is aggregates all the way
/// down**.
///
/// The rule is the shape of the multitrack rather than a depth: an aggregate whose
/// members are leaves is a lane of clips (that is what a lane *is*), and an
/// aggregate of aggregates is not one lane but each of theirs. A multitrack is
/// nested as deeply as the author nested it -- a multitrack of aggregates of tracks
/// of clangs is three deep before a single note is reached -- so anything that
/// stops at a fixed depth draws the containers and calls it a picture, which is
/// an empty clip where the music was.
///
/// `base` accumulates the offsets on the way down, because a clip's offset is
/// absolute on the shared axis while a member's is relative to its aggregate;
/// `parent` is the aggregate `member` is a member of, which is where a clip
/// this lane holds is removed from when it crosses to another lane.
fn lane_of(
    member: &Member,
    parent: NodeId,
    base: Beats,
    look: &Look<'_>,
    out: &mut Vec<LaneBuild>,
) {
    let here = base + member.offset;
    if let Body::Aggregate { members, .. } = &member.node.body
        && members
            .iter()
            .any(|inner| matches!(inner.node.body, Body::Aggregate { .. }))
    {
        for inner in members {
            lane_of(inner, member.node.id, here, look, out);
        }
        return;
    }
    let (mute, solo, gain) = mixing_of(&member.node);
    // Which aggregate holds this lane's clips, and what their offsets are
    // relative to. A lane of clips holds its own; a lane that *is* one element
    // borrows its parent's, because that is where the element is a member.
    let (holder, clip_base, clips) = match &member.node.body {
        // **An aggregate of clangs is one clip**, not a lane of clips: that is
        // what a track *is* in every editor, and drawing each note as its own
        // box gives a row of empty rectangles where the music was.
        Body::Aggregate { members, .. } if !members.is_empty() && notes_of(members).is_some() => {
            (parent, base, vec![clip_of(member, base, look)])
        }
        // A **window onto a timeline**: what a cut over notes leaves.
        Body::Segments { .. } if windowed_notes(&member.node, look).is_some() => {
            (parent, base, vec![clip_of(member, base, look)])
        }
        Body::Aggregate { members, .. } => (
            member.node.id,
            here,
            members.iter().map(|i| clip_of(i, here, look)).collect(),
        ),
        _ => (parent, base, vec![clip_of(member, base, look)]),
    };
    out.push(LaneBuild {
        node: member.node.id,
        holder,
        base: clip_base,
        label: label_of(&member.node),
        mute,
        solo,
        gain,
        clips,
    });
}

/// One box: where it sits on the shared axis, and the buffer it is a window
/// onto.
fn clip_of(member: &Member, base: Beats, look: &Look<'_>) -> ClipBuild {
    ClipBuild {
        node: member.node.id,
        offset: (base + member.offset) * look.units_per_beat,
        dur: clip_units(member, look.takes, look),
        // What frame of the source the box's own zero reads. The document keeps
        // it in the source reference's range, and a member that names none
        // starts at the beginning of what it names.
        start: start_of(&member.node),
        label: label_of(&member.node),
        // The samples, as a **server buffer**: the clip's body is drawn from
        // it, which is the same buffer an edit writes. One copy, not a picture
        // of one and a write to another. A negative number is a box over
        // nothing -- and negative rather than zero, because buffer 0 is a
        // buffer.
        source: take_of(&member.node, look).map_or(-1, |t| t.bufnum),
    }
}

/// The source frame a node's own zero reads.
fn start_of(node: &Node) -> f64 {
    match &node.body {
        Body::Vector { source, .. } => source.range.map_or(0.0, |r| r.start as f64),
        Body::Segments { segments, .. } => segments.first().map_or(0.0, |s| s.start),
        _ => 0.0,
    }
}

fn take_editors(
    document: &Document,
    look: &Look<'_>,
    ids: &mut Ids,
    bindings: &mut Vec<Bound>,
) -> Vec<Value> {
    let Some(takes) = look.takes else {
        return Vec::new();
    };
    let mut seen: Vec<clausters_document::SourceId> = Vec::new();
    let mut out = Vec::new();
    document.walk(&mut |node| {
        // One pane per **source**, and assembled samples names several: a
        // joined clip is edited multitrack by multitrack, since a multitrack is what a file
        // is.
        let sources: Vec<clausters_document::SourceId> = match &node.body {
            Body::Vector { source, .. } => vec![source.source],
            Body::Segments { segments, .. } => segments
                .iter()
                .filter_map(|s| s.source.samples())
                .map(|source| source.source)
                .collect(),
            _ => return,
        };
        for source in sources {
            if seen.contains(&source) {
                continue;
            }
            let Some(take) = takes.get(source) else {
                continue;
            };
            seen.push(source);
            let widget = ids.take();
            bindings.push(Bound {
                widget,
                node: node.id,
            });
            // **The picture is the catalogue's**, not this module's: which
            // widget a take draws as, and the three-gesture plan it offers, are
            // one rule the clients draw by too
            // (`clausters_document::view::catalogue`). What is stated here is
            // only what is this pane's -- where it sits in the stack, and that
            // its ruler counts frames because it is beside a document that does.
            //
            // **The head is anchored at 0, and that is the whole of drawing it.**
            // A session's clock is the *transport's position* rather than the device's
            // (`HeadClock::Transport`), so the sweep from an anchor of 0 is the
            // position itself: it stands still while the transport is stopped,
            // jumps where a locate puts it and wraps where the engine wraps it.
            // No `playhead_loop` here for the same reason -- the loop is the
            // transport's, and wrapping an already-wrapped number would double it.
            let mut props = catalogue::waveform(&catalogue::Waveform {
                buffer: Some(i64::from(take.bufnum)),
                channels: take.channels,
                ruler: "samples".into(),
                sample_rate: look.sample_rate,
                label: label_of(node),
                height: Some(editor_height(take.channels)),
                playhead_at: Some(0.0),
                // The position cursor from the start, where a play with
                // nothing placed begins -- the play cursor stands on it until
                // something plays.
                cursor: Some(0.0),
                ..catalogue::Waveform::default()
            });
            props.insert("id".into(), json!(widget));
            out.push(Value::Object(props));
        }
    });
    out
}

/// The samples a node draws, when it names some and somebody resolved it.
/// The length a clip is drawn at, **in timeline units**: the placement's where
/// it overrides, else the element's own, else **the samples'** -- a take placed
/// 1:1 is as long as it is, which is the one length nobody has to state -- else
/// a beat, because a clip with no length at all would be a line.
///
/// The unit conversion is part of the rule rather than the caller's: a length
/// is in the unit of its own data ([`clausters_document::Body::duration_unit`]),
/// so a take's seconds meet the axis through the rate and a phrase's beats
/// through the tempo. Handing a number back without saying which it was is how
/// the two get multiplied by the wrong ratio.
///
/// One rule, in one place. The draw asks it, and so does the adoption of an
/// applied edit ([`super::super::Host::adopt`]): a placement whose length went
/// back to *unstated* has to be redrawn at whatever that means here, and an
/// adopter with a shorter rule of its own left the clip at the size the hand
/// had given it -- which is an undo that moves the document and not the picture.
pub(crate) fn clip_units(
    member: &Member,
    takes: Option<&super::sources::Takes>,
    look: &Look<'_>,
) -> f64 {
    if let Some(dur) = member.length().filter(|d| *d > 0.0) {
        return match member.duration_unit() {
            TimeUnit::Seconds => dur * look.units_per_second,
            TimeUnit::Beats => dur * look.units_per_beat,
        };
    }
    // The samples' own length is already in frames, which is what the axis
    // counts.
    takes
        .and_then(|t| match &member.node.body {
            Body::Vector { source, .. } => t.get(source.source),
            _ => None,
        })
        .and_then(|t| t.frames)
        .map(|f| f as f64)
        .filter(|d| *d > 0.0)
        .unwrap_or(look.units_per_beat)
}

/// The samples a box is a window onto, when it is a window onto exactly one.
///
/// A **take** is one buffer, and so is a clip left by a *trim*: a single window
/// onto one file, which is what a cut leaves on each side. A clip assembled
/// from several windows is drawn empty rather than as its first multitrack -- an
/// honest "the samples are elsewhere" instead of a picture of a third of them
/// (the widget draws one body per box; see `clients/gui/PLAN.md`, "Found by
/// use").
fn take_of(node: &Node, look: &Look<'_>) -> Option<super::sources::Take> {
    let source = match &node.body {
        Body::Vector { source, .. } => source.source,
        Body::Segments { segments, .. } => match segments.as_slice() {
            [one] => one.source.samples()?.source,
            _ => return None,
        },
        _ => return None,
    };
    look.takes?.get(source)
}

/// The notes a **window onto content** shows, placed from the window's own zero.
///
/// A cut over notes is two windows onto one timeline: the timeline is a node in
/// [`Document::content`] and each half names it. What a half draws is the notes
/// inside its window, shifted back to its own start -- the same reading the
/// clients do, made here so a host with no client draws a split multitrack the way
/// the multitrack is.
fn windowed_notes(node: &Node, look: &Look<'_>) -> Option<Vec<(Beats, Beats, f32)>> {
    let Body::Segments { segments, .. } = &node.body else {
        return None;
    };
    // **Every window, back to back**: one is what a cut leaves, several is what
    // a join across timelines makes, and the run is read the same way either
    // time -- each window's notes placed from where that window sits in it.
    let mut out = Vec::new();
    let mut cursor = 0.0;
    for window in segments {
        let named = window.source.node()?;
        let held = look.content?.iter().find(|n| n.id == named)?;
        let Body::Aggregate { members, .. } = &held.body else {
            return None;
        };
        let notes = notes_of(members)?;
        let (from, to) = (window.start, window.start + window.duration);
        out.extend(
            notes
                .into_iter()
                .filter(|(start, _, _)| *start >= from && *start < to)
                .map(|(start, dur, pitch)| (cursor + start - from, dur, pitch)),
        );
        cursor += window.duration;
    }
    Some(out)
}

/// The `notes` body of an aggregate of clangs -- the flat `start dur pitch velocity
/// channel` quintuples the roll reads, in **beats** here and scaled by the
/// caller.
///
/// `None` unless *every* member is a clang carrying a pitch: an aggregate holding
/// takes, generators or anything else is a lane of clips and not a roll, and
/// half a roll would be a picture that leaves samples out without saying so.
fn notes_of(members: &[Member]) -> Option<Vec<(Beats, Beats, f32)>> {
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        let Body::Clang { config, .. } = &m.node.body else {
            return None;
        };
        // The configuration is the client's own opaque object: this reads two
        // keys out of it and understands nothing else, which is the rule the
        // document is built on -- a host does not interpret a leaf's config, it
        // only draws what it can recognize.
        let pitch = config
            .0
            .get("midinote")
            .and_then(Value::as_f64)
            .or_else(|| config.0.get("note").and_then(Value::as_f64))?;
        // The sounding length: the clang's own `dur`, the placement's, or a
        // beat -- a note with no length would be a line.
        let dur = m
            .dur
            .or(m.node.duration)
            .or_else(|| config.0.get("dur").and_then(Value::as_f64))
            .filter(|d| *d > 0.0)
            .unwrap_or(1.0);
        out.push((m.offset, dur, pitch as f32));
    }
    (!out.is_empty()).then_some(out)
}

/// The mixing a lane's strip draws, as the node carries it: muted, soloed and
/// its fader.
///
/// The keys are the clients' own (`clausters.form.document`'s `MIXING`), which
/// is what makes a multitrack muted in a script open muted here. A lane whose
/// configuration says nothing is not muted and is at unity -- the widget's props
/// are three values and not three optional ones, so *absent* is drawn as the
/// default rather than left unsaid.
///
/// `Owner::read_lanes` writes the same three keys back, which is what makes a
/// strip pressed here a fact about the multitrack rather than about the window.
fn mixing_of(node: &Node) -> (bool, bool, f64) {
    let table = node
        .body
        .config()
        .and_then(|c| c.0.as_object().cloned())
        .unwrap_or_default();
    let flag = |key: &str| table.get(key).and_then(Value::as_bool).unwrap_or(false);
    let level = table.get("level").and_then(Value::as_f64).unwrap_or(1.0);
    (flag("mute"), flag("solo"), level)
}

fn label_of(node: &Node) -> String {
    match &node.body {
        Body::Clang { .. } => format!("clang {}", node.id.0),
        Body::Sequence { .. } => format!("sequence {}", node.id.0),
        Body::Vector { .. } => format!("take {}", node.id.0),
        Body::Segments { segments, .. } => format!("take {} ({})", node.id.0, segments.len()),
        Body::Aggregate { grouping, .. } => format!("{grouping:?} {}", node.id.0).to_lowercase(),
        Body::Generator { .. } => format!("generator {}", node.id.0),
        // A body this build does not know: drawn as what it is rather than
        // refused, which is the same courtesy an older host shows a newer
        // widget. A document written by something ahead of us still opens, and
        // what it holds is still moved, undone and saved unchanged.
        Body::Unknown(_) => format!("node {}", node.id.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::{Grouping, Opaque};

    fn clang(id: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Clang {
                config: Opaque::default(),
                fires: None,
            },
        )
    }

    fn placed(offset: Beats, dur: Option<Beats>, node: Node) -> Member {
        Member { offset, dur, node }
    }

    fn aggregate(id: u64, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members,
                config: Opaque::none(),
            },
        )
    }

    /// The `multitrack` the window's first child is.
    fn view(def: &Value) -> &Value {
        &def["children"][0]
    }

    /// The flat props, back as groups -- the reading every payload of this
    /// shape gets.
    fn groups(prop: &Value, n: usize) -> Vec<Vec<Value>> {
        let flat = prop.as_array().expect("a flat array");
        flat.chunks_exact(n).map(<[Value]>::to_vec).collect()
    }

    fn lanes(def: &Value) -> Vec<Vec<Value>> {
        groups(&view(def)["lanes"], 7)
    }

    fn clips(def: &Value) -> Vec<Vec<Value>> {
        groups(&view(def)["clips"], 7)
    }

    /// **One widget for the whole multitrack**, and the lanes are its data. The
    /// milestone's shape, and the thing the old tree could not say: with the
    /// stack spread over N widgets there was nowhere to report *the
    /// arrangement*, so a gesture reported what the hand did.
    #[test]
    fn a_document_draws_as_one_multitrack_holding_every_lane() {
        let doc = Document::new(aggregate(
            1,
            vec![
                placed(
                    0.0,
                    None,
                    aggregate(2, vec![placed(0.0, Some(2.0), clang(3))]),
                ),
                placed(
                    0.0,
                    None,
                    aggregate(4, vec![placed(1.0, Some(1.0), clang(5))]),
                ),
            ],
        ));
        let drawn = draw(&doc, &Look::default(), "session");
        let kids = drawn.def["children"].as_array().expect("children");
        assert_eq!(kids.len(), 1, "one widget, not a lane each and a ruler");
        assert_eq!(view(&drawn.def)["type"], "multitrack");
        assert_eq!(view(&drawn.def)["id"], drawn.widget);
        // It draws its own ruler, which is why there is no strip beside it.
        assert_eq!(view(&drawn.def)["ruler"], "beats");
        assert_eq!(lanes(&drawn.def).len(), 2, "one lane per track");
        assert_eq!(clips(&drawn.def).len(), 2, "one clip each");
    }

    /// **A name on the wire is a node id**, which is what lets an edit-back be
    /// read with no map on the side: the payload says which node moved because
    /// that is what it calls it.
    #[test]
    fn every_lane_and_clip_is_named_by_the_node_it_draws() {
        let doc = Document::new(aggregate(
            1,
            vec![placed(
                0.0,
                None,
                aggregate(
                    2,
                    vec![placed(0.0, None, clang(7)), placed(1.0, None, clang(8))],
                ),
            )],
        ));
        let drawn = draw(&doc, &Look::default(), "session");
        assert_eq!(lanes(&drawn.def)[0][0], "2", "the lane is the track");
        let boxes = clips(&drawn.def);
        let names: Vec<&str> = boxes
            .iter()
            .map(|c| c[0].as_str().expect("a name"))
            .collect();
        assert_eq!(names, vec!["7", "8"], "the clips, in document order");
        let on: Vec<&str> = boxes
            .iter()
            .map(|c| c[1].as_str().expect("a lane"))
            .collect();
        assert_eq!(on, vec!["2", "2"], "both on the lane that holds them");

        // ...and the rows say the same thing to the owner, which is what an
        // edit-back is resolved against.
        assert_eq!(
            drawn
                .picture
                .clips
                .iter()
                .map(|c| c.node)
                .collect::<Vec<_>>(),
            vec![NodeId(7), NodeId(8)]
        );
        assert_eq!(
            drawn.picture.lane_of(NodeId(7)).map(|l| l.holder),
            Some(NodeId(2))
        );
    }

    /// A clip's offset is **absolute on the shared axis** while a member's is
    /// relative to its aggregate: the two are added once, here, or every lane
    /// after the first would draw in the wrong place -- and the lane records
    /// what they were added *to*, so an edit-back can subtract it again.
    #[test]
    fn a_nested_placement_is_absolute_on_the_shared_axis() {
        let doc = Document::new(aggregate(
            1,
            vec![placed(
                4.0, // the lane starts at beat 4
                None,
                aggregate(2, vec![placed(1.0, Some(2.0), clang(3))]), // the clip at 1 within it
            )],
        ));
        let look = Look {
            units_per_beat: 100.0,
            ..Look::default()
        };
        let drawn = draw(&doc, &look, "session");
        let clip = &clips(&drawn.def)[0];
        assert_eq!(clip[2], 500.0, "4 + 1 beats, in units");
        assert_eq!(clip[3], 200.0);
        assert_eq!(
            drawn.picture.lanes[0].base, 4.0,
            "and the lane says what the offset was measured from"
        );
    }

    /// A document that is one thing is still a window: a host handed a file
    /// draws what it was given rather than refusing a shape.
    #[test]
    fn a_document_that_is_not_an_aggregate_still_draws() {
        let doc = Document::new(clang(1));
        let drawn = draw(&doc, &Look::default(), "one thing");
        assert_eq!(lanes(&drawn.def).len(), 1);
        assert_eq!(clips(&drawn.def).len(), 1);
        assert_eq!(drawn.picture.clips[0].node, NodeId(1));
    }

    /// **A multitrack muted in a client opens muted here.** A node's configuration
    /// was already carried across a save; what was missing was reading it.
    #[test]
    fn a_lane_draws_the_mixing_its_element_carries() {
        let mut track = aggregate(2, vec![placed(0.0, None, clang(3))]);
        if let Body::Aggregate { config, .. } = &mut track.body {
            *config = Opaque(serde_json::json!({"mute": true, "level": 0.25}));
        }
        let doc = Document::new(aggregate(1, vec![placed(0.0, None, track)]));
        let drawn = draw(&doc, &Look::default(), "session");
        let lane = &lanes(&drawn.def)[0];
        assert_eq!(lane[3], true, "muted");
        assert_eq!(lane[4], false, "and not soloed");
        assert_eq!(lane[5], 0.25, "at the fader the multitrack carries");
    }

    /// A lane with no strip in its configuration is not muted and is at unity:
    /// the widget's props are three values, not three optional ones.
    #[test]
    fn a_lane_with_no_mixing_is_audible_at_unity() {
        let doc = Document::new(aggregate(
            1,
            vec![placed(
                0.0,
                None,
                aggregate(2, vec![placed(0.0, None, clang(3))]),
            )],
        ));
        let drawn = draw(&doc, &Look::default(), "session");
        let lane = &lanes(&drawn.def)[0];
        assert_eq!(
            (&lane[3], &lane[4], &lane[5]),
            (&json!(false), &json!(false), &json!(1.0))
        );
    }

    #[test]
    fn a_grid_reaches_the_view_and_nothing_reaches_it_when_there_is_none() {
        let doc = Document::new(aggregate(1, vec![placed(0.0, None, clang(2))]));
        let plain = draw(&doc, &Look::default(), "t");
        assert!(view(&plain.def).get("snap").is_none());
        let snapped = draw(
            &doc,
            &Look {
                quant: 0.5,
                units_per_beat: 100.0,
                ..Look::default()
            },
            "t",
        );
        assert_eq!(view(&snapped.def)["snap"], 50.0);
    }
}

#[cfg(test)]
mod take_tests {
    /// The host's spaces with the first `held` buffers already taken, so a
    /// load's numbers start at `held`.
    fn ids_past(held: usize) -> clausters_core::ids::IdSpaces {
        use clausters_core::ids::{IdShare, IdSpaces, ServerShape, Space};
        let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
        ids.alloc(Space::Buffers, held).expect("buffers");
        ids
    }

    use super::*;
    use crate::host::document::sources;
    use clausters_document::session::{Session, Source};
    use clausters_document::{Grouping, Lifetime, Opaque, SourceId, SourceRef};

    fn take(id: u64, source: u64) -> Node {
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

    fn one_take(source: u64) -> Document {
        Document::new(Node::new(
            NodeId(1),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members: vec![Member {
                    offset: 0.0,
                    dur: None,
                    node: take(2, source),
                }],
                config: Opaque::none(),
            },
        ))
    }

    /// The first clip's septuple.
    fn clip(def: &Value) -> Vec<Value> {
        def["children"][0]["clips"].as_array().expect("clips")[..7].to_vec()
    }

    /// A resolved take draws the **buffer**, which is the same samples an edit
    /// writes: the picture and the samples are one thing, or the host is
    /// showing one copy and editing another.
    #[test]
    fn a_resolved_take_draws_its_buffer_and_is_as_long_as_its_samples() {
        let dir = std::env::temp_dir().join(format!("clausters_gui_take_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("t.wav"), b"there").expect("write");
        let session = Session::new(one_take(3)).with_source(
            SourceId(3),
            // Two seconds at the drawing's own rate: 96000 frames.
            Source::file("t.wav", Lifetime::Session).shaped(2, 96_000, 48_000.0),
        );
        let load = sources::plan(&session, &dir, &mut ids_past(7));
        let look = Look {
            takes: Some(&load.takes),
            ..Look::default()
        };
        let drawn = draw(&session.document, &look, "take");
        let clip = clip(&drawn.def);
        assert_eq!(clip[6], 7, "the buffer it was read into");
        assert_eq!(
            clip[3], 96_000.0,
            "as long as the samples, in timeline units"
        );
        // Its own pane opens with both cursors, as a client's audio editor
        // does: the position cursor placed and the play cursor anchored.
        let pane = drawn.def["children"]
            .as_array()
            .expect("children")
            .iter()
            .find(|c| c["type"] == "signal" && c["buffer"] == 7)
            .expect("the take's pane");
        assert_eq!(pane["axes"]["x"]["cursor"], 0.0);
        assert_eq!(pane["axes"]["x"]["playhead_at"], 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A trim is still one window onto one file**, so the box it leaves draws
    /// that file and reads from the frame the trim moved it to. A clip
    /// assembled from *several* windows draws empty instead -- one box, one
    /// body, and a picture of a third of the samples would be worse than none.
    #[test]
    fn one_window_draws_its_samples_from_where_it_starts_and_several_draw_none() {
        use clausters_document::SegmentRef;

        let dir = std::env::temp_dir().join(format!("clausters_gui_segs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("a.wav"), b"one").expect("write");
        std::fs::write(dir.join("b.wav"), b"two").expect("write");
        let segment = |source: u64, start: f64, duration: f64| SegmentRef {
            source: clausters_document::SegmentSource::Samples(SourceRef {
                source: SourceId(source),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start,
            duration,
        };
        let multitrack = |segments: Vec<SegmentRef>| {
            let node = Node::new(
                NodeId(2),
                Body::Segments {
                    segments,
                    config: Default::default(),
                },
            );
            let document = Document::new(Node::new(
                NodeId(1),
                Body::Aggregate {
                    grouping: Grouping::Concrete,
                    members: vec![Member {
                        offset: 0.0,
                        dur: None,
                        node,
                    }],
                    config: Opaque::none(),
                },
            ));
            Session::new(document)
                .with_source(
                    SourceId(3),
                    Source::file("a.wav", Lifetime::Session).shaped(1, 48_000, 48_000.0),
                )
                .with_source(
                    SourceId(4),
                    Source::file("b.wav", Lifetime::Session).shaped(1, 48_000, 48_000.0),
                )
        };

        let one = multitrack(vec![segment(3, 480.0, 1.0)]);
        let load = sources::plan(&one, &dir, &mut ids_past(7));
        let look = Look {
            takes: Some(&load.takes),
            ..Look::default()
        };
        let trimmed = clip(&draw(&one.document, &look, "trimmed").def);
        assert_eq!(trimmed[6], 7, "the file the window is onto");
        assert_eq!(trimmed[4], 480.0, "read from the frame the trim left it at");

        let joined = multitrack(vec![segment(3, 0.0, 1.0), segment(4, 480.0, 2.0)]);
        let load = sources::plan(&joined, &dir, &mut ids_past(7));
        let look = Look {
            takes: Some(&load.takes),
            ..Look::default()
        };
        let clip = clip(&draw(&joined.document, &look, "joined").def);
        assert_eq!(
            clip[6], -1,
            "two files in one box: drawn empty, and honestly"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_takes_length_is_drawn_against_the_rate_and_a_phrases_against_the_tempo() {
        // At 120 bpm a beat is 24 000 frames and a second is still 48 000, so
        // the two ratios say different things about the same number. A take
        // three seconds long is 144 000 units wide whatever the tempo is --
        // drawing it through the beat would have stretched it to six beats of
        // picture over three seconds of sound.
        let look = Look {
            units_per_beat: 24_000.0,
            units_per_second: 48_000.0,
            tempo: 2.0,
            ..Look::default()
        };
        let mut node = take(2, 3);
        node.duration = Some(3.0);
        let document = Document::new(Node::new(
            NodeId(1),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members: vec![Member {
                    offset: 4.0,
                    dur: None,
                    node,
                }],
                config: Opaque::none(),
            },
        ));
        let clip = clip(&draw(&document, &look, "rates").def);
        assert_eq!(clip[2], 4.0 * 24_000.0, "a placement is musical");
        assert_eq!(clip[3], 3.0 * 48_000.0, "a recording is not");
    }

    /// A resolved take also **opens as an editor**, on its own axis and bound
    /// to the node the clip draws: the arrangement is where the samples are
    /// placed and this is where it is drawn over, and both write the one buffer.
    #[test]
    fn a_resolved_take_opens_an_editor_bound_to_the_same_node() {
        let dir = std::env::temp_dir().join(format!("clausters_gui_edit_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("t.wav"), b"there").expect("write");
        let session = Session::new(one_take(3)).with_source(
            SourceId(3),
            Source::file("t.wav", Lifetime::Session).shaped(1, 96_000, 48_000.0),
        );
        let load = sources::plan(&session, &dir, &mut ids_past(7));
        let look = Look {
            takes: Some(&load.takes),
            ..Look::default()
        };
        let drawn = draw(&session.document, &look, "take");
        let children = drawn.def["children"].as_array().expect("children");
        let editor = children.last().expect("the editor pane");
        assert_eq!(editor["type"], "signal", "a picture of the samples");
        assert_eq!(editor["buffer"], 7, "the very buffer the clip draws");
        assert_eq!(
            editor["gestures"]["alt"], "draw",
            "and a pencil on it, since this is where a sample is a thing"
        );
        assert!(
            editor.get("link").is_none(),
            "on its own axis: zooming to a sample must not scroll the session"
        );
        assert_eq!(
            editor["h"], EDITOR_ROW_H,
            "one channel, one row's worth of height"
        );
        // **The only bindings left are the editors.** A clip is not a widget
        // any more, so there is nothing per clip to bind -- the multitrack is one
        // widget and its boxes name their nodes themselves.
        assert_eq!(
            drawn.bindings.iter().map(|b| b.node).collect::<Vec<_>>(),
            vec![NodeId(2)],
        );
        assert_eq!(
            drawn.picture.clips[0].node,
            NodeId(2),
            "the same node, drawn twice"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A pane as tall as the take is wide.** Four channels are four rows,
    /// and the height is declared by what builds the tree rather than asked of
    /// the widget -- a natural size that followed its data would relayout the
    /// window on every `/gui_set`.
    #[test]
    fn a_wide_takes_editor_opens_taller_and_stops_at_the_cap() {
        assert_eq!(editor_height(None), EDITOR_ROW_H, "an unknown shape is one");
        assert_eq!(editor_height(Some(2)), 2.0 * EDITOR_ROW_H);
        assert_eq!(
            editor_height(Some(16)),
            EDITOR_MAX_H,
            "an ambisonic take is drawn whole, in thinner lanes: a pane taller \
             than the window would push the arrangement off the screen"
        );
    }

    /// Unresolved -- no session, no server, a missing file -- is a box with a
    /// name and no body, and **not** a refusal: what the document says still
    /// moves, undoes and saves.
    #[test]
    fn an_unresolved_take_still_draws_as_a_clip() {
        let doc = one_take(3);
        let drawn = draw(&doc, &Look::default(), "take");
        let clip = clip(&drawn.def);
        assert_eq!(clip[6], -1, "nothing to draw it with");
        assert_eq!(clip[3], 48_000.0, "one beat, for want of a length");
        assert_eq!(drawn.picture.clips.len(), 1, "and it is still a box");
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::host::{ClientId, Host, OscMessage, OscPacket, OscType};
    use clausters_document::{Grouping, Member, Opaque};

    fn from() -> ClientId {
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        )))
    }

    fn doc() -> Document {
        let clang = |id: u64| {
            Node::new(
                NodeId(id),
                Body::Clang {
                    config: Opaque::default(),
                    fires: None,
                },
            )
        };
        Document::new(Node::new(
            NodeId(1),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members: vec![Member {
                    offset: 0.0,
                    dur: None,
                    node: Node::new(
                        NodeId(2),
                        Body::Aggregate {
                            grouping: Grouping::Concrete,
                            members: vec![
                                Member {
                                    offset: 0.0,
                                    dur: Some(2.0),
                                    node: clang(3),
                                },
                                Member {
                                    offset: 4.0,
                                    dur: Some(1.0),
                                    node: clang(4),
                                },
                            ],
                            config: Opaque::none(),
                        },
                    ),
                }],
                config: Opaque::none(),
            },
        ))
    }

    fn open(host: &mut Host, def_id: i32, drawn: &Drawn) {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: "/gui_def".into(),
                args: vec![OscType::Int(def_id), OscType::String(drawn.def.to_string())],
            }),
            from(),
        );
    }

    /// **The tree a document draws actually reaches the registry**, which is
    /// not the same claim as the JSON being right -- and is the one a unit test
    /// over the JSON cannot make.
    ///
    /// Written for a bug that shipped: a def's id *is* its root widget's, so a
    /// tree numbered from 1 handed to `/gui_def 1` collided with itself, the
    /// registry dropped the whole subtree, and the window came up **empty**
    /// with one warning in the log. The multitrack being findable, and holding
    /// the multitrack, is what says the picture exists.
    #[test]
    fn the_multitrack_reaches_the_registry_holding_the_multitrack() {
        let def_id = 1;
        let drawn = draw(
            &doc(),
            &Look {
                first_id: def_id + 1,
                ..Look::default()
            },
            "session",
        );
        let mut host = Host::new();
        open(&mut host, def_id, &drawn);
        let kind = host.widget_kind(def_id, drawn.widget);
        assert!(
            kind.is_some(),
            "the multitrack is missing from the registry"
        );
        // And it built as the **element**, not as an unknown type laid out and
        // never painted -- which is what a name this host did not know would
        // give, and would pass a presence check.
        let info = host
            .widget_kind(def_id, drawn.widget)
            .and_then(|k| {
                k.as_element()
                    .map(crate::host::widget::element::Element::info)
            })
            .expect("an element with something to say");
        let clips = info
            .iter()
            .find(|(k, _)| k == "clips")
            .map(|(_, v)| v.as_array().map_or(0, Vec::len))
            .expect("the clips it holds");
        assert_eq!(clips, 2 * 7, "both boxes, as septuples");
    }

    /// And the failure itself, pinned: numbering from the def's own id loses
    /// the tree. A caller that gets this wrong should fail a test rather than
    /// an eye.
    #[test]
    fn numbering_from_the_defs_own_id_loses_the_tree() {
        let def_id = 1;
        let drawn = draw(&doc(), &Look::default(), "session"); // first_id: 1
        assert_eq!(
            drawn.widget, def_id,
            "the tree numbered over the def's id, which is the collision"
        );
        let mut host = Host::new();
        open(&mut host, def_id, &drawn);
        // The collided id still *resolves* -- to the window itself -- which is
        // exactly why presence is the wrong question and the kind is the right
        // one: what was dropped is the multitrack, not the number.
        assert!(
            host.widget_kind(def_id, drawn.widget)
                .and_then(|k| k.as_element())
                .is_none(),
            "and the registry dropped the widget that collided"
        );
    }
}

#[cfg(test)]
mod depth_tests {
    use super::*;
    use clausters_document::{Grouping, Member, Opaque};

    fn clang(id: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Clang {
                config: Opaque::default(),
                fires: None,
            },
        )
    }

    fn aggregate(id: u64, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members,
                config: Opaque::none(),
            },
        )
    }

    fn at(offset: Beats, node: Node) -> Member {
        Member {
            offset,
            dur: None,
            node,
        }
    }

    /// **A multitrack is nested as deeply as its author nested it**, and this
    /// draws the leaves wherever they are. The shape that found the bug is the
    /// ordinary one: a multitrack of aggregates of tracks of clangs, three
    /// aggregates deep before a single note -- and a walk that stopped at two
    /// drew the containers and called it a picture, which is an empty clip
    /// where the music was.
    #[test]
    fn aggregates_of_tracks_draw_the_leaves_and_not_the_containers() {
        let doc = Document::new(aggregate(
            1,
            vec![
                at(
                    0.0,
                    aggregate(
                        2,
                        vec![
                            at(
                                0.0,
                                aggregate(3, vec![at(0.0, clang(4)), at(1.0, clang(5))]),
                            ),
                            at(0.0, aggregate(6, vec![at(2.0, clang(7))])),
                        ],
                    ),
                ),
                at(0.0, aggregate(8, vec![at(0.0, clang(9))])),
            ],
        ));
        let drawn = draw(&doc, &Look::default(), "deep");
        let names: Vec<NodeId> = drawn.picture.lanes.iter().map(|l| l.node).collect();
        assert_eq!(
            names,
            vec![NodeId(3), NodeId(6), NodeId(8)],
            "a lane per track, wherever it sits -- not one per container"
        );
        let clips: Vec<NodeId> = drawn.picture.clips.iter().map(|c| c.node).collect();
        assert_eq!(clips, vec![NodeId(4), NodeId(5), NodeId(7), NodeId(9)]);
    }

    /// And the offsets accumulate through every level, so a track nested two
    /// aggregates deep still draws where it sounds.
    #[test]
    fn offsets_accumulate_through_every_level() {
        let doc = Document::new(aggregate(
            1,
            vec![at(
                2.0,
                aggregate(2, vec![at(3.0, aggregate(3, vec![at(4.0, clang(4))]))]),
            )],
        ));
        let look = Look {
            units_per_beat: 10.0,
            ..Look::default()
        };
        let drawn = draw(&doc, &look, "deep");
        let clips = drawn.def["children"][0]["clips"].as_array().expect("clips");
        assert_eq!(clips[2], 90.0, "2 + 3 + 4 beats, in units");
        assert_eq!(
            drawn.picture.lanes[0].base, 5.0,
            "and the lane holds the 2 + 3 its member's own offset is measured from"
        );
    }
}

#[cfg(test)]
mod roll_tests {
    use super::*;
    use clausters_document::{Grouping, Member, Opaque};

    fn note(id: u64, midinote: f64, dur: f64) -> Node {
        Node::new(
            NodeId(id),
            Body::Clang {
                config: Opaque(json!({ "midinote": midinote, "dur": dur })),
                fires: None,
            },
        )
    }

    fn aggregate(id: u64, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members,
                config: Opaque::none(),
            },
        )
    }

    fn at(offset: Beats, node: Node) -> Member {
        Member {
            offset,
            dur: None,
            node,
        }
    }

    /// **A track is one clip**, which is what a track is in every editor -- and
    /// what a document of clangs has to become to be read as music rather than
    /// as a row of empty rectangles.
    ///
    /// The notes themselves are not drawn inside it: the multitrack **places**,
    /// and a body that is edited is entered. Drawing the roll read-only inside
    /// the box is a widget feature this host does not have yet
    /// (`clients/gui/PLAN.md`, "Found by use").
    #[test]
    fn an_aggregate_of_clangs_is_one_clip_as_long_as_the_notes_reach() {
        let doc = Document::new(aggregate(
            1,
            vec![at(
                0.0,
                aggregate(
                    2,
                    vec![at(0.0, note(3, 72.0, 1.0)), at(2.0, note(4, 76.0, 1.0))],
                ),
            )],
        ));
        let look = Look {
            units_per_beat: 100.0,
            ..Look::default()
        };
        let drawn = draw(&doc, &look, "t");
        assert_eq!(drawn.picture.clips.len(), 1, "one clip, not one per note");
        // The clip draws the **aggregate**, so a drag on it moves the track.
        assert_eq!(drawn.picture.clips[0].node, NodeId(2));
        // ...and its lane is the track too, held by the multitrack above it: a lane
        // that *is* one element borrows its parent's members, because that is
        // where the element is a member.
        assert_eq!(drawn.picture.lanes[0].node, NodeId(2));
        assert_eq!(drawn.picture.lanes[0].holder, NodeId(1));
    }

    /// An aggregate that is **not** all pitched clangs stays a lane of clips: a
    /// roll drawn over half an aggregate would leave elements out without
    /// saying so.
    #[test]
    fn an_aggregate_that_is_not_all_notes_stays_a_lane_of_clips() {
        let doc = Document::new(aggregate(
            1,
            vec![at(
                0.0,
                aggregate(
                    2,
                    vec![
                        at(0.0, note(3, 72.0, 1.0)),
                        // A leaf with no pitch: a take, a generator, anything.
                        at(
                            2.0,
                            Node::new(
                                NodeId(4),
                                Body::Clang {
                                    config: Opaque::none(),
                                    fires: None,
                                },
                            ),
                        ),
                    ],
                ),
            )],
        ));
        let drawn = draw(&doc, &Look::default(), "t");
        assert_eq!(
            drawn
                .picture
                .clips
                .iter()
                .map(|c| c.node)
                .collect::<Vec<_>>(),
            vec![NodeId(3), NodeId(4)],
            "two boxes on the lane, not one carrying half a roll"
        );
        assert_eq!(
            drawn.picture.lanes[0].holder,
            NodeId(2),
            "and the lane holds them"
        );
    }
}
