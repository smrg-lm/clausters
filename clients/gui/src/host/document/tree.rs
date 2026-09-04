//! Drawing a document as a multitrack — the host's own `Editor`.
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
//! anything else is a lane) — the protocol's "generic on the wire, typed in the
//! renderer" invariant, which the Python builders' names hide and this had to
//! learn the hard way: a tree that says `"type": "track"` builds nothing, and
//! an empty window is all it says about it.
//!
//! # It builds a GuiDef, and nothing else
//!
//! What comes out is the ordinary `{id, type, props, children}` tree the host
//! already parses — no new widget, no new prop, no path into the tree that a
//! script could not take. That is the point: a standalone editor is this host
//! driven by itself, so anything it can draw a script can draw too, and
//! anything it cannot is missing for both.
//!
//! # A clip's body, and the one thing this cannot know
//!
//! An aggregate of pitched clangs draws as a **roll** from the tree alone,
//! because the pitches are in the tree. A **take** cannot: the document names a
//! source and never says where the samples are, so drawing one needs the
//! session's table resolved to something a host can read — which is
//! [`super::sources`], and is why `Look` takes the resolved [`Takes`] rather
//! than the session. Given none, a take is still drawn: its placement and its
//! name, honest about the rest.

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
    /// Samples per beat — what a clip's `offset` is drawn with, since the
    /// shared time axis measures samples and a placement is in beats.
    pub units_per_beat: f64,
    /// Samples per second — what a clip's `dur` is drawn with when the data
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
    /// itself, and the registry drops the whole subtree — which looks like an
    /// empty window and one line in the log.
    pub first_id: i32,
    /// The document's **content**: the nodes a window names
    /// ([`clausters_document::SegmentSource::Node`]).
    ///
    /// A cut over notes leaves two windows onto one timeline, and the timeline
    /// is here rather than in either of them — so without this a split roll clip
    /// draws as an empty rectangle, which is the picture for *the samples are
    /// elsewhere* and a lie about notes the document is holding.
    pub content: Option<&'a [Node]>,
    /// The session's samples, once somebody has resolved it to buffers.
    ///
    /// `None` — or a source missing from it — draws a take as its placement and
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

/// The window a document draws as, plus what each widget in it is a picture of.
pub struct Drawn {
    /// The GuiDef, ready for `/gui_def`.
    pub def: Value,
    /// Every clip's widget, and the node it draws.
    pub bindings: Vec<Bound>,
    /// Every **lane's** widget, and the node its header configures.
    ///
    /// Kept apart from the clips rather than folded in with them, because the
    /// two answer different questions: a clip is a placement a hand moves, a
    /// lane is an element a hand mutes. Both are bound the same way and by the
    /// same call — what is separate is only the *counting*, so "twelve clips"
    /// goes on meaning twelve clips.
    pub headers: Vec<Bound>,
    /// The next free widget id, so a caller can keep allocating after it.
    pub next_id: i32,
}

/// Draws `document` as a window of lanes.
///
/// One lane per top-level member: an **aggregate** becomes a lane of its
/// members' clips (which is what a track is), and anything else becomes a lane
/// holding one clip. Nesting deeper than that is drawn flat for now — an
/// aggregate inside an aggregate is one lane of its own, in document order —
/// because an expanded/collapsed state is a thing the *editor* holds and this
/// has nowhere yet to keep one.
pub fn draw(document: &Document, look: &Look<'_>, title: &str) -> Drawn {
    let mut ids = Ids {
        next: look.first_id,
    };
    let mut bindings = Vec::new();
    let mut headers = Vec::new();
    let mut lanes: Vec<Value> = Vec::new();
    // The content is the document's, so it is filled in here rather than asked
    // of a caller that would have to remember.
    let look = &Look {
        content: Some(&document.content),
        ..*look
    };

    match &document.root.body {
        Body::Aggregate { members, .. } => {
            for member in members {
                lane_of(
                    member,
                    0.0,
                    look,
                    &mut ids,
                    &mut bindings,
                    &mut headers,
                    &mut lanes,
                );
            }
        }
        // A document that is one thing is one lane holding it.
        _ => {
            let member = Member {
                offset: 0.0,
                dur: None,
                node: document.root.clone(),
            };
            lane_of(
                &member,
                0.0,
                look,
                &mut ids,
                &mut bindings,
                &mut headers,
                &mut lanes,
            );
        }
    }

    // The ruler joins the lanes' navigation group rather than owning a strip
    // inside one of them, exactly as the Python editor places it.
    lanes.push(json!({
        "id": ids.take(),
        "type": "field",
        "h": 20.0,
        "ruler": "beats",
        "sample_rate": look.sample_rate,
        "tempo": look.tempo,
    }));

    lanes.extend(take_editors(document, look, &mut ids, &mut bindings));

    let def = json!({
        "type": "window",
        "title": title,
        "layout": "col",
        "w": 1000,
        "h": 640,
        "children": lanes,
    });
    Drawn {
        def,
        bindings,
        headers,
        next_id: ids.next,
    }
}

/// The height one channel's lane gets in a take's editor, in logical pixels:
/// enough for a trace to have a shape, and small enough that the tracks above
/// stay the picture.
const EDITOR_LANE_H: f64 = 120.0;

/// The tallest a take's editor is built, however many channels it holds. Past
/// this the lanes get thinner instead — a pane taller than the window would
/// push the arrangement off the screen, which is worse than a cramped lane.
const EDITOR_MAX_H: f64 = 360.0;

/// **The pane a take opens in is as tall as the take is wide**: samples with
/// four channels is four lanes, and a view that shows two of them is a picture
/// of half the file — the same argument that makes a clip draw every channel.
///
/// It is declared **here, by what built the tree**, and not asked of the widget:
/// a natural size that followed its data would relayout the window whenever a
/// `/gui_set` landed, so the rule is that the driver knows the shape and says
/// so. This driver reads it from the session's own source table, which is where
/// a file's channel count is written down before anything is drawn.
///
/// It stops growing at [`EDITOR_MAX_H`], and what that costs is stated rather
/// than hidden: an ambisonic take's sixteen lanes are drawn, at sixteenths of
/// that height. Making many channels *readable* — scrolling the pane, folding
/// lanes, choosing which to show — is a design this does not have yet
/// (`clients/gui/PLAN.md`, "Future directions").
fn editor_height(channels: Option<u32>) -> f64 {
    let lanes = channels.unwrap_or(1).max(1) as f64;
    (EDITOR_LANE_H * lanes).min(EDITOR_MAX_H)
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

/// Turns one member into lanes, **recursing while it is aggregates all the way
/// down**.
///
/// The rule is the shape of the samples rather than a depth: an aggregate
/// whose members are leaves is a lane of clips (that is what a lane *is*), and
/// an aggregate of aggregates is not one lane but each of theirs. A composition
/// is nested as deeply as the author nested it — a piece of aggregates of
/// tracks of clangs is three deep before a single note is reached — so anything
/// that stops at a fixed depth draws the containers and calls it a picture,
/// which is an empty clip where the music was.
///
/// `base` accumulates the offsets on the way down, because a clip's offset is
/// absolute on the shared axis while a member's is relative to its aggregate.
#[allow(clippy::too_many_arguments)] // one accumulator per thing being built
fn lane_of(
    member: &Member,
    base: Beats,
    look: &Look<'_>,
    ids: &mut Ids,
    bindings: &mut Vec<Bound>,
    headers: &mut Vec<Bound>,
    lanes: &mut Vec<Value>,
) {
    let here = base + member.offset;
    if let Body::Aggregate { members, .. } = &member.node.body
        && members
            .iter()
            .any(|inner| matches!(inner.node.body, Body::Aggregate { .. }))
    {
        for inner in members {
            lane_of(inner, here, look, ids, bindings, headers, lanes);
        }
        return;
    }
    let label = label_of(&member.node);
    let clips = match &member.node.body {
        // **An aggregate of clangs is one clip with a roll in it**, not a lane of
        // clips: that is what a track *is* in every editor, and drawing each
        // note as its own clip gives a row of empty rectangles where the music
        // was. The notes go in the clip's own axis, so their starts are
        // relative to it.
        Body::Aggregate { members, .. } if !members.is_empty() && notes_of(members).is_some() => {
            let notes = notes_of(members).expect("checked");
            vec![roll_clip(member, here, notes, look, ids, bindings)]
        }
        // A **window onto a timeline**: what a cut over notes leaves. The notes
        // are content the document holds once, and this half shows the stretch
        // of them its window names.
        Body::Segments { .. } if windowed_notes(&member.node, look).is_some() => {
            let notes = windowed_notes(&member.node, look).expect("checked");
            vec![roll_clip(member, here, notes, look, ids, bindings)]
        }
        Body::Aggregate { members, .. } => members
            .iter()
            .map(|inner| match windowed_notes(&inner.node, look) {
                // A lane of clips, one of which is a window onto notes: the two
                // halves of a cut sit on the lane like any other pair of clips.
                Some(notes) => roll_clip(inner, here, notes, look, ids, bindings),
                None => clip_of(inner, here, look, ids, bindings),
            })
            .collect(),
        _ => vec![clip_of(member, base, look, ids, bindings)],
    };
    let widget = ids.take();
    // **The lane is bound to what it draws.** Its header's mute, solo and level
    // are the *element's* configuration, so a press on one has to name a node —
    // and until this binding existed only the clips inside a lane named
    // anything, which left a header a person could press and nothing could
    // read.
    headers.push(Bound {
        widget,
        node: member.node.id,
    });
    let mut props = Map::new();
    props.insert("id".into(), json!(widget));
    props.insert("type".into(), json!("field"));
    props.insert("label".into(), json!(label));
    props.insert("sample_rate".into(), json!(look.sample_rate));
    props.insert("tempo".into(), json!(look.tempo));
    // The mixing the piece carries. A node's configuration is opaque here and
    // is round-tripped whole, so it was already saved and already restored;
    // what was missing was drawing it, which is why a piece muted in a client
    // opened audible.
    for (key, value) in mixing_of(&member.node) {
        props.insert(key.into(), value);
    }
    if look.quant > 0.0 {
        props.insert("snap".into(), json!(look.quant * look.units_per_beat));
    }
    // **The window is the reader's.** A session host is an editor, and in an
    // editor a content change is mostly the reader's own edit -- undoing a
    // trim, splitting a clip, dragging one onto another lane -- so the axis
    // does not re-frame itself on one. The extent is still registered; only the
    // window stays put.
    props.insert("autofit".into(), json!(false));
    props.insert("children".into(), Value::Array(clips));
    lanes.push(Value::Object(props));
}

fn clip_of(
    member: &Member,
    base: Beats,
    look: &Look<'_>,
    ids: &mut Ids,
    bindings: &mut Vec<Bound>,
) -> Value {
    let widget = ids.take();
    bindings.push(Bound {
        widget,
        node: member.node.id,
    });
    let take = take_of(&member.node, look);
    let dur = clip_units(member, look.takes, look);
    let mut props = Map::new();
    props.insert("id".into(), json!(widget));
    props.insert("type".into(), json!("field"));
    props.insert(
        "offset".into(),
        json!((base + member.offset) * look.units_per_beat),
    );
    props.insert("dur".into(), json!(dur));
    props.insert("label".into(), json!(label_of(&member.node)));
    // The samples, as a **server buffer**: the clip's take body fetches it
    // over the host's client leg, which is the same route a script's clip
    // takes. What is drawn is then what an edit writes — one copy, not a
    // picture of one and a write to another.
    if let Some(take) = take {
        props.insert("buffer".into(), json!(take.bufnum));
        if let Some(channels) = take.channels {
            props.insert("channels".into(), json!(channels));
        }
    }
    // **Assembled samples draw a take per window**, each over its own stretch
    // of the clip: one clip, because that is what the element is, and one body
    // per piece, because each reads a different part of different samples.
    if let Body::Segments { segments, .. } = &member.node.body {
        let mut children = Vec::new();
        let mut cursor = 0.0f64;
        for segment in segments {
            // A window's length is in seconds -- these are samples -- so it is
            // drawn against the rate and not against the tempo.
            let len = segment.duration * look.units_per_second;
            let at = cursor;
            cursor += len;
            // A window onto a **node** draws nothing here: what it reads is
            // content of the document, not a take, and the body that shows it
            // is the one its own kind asks for.
            let Some(source) = segment.source.samples() else {
                continue;
            };
            let Some(take) = look.takes.and_then(|t| t.get(source.source)) else {
                continue;
            };
            let mut body = Map::new();
            body.insert("type".into(), json!("signal"));
            body.insert("view".into(), json!("trace"));
            body.insert("buffer".into(), json!(take.bufnum));
            if let Some(channels) = take.channels {
                body.insert("channels".into(), json!(channels));
            }
            body.insert("at".into(), json!(at));
            body.insert("dur".into(), json!(len));
            if segment.start != 0.0 {
                body.insert("start".into(), json!(segment.start));
            }
            children.push(Value::Object(body));
        }
        if !children.is_empty() {
            props.insert("children".into(), json!(children));
        }
    }
    Value::Object(props)
}

/// **One editor per take**, under the tracks: the samples as a navigable
/// picture of themselves, where they can be zoomed to the sample and drawn over.
///
/// The arrangement says *where* samples are and the editor is where they are
/// *edited*, and the two views are of one thing — the same server buffer, the
/// same node — so a stroke here moves the clip's picture above it without
/// anything being told. That is the whole reason a take opens as a second view
/// rather than as a mode over the clip: a lane measures beats and a pencil
/// measures samples, and one axis cannot be both.
///
/// It stays **out of the tracks' navigation group** for that same reason: the
/// arrangement's zoom is where the piece is, the editor's is how close the hand
/// is, and joining them would make drawing a sample scroll the whole session.
///
/// A source drawn by several clips opens once (the first node that names it):
/// the buffer is one, and editing it twice would be two pictures of the same
/// buffer disagreeing while the hand is down.
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
        // joined clip is edited piece by piece, since a piece is what a file
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
            let mut props = Map::new();
            props.insert("id".into(), json!(widget));
            props.insert("type".into(), json!("signal"));
            props.insert("view".into(), json!("trace"));
            props.insert("buffer".into(), json!(take.bufnum));
            if let Some(channels) = take.channels {
                props.insert("channels".into(), json!(channels));
            }
            props.insert("label".into(), json!(label_of(node)));
            props.insert("h".into(), json!(editor_height(take.channels)));
            props.insert("sample_rate".into(), json!(look.sample_rate));
            props.insert("ruler".into(), json!("samples"));
            // **The head is anchored at 0, and that is the whole of drawing it.**
            // A session's clock is the *piece's position* rather than the device's
            // (`HeadClock::Piece`), so the sweep from an anchor of 0 is the
            // position itself: it stands still while the transport is stopped,
            // jumps where a locate puts it and wraps where the engine wraps it.
            // No `playhead_loop` here for the same reason — the loop is the
            // transport's, and wrapping an already-wrapped number would double it.
            props.insert("playhead_at".into(), json!(0.0));
            // The plan, and it is three gestures rather than a mode: a plain drag
            // sweeps a selection (what an editor does by default), Alt draws over
            // the samples and Ctrl grabs one. `draw` refuses out loud below the
            // zoom where a pixel is one sample, so it never silently paints what
            // the eye cannot check.
            props.insert(
                "gestures".into(),
                json!({"drag": "select", "alt": "draw", "ctrl": "sample"}),
            );
            out.push(Value::Object(props));
        }
    });
    out
}

/// The samples a node draws, when it names some and somebody resolved it.
/// The length a clip is drawn at, **in timeline units**: the placement's where
/// it overrides, else the element's own, else **the samples'** — a take placed
/// 1:1 is as long as it is, which is the one length nobody has to state — else
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
/// had given it — which is an undo that moves the document and not the picture.
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

fn take_of(node: &Node, look: &Look<'_>) -> Option<super::sources::Take> {
    let Body::Vector { source, .. } = &node.body else {
        return None;
    };
    look.takes?.get(source.source)
}

/// The notes a **window onto content** shows, placed from the window's own zero.
///
/// A cut over notes is two windows onto one timeline: the timeline is a node in
/// [`Document::content`] and each half names it. What a half draws is the notes
/// inside its window, shifted back to its own start — the same reading the
/// clients do, made here so a host with no client draws a split piece the way
/// the piece is.
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

/// The `notes` body of an aggregate of clangs — the flat `start dur pitch velocity
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
        // document is built on — a host does not interpret a leaf's config, it
        // only draws what it can recognize.
        let pitch = config
            .0
            .get("midinote")
            .and_then(Value::as_f64)
            .or_else(|| config.0.get("note").and_then(Value::as_f64))?;
        // The sounding length: the clang's own `dur`, the placement's, or a
        // beat — a note with no length would be a line.
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

/// One clip drawing an aggregate of clangs as a roll.
fn roll_clip(
    member: &Member,
    base: Beats,
    notes: Vec<(Beats, Beats, f32)>,
    look: &Look<'_>,
    ids: &mut Ids,
    bindings: &mut Vec<Bound>,
) -> Value {
    let widget = ids.take();
    // The clip draws the **aggregate**, so a drag on it moves the track: a note
    // inside it edits through the roll's own payload, which addresses the note.
    bindings.push(Bound {
        widget,
        node: member.node.id,
    });
    let span = notes
        .iter()
        .map(|(start, dur, _)| start + dur)
        .fold(0.0f64, f64::max);
    let dur = member
        .dur
        .or(member.node.duration)
        .filter(|d| *d > 0.0)
        .unwrap_or(span.max(1.0));
    let flat: Vec<Value> = notes
        .iter()
        .flat_map(|(start, dur, pitch)| {
            [
                json!(start * look.units_per_beat),
                json!(dur * look.units_per_beat),
                json!(pitch),
                json!(100), // the roll's default velocity
                json!(0),   // and channel
            ]
        })
        .collect();
    json!({
        "id": widget,
        "type": "field",
        "offset": base * look.units_per_beat,
        "dur": dur * look.units_per_beat,
        "label": label_of(&member.node),
        "notes": flat,
    })
}

/// What a node is called on screen. The document holds no names — a name is a
/// client's idea — so this says what it *is*, which is what a reader needs from
/// a picture drawn by a host that was handed a file.
/// The three mixing props a lane header draws, as the node carries them.
///
/// Only what is actually written: a lane with no `mute` in its configuration is
/// not muted, and saying so with a `false` would be this driver inventing a
/// value the document does not hold. The keys are the clients' own
/// (`clausters.form.document`'s `MIXING`), which is what makes a piece muted in
/// a script open muted here.
fn mixing_of(node: &Node) -> Vec<(&'static str, Value)> {
    let Some(config) = node.body.config() else {
        return Vec::new();
    };
    let Some(table) = config.0.as_object() else {
        return Vec::new();
    };
    ["mute", "solo", "level"]
        .into_iter()
        .filter_map(|key| table.get(key).map(|value| (key, value.clone())))
        .collect()
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

    fn children(def: &Value) -> &Vec<Value> {
        def["children"].as_array().expect("a window has children")
    }

    #[test]
    fn an_aggregate_of_aggregates_draws_one_lane_each_and_a_ruler_under_them() {
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
        let kids = children(&drawn.def);
        assert_eq!(kids.len(), 3, "two lanes and the ruler");
        // All three are `field` on the wire and told apart by their props: the
        // lanes carry children, the ruler carries a height and no children.
        for kid in kids {
            assert_eq!(kid["type"], "field");
        }
        assert!(kids[0]["children"].is_array() && kids[1]["children"].is_array());
        assert!(kids[2].get("children").is_none() && kids[2]["ruler"] == "beats");
    }

    /// A clip's offset is **absolute on the shared axis** while a member's is
    /// relative to its aggregate: the two are added once, here, or every lane after
    /// the first would draw in the wrong place.
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
        let clip = &children(&drawn.def)[0]["children"][0];
        assert_eq!(clip["offset"], 500.0, "4 + 1 beats, in units");
        assert_eq!(clip["dur"], 200.0);
    }

    /// Every clip says which node it draws — the one thing only what built the
    /// tree can know, and what an intent needs to name.
    #[test]
    fn every_clip_is_bound_to_the_node_it_draws() {
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
        let nodes: Vec<u64> = drawn.bindings.iter().map(|b| b.node.0).collect();
        assert_eq!(nodes, vec![7, 8], "the clips, in document order");
        let clips = children(&drawn.def)[0]["children"].as_array().unwrap();
        let widgets: Vec<i64> = clips.iter().map(|c| c["id"].as_i64().unwrap()).collect();
        assert_eq!(
            widgets,
            drawn
                .bindings
                .iter()
                .map(|b| b.widget as i64)
                .collect::<Vec<_>>(),
            "and the bindings name those very widgets"
        );
        assert!(drawn.next_id > widgets.iter().max().copied().unwrap() as i32);
    }

    /// A document that is one thing is still a window: a host handed a file
    /// draws what it was given rather than refusing a shape.
    #[test]
    fn a_document_that_is_not_an_aggregate_still_draws() {
        let doc = Document::new(clang(1));
        let drawn = draw(&doc, &Look::default(), "one thing");
        let kids = children(&drawn.def);
        assert_eq!(kids.len(), 2, "one lane and the ruler");
        assert_eq!(drawn.bindings.len(), 1);
        assert_eq!(drawn.bindings[0].node, NodeId(1));
    }

    /// **A piece muted in a client opens muted here.** A node's configuration
    /// was already carried across a save; what was missing was reading it.
    #[test]
    fn a_lane_draws_the_mixing_its_element_carries() {
        let mut track = aggregate(2, vec![placed(0.0, None, clang(3))]);
        if let Body::Aggregate { config, .. } = &mut track.body {
            *config = Opaque(serde_json::json!({"mute": true, "level": 0.25}));
        }
        let doc = Document::new(aggregate(1, vec![placed(0.0, None, track)]));
        let drawn = draw(&doc, &Look::default(), "session");
        let lane = &children(&drawn.def)[0];
        assert_eq!(lane["mute"], true);
        assert_eq!(lane["level"], 0.25);
        assert!(
            lane.get("solo").is_none(),
            "only what the document holds: an absent key is not a false one"
        );
    }

    /// And the header is bound, so a press on it names the node it configures —
    /// which is what the clips inside the lane had and the lane itself did not.
    #[test]
    fn a_lane_is_bound_to_the_element_its_header_configures() {
        let doc = Document::new(aggregate(
            1,
            vec![placed(
                0.0,
                None,
                aggregate(2, vec![placed(0.0, None, clang(3))]),
            )],
        ));
        let drawn = draw(&doc, &Look::default(), "session");
        assert_eq!(
            drawn.headers.iter().map(|b| b.node).collect::<Vec<_>>(),
            vec![NodeId(2)],
            "one lane, bound to the track it draws"
        );
        assert_eq!(drawn.headers[0].widget, children(&drawn.def)[0]["id"]);
        assert!(
            drawn.bindings.iter().all(|b| b.node != NodeId(2)),
            "and the clips are still the clips"
        );
    }

    #[test]
    fn a_grid_reaches_the_lane_and_nothing_reaches_it_when_there_is_none() {
        let doc = Document::new(aggregate(1, vec![placed(0.0, None, clang(2))]));
        let plain = draw(&doc, &Look::default(), "t");
        assert!(children(&plain.def)[0].get("snap").is_none());
        let snapped = draw(
            &doc,
            &Look {
                quant: 0.5,
                units_per_beat: 100.0,
                ..Look::default()
            },
            "t",
        );
        assert_eq!(children(&snapped.def)[0]["snap"], 50.0);
    }
}

#[cfg(test)]
mod take_tests {
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
        let load = sources::plan(&session, &dir, 7);
        let look = Look {
            takes: Some(&load.takes),
            ..Look::default()
        };
        let drawn = draw(&session.document, &look, "take");
        let clip = &drawn.def["children"][0]["children"][0];
        assert_eq!(clip["buffer"], 7, "the buffer it was read into");
        assert_eq!(clip["channels"], 2);
        assert_eq!(
            clip["dur"], 96_000.0,
            "as long as the samples, in timeline units"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Assembled samples draw one clip and a take per window**, each over
    /// its own stretch of it — the standalone host reading what a joined clip
    /// was saved as, which is the one path a script is not there to draw.
    #[test]
    fn segments_draw_one_clip_with_a_take_per_window() {
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
        let node = Node::new(
            NodeId(2),
            Body::Segments {
                segments: vec![segment(3, 0.0, 1.0), segment(4, 480.0, 2.0)],
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
        let session = Session::new(document)
            .with_source(
                SourceId(3),
                Source::file("a.wav", Lifetime::Session).shaped(1, 48_000, 48_000.0),
            )
            .with_source(
                SourceId(4),
                Source::file("b.wav", Lifetime::Session).shaped(1, 48_000, 48_000.0),
            );
        let load = sources::plan(&session, &dir, 7);
        let look = Look {
            takes: Some(&load.takes),
            ..Look::default()
        };
        let drawn = draw(&session.document, &look, "joined");
        let clip = &drawn.def["children"][0]["children"][0];
        let takes = clip["children"].as_array().expect("a body per window");
        assert_eq!(takes.len(), 2);
        assert_eq!(takes[0]["buffer"], 7, "the first window's samples");
        assert_eq!(takes[1]["buffer"], 8, "and the second's, a different file");
        // Each on its own stretch of the clip, in timeline units.
        assert_eq!(takes[0]["at"], 0.0);
        // A window's length is in seconds, so it is drawn against the rate.
        assert_eq!(takes[1]["at"], look.units_per_second);
        assert_eq!(takes[1]["dur"], 2.0 * look.units_per_second);
        // ...and the second reads its own frame, which the first does not name.
        assert_eq!(takes[1]["start"], 480.0);
        assert!(takes[0].get("start").is_none());
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
        let drawn = draw(&document, &look, "rates");
        let clip = &drawn.def["children"][0]["children"][0];
        assert_eq!(clip["offset"], 4.0 * 24_000.0, "a placement is musical");
        assert_eq!(clip["dur"], 3.0 * 48_000.0, "a recording is not");
    }

    /// A resolved take also **opens as an editor**, on its own axis and bound
    /// to the same node the clip is: the arrangement is where the samples are
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
        let load = sources::plan(&session, &dir, 7);
        let look = Look {
            takes: Some(&load.takes),
            ..Look::default()
        };
        let drawn = draw(&session.document, &look, "take");
        let children = drawn.def["children"].as_array().expect("lanes");
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
            editor["h"], EDITOR_LANE_H,
            "one channel, one lane's worth of height"
        );
        let bound: Vec<NodeId> = drawn.bindings.iter().map(|b| b.node).collect();
        assert_eq!(
            bound,
            vec![NodeId(2), NodeId(2)],
            "the clip and the editor are two views of one node"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A pane as tall as the take is wide.** Four channels are four lanes,
    /// and the height is declared by what builds the tree rather than asked of
    /// the widget — a natural size that followed its data would relayout the
    /// window on every `/gui_set`.
    #[test]
    fn a_wide_takes_editor_opens_taller_and_stops_at_the_cap() {
        assert_eq!(
            editor_height(None),
            EDITOR_LANE_H,
            "an unknown shape is one"
        );
        assert_eq!(editor_height(Some(2)), 2.0 * EDITOR_LANE_H);
        assert_eq!(
            editor_height(Some(16)),
            EDITOR_MAX_H,
            "an ambisonic take is drawn whole, in thinner lanes: a pane taller \
             than the window would push the arrangement off the screen"
        );
    }

    /// Unresolved — no session, no server, a missing file — is a clip with a
    /// name and no body, and **not** a refusal: what the document says still
    /// moves, undoes and saves.
    #[test]
    fn an_unresolved_take_still_draws_as_a_clip() {
        let doc = one_take(3);
        let drawn = draw(&doc, &Look::default(), "take");
        let clip = &drawn.def["children"][0]["children"][0];
        assert!(clip.get("buffer").is_none(), "nothing to draw it with");
        assert_eq!(clip["dur"], 48_000.0, "one beat, for want of a length");
        assert_eq!(drawn.bindings.len(), 1, "and it is still bound");
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
    /// not the same claim as the JSON being right — and is the one a unit test
    /// over the JSON cannot make.
    ///
    /// Written for a bug that shipped: a def's id *is* its root widget's, so a
    /// tree numbered from 1 handed to `/gui_def 1` collided with itself, the
    /// registry dropped the whole subtree, and the window came up **empty**
    /// with one warning in the log. Every clip being findable afterwards is
    /// what says the picture exists.
    #[test]
    fn every_drawn_widget_reaches_the_registry() {
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
        for bound in &drawn.bindings {
            let kind = host.widget_kind(def_id, bound.widget);
            assert!(
                kind.is_some(),
                "clip widget {} is missing from the registry",
                bound.widget
            );
            // And it built as a **clip**: the three lane-ish widgets share one
            // wire name and are told apart by their props, so a clip that came
            // out a lane would pass a presence check and draw nothing of what
            // it is.
            assert!(
                matches!(kind, Some(crate::host::widget::WidgetKind::Clip { .. })),
                "widget {} built as {:?} rather than a clip",
                bound.widget,
                kind.map(std::mem::discriminant)
            );
        }
    }

    /// And the failure itself, pinned: numbering from the def's own id loses
    /// the tree. A caller that gets this wrong should fail a test rather than
    /// an eye.
    #[test]
    fn numbering_from_the_defs_own_id_loses_the_tree() {
        let def_id = 1;
        let drawn = draw(&doc(), &Look::default(), "session"); // first_id: 1
        assert!(
            drawn.bindings.iter().any(|b| b.widget == def_id),
            "the tree numbered over the def's id, which is the collision"
        );
        let mut host = Host::new();
        open(&mut host, def_id, &drawn);
        // The collided id still *resolves* — to the window itself — which is
        // exactly why presence is the wrong question and the kind is the right
        // one: what was dropped is the clip, not the number.
        assert!(
            drawn.bindings.iter().any(|b| !matches!(
                host.widget_kind(def_id, b.widget),
                Some(crate::host::widget::WidgetKind::Clip { .. })
            )),
            "and the registry dropped the clip that collided"
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

    /// **A composition is nested as deeply as its author nested it**, and this
    /// draws the leaves wherever they are. The shape that found the bug is the
    /// ordinary one: a piece of aggregates of tracks of clangs, three
    /// aggregates deep before a single note — and a walk that stopped at two
    /// drew the containers and called it a picture, which is an empty clip
    /// where the music was.
    #[test]
    fn a_piece_of_aggregates_of_tracks_draws_the_notes_and_not_the_containers() {
        let doc = Document::new(aggregate(
            1,
            vec![
                at(
                    0.0,
                    aggregate(
                        2,
                        vec![at(
                            0.0,
                            aggregate(3, vec![at(0.0, clang(4)), at(2.0, clang(5))]),
                        )],
                    ),
                ),
                at(
                    0.0,
                    aggregate(6, vec![at(0.0, aggregate(7, vec![at(0.0, clang(8))]))]),
                ),
            ],
        ));
        let drawn = draw(&doc, &Look::default(), "piece");
        let nodes: Vec<u64> = drawn.bindings.iter().map(|b| b.node.0).collect();
        assert_eq!(
            nodes,
            vec![4, 5, 8],
            "the clips are the clangs, not the tracks that hold them"
        );
        let kids = drawn.def["children"].as_array().unwrap();
        assert_eq!(kids.len(), 3, "one lane per track, and the ruler");
        assert_eq!(
            kids[0]["children"].as_array().unwrap().len(),
            2,
            "the first track's two notes"
        );
    }

    /// **A cut over notes draws as two clips of one timeline**, and the host
    /// has to read the window to do it: the notes are content the document
    /// holds once, each half names it, and a host that stopped at the tree
    /// would draw two empty rectangles where the phrase is.
    #[test]
    fn a_window_onto_content_draws_the_notes_its_window_names() {
        use clausters_document::{SegmentRef, SegmentSource};

        let note = |id: u64, pitch: f64| {
            Node::new(
                NodeId(id),
                Body::Clang {
                    config: Opaque(json!({ "midinote": pitch, "dur": 1.0 })),
                    fires: None,
                },
            )
        };
        let held = aggregate(10, vec![at(0.0, note(11, 60.0)), at(2.0, note(12, 64.0))]);
        let half = |id: u64, start: Beats, dur: Beats| {
            Node::new(
                NodeId(id),
                Body::Segments {
                    segments: vec![SegmentRef {
                        source: SegmentSource::Node { node: NodeId(10) },
                        start,
                        duration: dur,
                    }],
                    config: Opaque::none(),
                },
            )
        };
        let doc = Document::new(aggregate(
            1,
            vec![at(
                0.0,
                aggregate(
                    4,
                    vec![at(0.0, half(2, 0.0, 2.0)), at(2.0, half(3, 2.0, 2.0))],
                ),
            )],
        ))
        .with_content(vec![held]);

        let drawn = draw(&doc, &Look::default(), "piece");
        let clips = drawn.def["children"][0]["children"].as_array().unwrap();
        assert_eq!(clips.len(), 2, "one clip per window");
        assert_eq!(
            windowed_notes(
                &Node::new(
                    NodeId(9),
                    Body::Segments {
                        segments: vec![
                            SegmentRef {
                                source: SegmentSource::Node { node: NodeId(10) },
                                start: 0.0,
                                duration: 2.0,
                            },
                            SegmentRef {
                                source: SegmentSource::Node { node: NodeId(10) },
                                start: 2.0,
                                duration: 2.0,
                            },
                        ],
                        config: Opaque::none(),
                    },
                ),
                &Look {
                    content: Some(&doc.content),
                    ..Look::default()
                },
            )
            .map(|notes| notes.iter().map(|(at, _, _)| *at).collect::<Vec<_>>()),
            Some(vec![0.0, 2.0]),
            "a run of two windows reads them back to back"
        );
        // Each half draws the note its window holds, placed from its own zero --
        // and the second's note is *not* drawn at beat two, which is where it
        // sits on the timeline they share.
        for clip in clips {
            let notes = clip["notes"].as_array().unwrap();
            assert_eq!(notes.len() / 5, 1, "one note in each window");
            assert_eq!(notes[0], 0.0, "placed from the window's own start");
        }
    }

    /// The offsets accumulate all the way down: a note two beats into a track
    /// that starts four beats into the piece sits at six on the shared axis.
    #[test]
    fn offsets_accumulate_through_every_level() {
        let doc = Document::new(aggregate(
            1,
            vec![at(
                4.0,
                aggregate(2, vec![at(0.0, aggregate(3, vec![at(2.0, clang(4))]))]),
            )],
        ));
        let look = Look {
            units_per_beat: 10.0,
            ..Look::default()
        };
        let drawn = draw(&doc, &look, "piece");
        let clip = &drawn.def["children"][0]["children"][0];
        assert_eq!(clip["offset"], 60.0, "4 + 0 + 2 beats, in units");
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

    /// **A track is one clip with a roll in it**, which is what a track is in
    /// every editor — and what a document of clangs has to become to be read as
    /// music rather than as a row of empty rectangles.
    #[test]
    fn an_aggregate_of_clangs_is_one_clip_carrying_the_notes() {
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
        let lane = &drawn.def["children"][0];
        let clips = lane["children"].as_array().expect("a lane of clips");
        assert_eq!(clips.len(), 1, "one clip, not one per note");

        // The notes ride in the clip's **own** axis, so their starts are
        // relative to it: the second note is two beats in, not two beats from
        // the window's origin.
        let notes = clips[0]["notes"].as_array().expect("a roll");
        assert_eq!(notes.len(), 10, "five numbers a note: {notes:?}");
        assert_eq!(notes[0], 0.0, "the first note's start");
        assert_eq!(notes[2], 72.0, "and its pitch");
        assert_eq!(notes[5], 200.0, "the second note, two beats in");
        assert_eq!(notes[7], 76.0);
        assert_eq!(
            clips[0]["dur"], 300.0,
            "the clip spans to the last note's end"
        );

        // The clip draws the **aggregate**, so a drag on it moves the track; a note
        // inside edits through the roll's own payload.
        assert_eq!(drawn.bindings.len(), 1);
        assert_eq!(drawn.bindings[0].node, NodeId(2));
    }

    /// An aggregate that is **not** all pitched clangs stays a lane of clips: a roll
    /// drawn over half an aggregate would leave samples out without saying so.
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
        let clips = drawn.def["children"][0]["children"].as_array().unwrap();
        assert_eq!(clips.len(), 2, "one clip each, and no roll");
        assert!(clips[0].get("notes").is_none());
    }
}
