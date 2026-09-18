//! The host as the **owner** of what it draws — the third writer.
//!
//! Every gesture in this host emits an *intent* and waits for somebody to apply
//! it: a script drives the window, edits the document it holds, and pushes back
//! what now stands. That is the whole design and it does not change here. What
//! changes is who the somebody is: a standalone host has **no language client
//! at all**, so it must be its own owner, and this module is that role.
//!
//! It owns nothing new. The document, the intent vocabulary, the log with its
//! inverses and the session format are all `clausters_document`'s, built for
//! exactly this and used by the Python client already. What lives here is the
//! **wiring**: which node a widget's gesture addresses, turning the flat
//! `/gui_event` payload into an [`Intent`], applying it through the log so it
//! can be undone, and answering with what the document now says.
//!
//! # Why this is not a second implementation
//!
//! The alternative would be a host that edits its own render tree and calls
//! that the document — which is what the D track's premise forbids, and for a
//! reason this milestone makes concrete: a session written here has to open in
//! the Python client and come back unchanged. Two implementations of one format
//! is a format that drifts, and the whole point of the crate is that there is
//! one. So the host applies the *crate's* intents through the *crate's* log,
//! and what it adds is a map from widget id to node id and nothing else.
//!
//! # What an owner answers
//!
//! An intent applied here is acknowledged here: the outbox's stamp is retired
//! and the pending drawing dropped by the same rule a script's acknowledgement
//! would follow ([`super::ack`]). There is no branch for a refusal — a refused
//! edit is the previous value handed back, which is the crate's decision and
//! the reason the caller can adopt the outcome unconditionally.

pub mod multitrack;
pub mod sources;
pub mod tree;

use std::collections::HashMap;

use clausters_apps::editing::{Editing, Effect, Member, MemberId, Stepped};
use clausters_apps::multitrack::editor::MultitrackEditor;
use clausters_core::osc::OscType;
use clausters_document::clipboard::decode_samples;
use clausters_document::history::Direction;
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::{
    Against, Document, Intent, NodeId, Opaque, Outcome, Rules, Session, TimeUnit, apply_logged_in,
    log::TREE,
};
use serde_json::Value;

/// An OSC argument that may have been sent as a float or as an int.
fn float_at(args: &[OscType], n: usize) -> Option<f32> {
    match args.get(n) {
        Some(OscType::Float(f)) => Some(*f),
        Some(OscType::Int(i)) => Some(*i as f32),
        _ => None,
    }
}

/// A sample position: a long, or an int from a client that had no long to hand.
/// **Which channel a destructive payload addressed** — argument 1 of both
/// `"sample"` and `"draw"`, 0 for a payload that names none.
///
/// A missing or negative channel is the first one rather than a refusal: a mono
/// view has one channel and the hand cannot be over another.
fn channel_at(args: &[OscType]) -> u32 {
    match args.get(1) {
        Some(OscType::Int(ch)) if *ch >= 0 => *ch as u32,
        _ => 0,
    }
}

/// A flag as any of the three ways a client may have written it.
fn truthy_at(args: &[OscType], n: usize) -> bool {
    match args.get(n) {
        Some(OscType::Int(v)) => *v != 0,
        Some(OscType::Long(v)) => *v != 0,
        Some(OscType::Bool(v)) => *v,
        Some(OscType::Float(v)) => *v != 0.0,
        Some(OscType::Double(v)) => *v != 0.0,
        _ => false,
    }
}

fn long_at(args: &[OscType], n: usize) -> Option<u64> {
    match args.get(n) {
        Some(OscType::Long(v)) if *v >= 0 => Some(*v as u64),
        Some(OscType::Int(v)) if *v >= 0 => Some(*v as u64),
        _ => None,
    }
}

/// What the host holds when it is the one answering its own gestures.
pub struct Owner {
    /// The document, as the crate keeps it.
    ///
    /// **The leg being walked off**, and the crate says so: a session written
    /// today carries [`Owner::multitrack`] and leaves this empty. It stays because
    /// what it holds — the general tree, its samples, its destructive edits —
    /// has nowhere else to be yet.
    pub document: Document,
    /// **The multitrack**: the tracks and the timeline they sit on, which is what a
    /// session written today actually carries.
    ///
    /// Two descriptions, one owner, and the picture comes from whichever is
    /// filled ([`Owner::draws_multitrack`]). They are separate structures in one
    /// history, so a multitrack's edit and a tree's edit undo in the order they were
    /// made rather than in two orders.
    pub multitrack: Multitrack,
    /// **The editing context**: the one undo order the tree, the multitrack and the
    /// multitrack editor share — the applications crate's, as a client's is, so
    /// an inverse is read out of what was edited rather than remembered by the
    /// gesture that made it, and a multitrack's edit and a tree's undo in the order
    /// they were made.
    pub editing: Editing,
    /// The session this document came from, when it came from one: the sources
    /// its samples live in, which is what a save has to write back.
    pub session: Option<Session>,
    /// How an edit is transformed on the way in (the grid a placement snaps
    /// to). The host states where the hand put something; this decides.
    pub rules: Rules,
    /// Samples per beat, as the tree was drawn with: the document measures
    /// beats and a clip's `offset`/`dur` are timeline units, so adopting an
    /// applied edit back onto the picture needs the same factor the drawing
    /// used. Getting it from anywhere else would put the clip somewhere the
    /// ruler does not agree with.
    pub units_per_beat: f64,
    /// Samples per second, as the tree was drawn with: the length of data
    /// measured in seconds (a take's) meets the axis through the rate rather
    /// than through the tempo, so an edit-back that divided it by the beat
    /// would stretch every take by the tempo.
    pub units_per_second: f64,
    /// Where a save writes, when the caller named a file.
    ///
    /// `None` is a session opened read-only, or one built in memory: **saving
    /// over what you opened is a decision, not a default**, so a caller that
    /// wants it says where.
    pub save_path: Option<std::path::PathBuf>,
    /// The session's samples, when somebody resolved them: what a take's own
    /// length is, for the one case a placement does not state it.
    ///
    /// Held for the same reason [`Self::units_per_beat`] is — adopting an
    /// applied edit back onto the picture has to reach the rule the drawing
    /// used, and that rule ends at the samples.
    pub takes: sources::Takes,
    /// Which document node each widget's gestures address.
    ///
    /// The one thing this module adds to the crate, and the one thing only a
    /// host can know: a widget is a picture *of* a node, and an intent names
    /// the node. Nothing infers it — the tree that built the widgets records
    /// it, so a picture and the samples under it cannot drift apart.
    nodes: HashMap<i32, NodeId>,
    /// And which node each **lane header** configures. See
    /// [`Owner::bind_header`] for why it is not the same map.
    headers: HashMap<i32, NodeId>,
    /// The tree, as a member of [`Owner::editing`]: the crate does not apply the
    /// document's edits, so a step hands its payloads back to be applied here.
    tree: MemberId,
    /// The multitrack, as a member of [`Owner::editing`] — joined whether or not a
    /// window is ever opened over it, so its identity does not depend on what
    /// was edited first. A step hands its payloads back to be applied to
    /// [`Owner::multitrack`].
    multitrack_member: MemberId,
    /// The widget drawing the whole multitrack, when the tree has one.
    ///
    /// Not a map, because there is nothing to map: the multitrack names its
    /// lanes and its clips by the nodes' own numbers, so a payload is read
    /// without asking anything which widget it came from. What the id is for is
    /// the other direction — writing an applied edit back onto the picture.
    multitrack_widget: Option<i32>,
    /// **The multitrack editor** over the multitrack, once a window has been opened
    /// for it ([`Owner::open_editor`]): a member of [`Owner::editing`] under the
    /// multitrack's key, so it is the same structure in the order as the multitrack.
    editor_member: Option<MemberId>,
}

/// What applying an edit left behind, for the caller to draw and answer with.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// The edit describing the **tree** as it now stands — the intent as given
    /// when it applied verbatim, the transformed one when it was snapped, and
    /// the **previous** value when it was refused.
    ///
    /// `None` for an edit written in the **multitrack's** vocabulary, which is not
    /// an [`Intent`] and has nobody here to answer for it: the picture is
    /// redrawn from the owner rather than patched from what an edit said, so
    /// the only caller left that reads this is the one restoring samples
    /// ([`super::Host::replay_writes`]), and samples are the tree's.
    pub effective: Option<Intent>,
    /// The document's version afterwards, which is what an acknowledgement
    /// carries so a later edit can say what it was made against.
    pub version: u64,
    /// Whether anything actually moved. A refusal is not an error — it is the
    /// previous value — but nothing needs redrawing for one.
    pub applied: bool,
}

impl Owner {
    /// An owner of `document`, with no session behind it (a multitrack built
    /// in memory) and no grid.
    pub fn new(document: Document) -> Self {
        let mut editing = Editing::new();
        // The tree, and the multitrack as the second structure in the same order,
        // joined whether or not this owner turns out to hold a multitrack: one
        // history is what makes an undo walk the two descriptions in the order
        // the hand made them, and joining lazily would mean an id that depends
        // on what was edited first.
        let tree = editing.join(
            "tree",
            Member::External {
                domain: TREE.into(),
            },
        );
        let multitrack_member = editing.join(
            "multitrack",
            Member::External {
                domain: MULTITRACK.into(),
            },
        );
        Self {
            document,
            multitrack: Multitrack::default(),
            editing,
            tree,
            multitrack_member,
            session: None,
            rules: Rules::none(),
            units_per_beat: 48_000.0,
            units_per_second: 48_000.0,
            save_path: None,
            takes: sources::Takes::default(),
            nodes: HashMap::new(),
            headers: HashMap::new(),
            multitrack_widget: None,
            editor_member: None,
        }
    }

    /// Whether the picture comes from the **multitrack** rather than from the tree.
    ///
    /// Read off what the session actually carries rather than from a flag a
    /// caller sets: a session written today has tracks and an empty document,
    /// one written before the turn has the other, and a host that asked which
    /// mode it was in would be asking the caller to know something the file
    /// already says.
    pub fn draws_multitrack(&self) -> bool {
        !self.multitrack.tracks.is_empty()
    }

    /// An owner of a session's document, keeping the session so a save has the
    /// sources to write with it.
    pub fn from_session(session: Session) -> Self {
        let mut owner = Self::new(session.document.clone());
        owner.multitrack = session.multitrack.clone();
        owner.session = Some(session);
        owner
    }

    /// The unit the picture was drawn in (samples per beat).
    pub fn with_units_per_beat(mut self, units: f64) -> Self {
        self.units_per_beat = units;
        self
    }

    /// The rate the picture was drawn at (samples per second), for the lengths
    /// that are in seconds.
    pub fn with_units_per_second(mut self, units: f64) -> Self {
        self.units_per_second = units;
        self
    }

    /// The scales the drawing used, as the tree's own `Look` — the one place
    /// that turns a length in its own data's unit into units on the axis.
    pub(crate) fn look(&self) -> tree::Look<'_> {
        tree::Look {
            units_per_beat: self.units_per_beat,
            units_per_second: self.units_per_second,
            takes: Some(&self.takes),
            ..tree::Look::default()
        }
    }

    /// The samples this document's sources resolved to, for the length rule.
    pub fn with_takes(mut self, takes: sources::Takes) -> Self {
        self.takes = takes;
        self
    }

    /// The **placement** of a node — the member that holds it, which is where a
    /// length lives.
    ///
    /// A node knows what it is; only its member knows how long it is placed
    /// for, and that is what a picture is drawn from. Read out of the document
    /// *after* an edit has landed, so it is the state that now holds rather
    /// than what an intent said about it.
    pub fn member_of(&self, node: NodeId) -> Option<&clausters_document::Member> {
        fn walk(n: &clausters_document::Node, id: NodeId) -> Option<&clausters_document::Member> {
            for m in n.members() {
                if m.node.id == id {
                    return Some(m);
                }
                if let Some(found) = walk(&m.node, id) {
                    return Some(found);
                }
            }
            None
        }
        walk(&self.document.root, node)
    }

    /// Which widget draws `node`, if one does — the binding read the other way,
    /// which is what adopting an applied edit needs.
    pub fn widget_of(&self, node: NodeId) -> Option<i32> {
        self.nodes
            .iter()
            .find_map(|(widget, bound)| (*bound == node).then_some(*widget))
    }

    /// Where [`Self::save_now`] writes.
    pub fn saving_to(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.save_path = Some(path.into());
        self
    }

    /// Writes to [`Self::save_path`], or reports that there is nowhere to write.
    pub fn save_now(&self) -> Result<&std::path::Path, String> {
        let path = self
            .save_path
            .as_deref()
            .ok_or_else(|| "this session has nowhere to save to".to_string())?;
        self.save(path)?;
        Ok(path)
    }

    /// Snapping placements to a grid of `quant` beats (0 snaps nothing).
    pub fn with_quant(mut self, quant: f64) -> Self {
        self.rules = Rules { quant };
        self
    }

    /// Opens a session file — the format the Python client writes, read by the
    /// crate and not by a parser of this host's own, which is the whole reason
    /// the format has one implementation.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| format!("{}: {e}", path.as_ref().display()))?;
        // Read through the crate's door, so a session written in an older
        // format opens as this one writes it.
        let session =
            Session::read_str(&text).map_err(|e| format!("{}: {e}", path.as_ref().display()))?;
        Ok(Self::from_session(session))
    }

    /// Writes the session back, with the document as it now stands.
    ///
    /// The sources travel unchanged: what an editing session edits is the
    /// arrangement and the samples, and where the samples *lives* is the
    /// session's own bookkeeping, which this host has no business rewriting.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), String> {
        let mut session = self
            .session
            .clone()
            .unwrap_or_else(|| Session::new(self.document.clone()));
        session.document = self.document.clone();
        session.multitrack = self.multitrack.clone();
        let text = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
        std::fs::write(path.as_ref(), text).map_err(|e| format!("{}: {e}", path.as_ref().display()))
    }

    /// Says which node a widget's gestures address. The tree that built the
    /// widgets is what calls this; nothing guesses.
    pub fn bind(&mut self, widget_id: i32, node: NodeId) {
        self.nodes.insert(widget_id, node);
    }

    /// Says which node a **lane header's** gestures address.
    ///
    /// A second map rather than a second entry in the first, because one node
    /// can be drawn twice: a lane holding a single element binds that element
    /// for its header *and* for the clip inside it. Both directions have to
    /// stay answerable — a gesture asks which node a widget is (either map
    /// does), and an applied edit asks which widget a node is, where "the clip"
    /// and "the header" are different questions with different answers.
    pub fn bind_header(&mut self, widget_id: i32, node: NodeId) {
        self.headers.insert(widget_id, node);
    }

    /// Forgets a widget — a window closing, or a tree rebuilt.
    pub fn unbind(&mut self, widget_id: i32) {
        self.nodes.remove(&widget_id);
        self.headers.remove(&widget_id);
    }

    /// The node a widget addresses, if it addresses one.
    pub fn node_of(&self, widget_id: i32) -> Option<NodeId> {
        self.nodes
            .get(&widget_id)
            .or_else(|| self.headers.get(&widget_id))
            .copied()
    }

    /// The lane header drawing this node, if one does.
    pub fn header_of(&self, node: NodeId) -> Option<i32> {
        self.headers
            .iter()
            .find_map(|(widget, bound)| (*bound == node).then_some(*widget))
    }

    /// Says which widget draws **the whole multitrack**.
    ///
    /// The multitrack needs no per-clip binding — a clip's name on the wire is
    /// its node's number — so what is recorded is only the widget id, and only
    /// because an applied edit has to be written back onto *some* widget.
    pub fn bind_multitrack(&mut self, widget_id: i32) {
        self.multitrack_widget = Some(widget_id);
    }

    /// The widget drawing the multitrack, if one does.
    pub fn multitrack_widget(&self) -> Option<i32> {
        self.multitrack_widget
    }

    /// **What is on screen**, in the widget's own vocabulary — the lanes and
    /// clips a `/gui_set` would carry, and the ids behind them.
    ///
    /// Re-derived rather than remembered: the owner is the state, and a second
    /// copy of the picture kept beside it is a second copy to keep in step. It
    /// is the same walk the window was drawn with, so what an edit-back is
    /// resolved against cannot disagree with what a hand moved.
    ///
    /// It comes from the **multitrack** when there is one and from the tree
    /// otherwise ([`Self::draws_multitrack`]) — one widget, two descriptions, and
    /// the file says which.
    pub fn shown(&self) -> tree::Picture {
        if self.draws_multitrack() {
            multitrack::shown(&self.multitrack, &self.multitrack_look())
        } else {
            tree::multitrack(&self.document, &self.look())
        }
    }

    /// The scales the multitrack is drawn with.
    pub fn multitrack_look(&self) -> multitrack::Look<'_> {
        multitrack::Look {
            rate: self.units_per_second,
            takes: Some(&self.takes),
            sources: self.session.as_ref().map(|session| &session.sources),
        }
    }

    /// Reads a widget's `/gui_event` payload as **the edits it stands for**.
    ///
    /// The plural door, and the one the multitrack comes through. A payload
    /// that states the multitrack — `"clips"`, `"lanes"` — is one message describing
    /// every box or every strip, so what it means is however many intents it
    /// takes to make the document say that; a payload that states one thing is
    /// one intent, and goes through [`Self::read_event`] unchanged.
    ///
    /// **They are one transaction.** A block move is one thing a hand did, so
    /// `Ctrl`+`Z` walks back over all of it at once — which is why this returns
    /// the run rather than the caller applying them one at a time.
    pub fn read_events(&self, widget_id: i32, args: &[OscType]) -> Vec<(Intent, &'static str)> {
        match args.first() {
            Some(OscType::String(tag)) if tag == "clips" && !self.draws_multitrack() => {
                self.read_clips(&args[1..])
            }
            Some(OscType::String(tag)) if tag == "lanes" && !self.draws_multitrack() => {
                self.read_lanes(&args[1..])
            }
            _ => self.read_event(widget_id, args).into_iter().collect(),
        }
    }

    /// **Opens the multitrack editor over the multitrack** — the one a script and a
    /// page open — and answers its window, as the GuiDef to define as `window`.
    ///
    /// The multitrack is drawn at `window + 1` and ruled at `window + 2`, and the
    /// transport row is numbered after them: a host composing a window for
    /// itself has nobody to number it on the way out. From here on a gesture on
    /// either widget is the editor's turn ([`super::Host::answer_own`]).
    pub fn open_editor(&mut self, window: i32, title: &str, size: (i64, i64)) -> serde_json::Value {
        use clausters_apps::multitrack::{Transport, TransportIds};

        let mut editor = MultitrackEditor::new(
            self.multitrack.clone(),
            self.units_per_second,
            self.editing.version(),
        );
        editor.chrome(
            None,
            Transport::Numbered(TransportIds {
                row: window + 3,
                rewind: window + 4,
                play: window + 5,
                stop: window + 6,
                clock: window + 7,
            }),
            title,
            size,
        );
        editor.set_sources(self.buffer_table());
        editor.set_lengths(self.buffer_lengths());
        editor.set_segments(self.segments());
        let def = editor.window(window + 1, window + 2);
        editor.set_window(Some(window));
        // **One editor over the multitrack**: opening the window again replaces the
        // editor in its seat rather than seating a second one.
        match self.editor_member {
            Some(member) => {
                if let Some(Member::Multitrack(held)) = self.editing.member_mut(member) {
                    **held = editor;
                }
            }
            None => {
                self.editor_member = Some(
                    self.editing
                        .join("multitrack", Member::Multitrack(Box::new(editor))),
                );
            }
        }
        self.bind_multitrack(window + 1);
        def
    }

    /// The multitrack editor over the multitrack, once a window has been opened for
    /// it.
    pub fn editor(&self) -> Option<&MultitrackEditor> {
        match self.editing.member(self.editor_member?)? {
            Member::Multitrack(editor) => Some(editor),
            _ => None,
        }
    }

    /// The multitrack editor, to hand it what the owner holds.
    pub fn editor_mut(&mut self) -> Option<&mut MultitrackEditor> {
        match self.editing.member_mut(self.editor_member?)? {
            Member::Multitrack(editor) => Some(editor),
            _ => None,
        }
    }

    /// The editor's member in [`Owner::editing`], once there is one.
    pub fn editor_member(&self) -> Option<MemberId> {
        self.editor_member
    }

    /// **The joins the session holds**, by source: the segments each is made
    /// of, which the multitrack editor reads through when a hand joins a box
    /// that is itself a join.
    pub fn segments(
        &self,
    ) -> HashMap<clausters_document::SourceId, Vec<clausters_document::session::Part>> {
        self.session
            .as_ref()
            .map(|session| {
                session
                    .sources
                    .iter()
                    .filter_map(|(id, source)| match &source.location {
                        clausters_document::session::Location::Segments { parts } => {
                            Some((*id, parts.clone()))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Which server buffer each of the session's sources was read into.
    pub fn buffer_table(&self) -> HashMap<clausters_document::SourceId, i64> {
        self.takes
            .iter()
            .map(|(id, take)| (*id, i64::from(take.bufnum)))
            .collect()
    }

    /// **How many frames each take holds**, where the session said: what lets
    /// the editor refuse a join over a box that reads past its take.
    pub fn buffer_lengths(&self) -> HashMap<clausters_document::SourceId, u64> {
        self.takes
            .iter()
            .filter_map(|(id, take)| take.frames.filter(|f| *f > 0).map(|f| (*id, f)))
            .collect()
    }

    /// **Writes down how long each take is, where nothing said**, asking
    /// `frames_of` by buffer number once the samples are read. Both the take
    /// table and the session's own source entry learn it, so a save states the
    /// length a join is bounded by and a reader of the file does not have to
    /// open the file to know it. A length already stated is kept. Returns how
    /// many takes learned one.
    pub fn learn_lengths(&mut self, frames_of: impl Fn(i32) -> Option<u64>) -> usize {
        let learned: Vec<(clausters_document::SourceId, i32, Option<u32>, u64)> = self
            .takes
            .iter()
            .filter(|(_, take)| take.frames.is_none_or(|f| f == 0))
            .filter_map(|(id, take)| {
                let frames = frames_of(take.bufnum).filter(|f| *f > 0)?;
                Some((*id, take.bufnum, take.channels, frames))
            })
            .collect();
        for &(id, bufnum, channels, frames) in &learned {
            if let Some(entry) = self.session.as_mut().and_then(|s| s.sources.get_mut(&id))
                && entry.frames.is_none_or(|f| f == 0)
            {
                entry.frames = Some(frames);
            }
            self.takes.insert(
                id,
                sources::Take {
                    bufnum,
                    channels,
                    frames: Some(frames),
                },
            );
        }
        learned.len()
    }

    /// **The multitrack's clips, as they now stand** — the one payload every
    /// placement gesture leaves.
    ///
    /// A move, a trim, a block drag and a lane change all arrive here, and
    /// nothing in the payload says which of them it was: what is compared is
    /// the list against the document, and what comes out is the difference.
    /// That is the whole reason the widget reports the multitrack — the reader has
    /// no case to get wrong.
    ///
    /// Two shapes come out of it. A clip that stayed on its lane is a
    /// [`Intent::Place`], which is what a placement is. A clip that **crossed**
    /// is not a placement at all — it left one aggregate and joined another —
    /// so it is a pair of [`Intent::SetMembers`], one per aggregate, stating
    /// what each now holds. Both are absolute, so applying the run twice leaves
    /// the same multitrack.
    fn read_clips(&self, args: &[OscType]) -> Vec<(Intent, &'static str)> {
        let units = self.units_per_beat.max(f64::MIN_POSITIVE);
        let now = tree::multitrack(&self.document, &self.look());
        // What each aggregate ends up holding, for the clips that crossed.
        let mut leaving: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut joining: HashMap<NodeId, Vec<clausters_document::Member>> = HashMap::new();
        let mut placed = Vec::new();
        for clip in args.as_chunks::<7>().0 {
            let (Some(OscType::String(name)), Some(OscType::String(lane))) =
                (clip.first(), clip.get(1))
            else {
                continue;
            };
            let (Some(node), Some(lane)) = (tree::node_named(name), tree::node_named(lane)) else {
                continue;
            };
            // A clip naming a lane the document has none of is **kept where it
            // is** rather than dropped: the widget hands back what it could not
            // place so it can be re-homed, and a reader that acted on it would
            // be moving a box into a container that does not exist.
            let Some(dest) = now.lane(lane) else { continue };
            let Some(was) = now.lane_of(node) else {
                continue;
            };
            let offset = float_at(clip, 2).unwrap_or(0.0) as f64 / units;
            // **And the length leaves by the unit of its own data.** A take's
            // seconds are a wall-clock fact; dividing them by the beat would
            // write a length that the next tempo change moves.
            let per_length = match self.document.find(node).map(|n| n.duration_unit()) {
                Some(TimeUnit::Seconds) => self.units_per_second.max(f64::MIN_POSITIVE),
                _ => units,
            };
            let dur = float_at(clip, 3)
                .map(|d| d as f64 / per_length)
                .filter(|d| *d > 0.0);
            if was.holder == dest.holder {
                // Same aggregate: a placement, whatever the picture called it.
                placed.push((node, offset - dest.base, dur));
                continue;
            }
            let Some(held) = self.document.find(node).cloned() else {
                continue;
            };
            leaving.entry(was.holder).or_default().push(node);
            joining
                .entry(dest.holder)
                .or_default()
                .push(clausters_document::Member {
                    offset: offset - dest.base,
                    dur,
                    node: held,
                });
        }
        let mut out: Vec<(Intent, &'static str)> = Vec::new();
        for holder in leaving
            .keys()
            .chain(joining.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
        {
            let Some(members) = self.members_of(holder) else {
                continue;
            };
            let gone = leaving.get(&holder).cloned().unwrap_or_default();
            let mut members: Vec<clausters_document::Member> = members
                .iter()
                .filter(|m| !gone.contains(&m.node.id))
                .cloned()
                .collect();
            members.extend(joining.get(&holder).cloned().unwrap_or_default());
            out.push((
                Intent::SetMembers {
                    node: holder,
                    members,
                },
                "move a clip to another lane",
            ));
        }
        out.extend(
            placed
                .into_iter()
                .map(|(node, offset, dur)| (Intent::Place { node, offset, dur }, "move a clip")),
        );
        out
    }

    /// **The multitrack's lanes, as they now stand** — the mixer's payload.
    ///
    /// Separate from the clips for the reason they are two structures: a fader
    /// moved must not resend every clip. What is written is the *element's*
    /// configuration, so a mute survives a save and undoes like a move — the
    /// same [`Intent::Configure`] a script emits, which is why the undo comes
    /// out of the document identically whoever made the edit.
    ///
    /// A configuration is replaced **whole**, so each starts from what the node
    /// already carries and writes the three keys over it; a lane whose strip
    /// says what the document already says is not an edit, which is what keeps
    /// a drag on one fader from logging every other lane.
    fn read_lanes(&self, args: &[OscType]) -> Vec<(Intent, &'static str)> {
        let mut out = Vec::new();
        for lane in args.as_chunks::<7>().0 {
            let Some(OscType::String(name)) = lane.first() else {
                continue;
            };
            let Some(node) = tree::node_named(name) else {
                continue;
            };
            let Some(held) = self.document.find(node) else {
                continue;
            };
            let held_config = held
                .body
                .config()
                .and_then(|c| c.0.as_object().cloned())
                .unwrap_or_default();
            let (mute, solo, level) = (
                truthy_at(lane, 3),
                truthy_at(lane, 4),
                float_at(lane, 5).unwrap_or(1.0) as f64,
            );
            // **Compared against what the lane was drawn with**, not against
            // the keys the configuration happens to hold: a lane that says
            // nothing is drawn audible at unity, so a strip reporting exactly
            // that is not an edit. Comparing the raw tables instead would make
            // the first fader drag log a `Configure` for every other lane --
            // and one undo per lane to take it back.
            let held_flag = |key: &str| {
                held_config
                    .get(key)
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            };
            let held_level = held_config
                .get("level")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            if (held_flag("mute"), held_flag("solo"), held_level) == (mute, solo, level) {
                continue;
            }
            let mut config = held_config;
            config.insert("mute".into(), Value::from(mute));
            config.insert("solo".into(), Value::from(solo));
            config.insert("level".into(), Value::from(level));
            out.push((
                Intent::Configure {
                    node,
                    config: Opaque(Value::Object(config)),
                },
                "mix a lane",
            ));
        }
        out
    }

    /// The members of an aggregate, wherever it sits.
    fn members_of(&self, node: NodeId) -> Option<&[clausters_document::Member]> {
        self.document
            .find(node)
            .map(clausters_document::Node::members)
    }

    /// Applies a run of intents as **one entry** in the log, so a block move
    /// undoes the way it was made.
    ///
    /// The inverse of each is read out of the document *before* that one lands,
    /// which is the same rule [`apply_logged_in`] follows — it is spelled out here
    /// only because there is no one-call form for a transaction.
    pub fn apply_all(
        &mut self,
        intents: &[(Intent, &'static str)],
        against: &Against,
    ) -> Vec<Applied> {
        use clausters_document::log::{Entry, Step, inverse_of};

        let mut entry: Option<Entry> = None;
        let mut out = Vec::with_capacity(intents.len());
        for (intent, label) in intents {
            let backward = inverse_of(&self.document, intent);
            let outcome =
                clausters_document::apply(&mut self.document, intent, against, &self.rules);
            if outcome.applied
                && let Some(backward) = backward
            {
                let forward = Step::Edit(outcome.effective.clone());
                entry = Some(match entry.take() {
                    Some(e) => e.and(forward, backward),
                    None => Entry::new(*label, forward, backward),
                });
            }
            out.push(self.report(outcome));
        }
        if let (Some(entry), Some(structure)) = (entry, self.editing.structure(self.tree)) {
            self.editing.record_entry(entry.generic(structure));
        }
        out
    }

    /// Reads a widget's `/gui_event` payload as an edit to the document, with
    /// the label an undo stack would show for it.
    ///
    /// **This is the translation and nothing more.** The payload's vocabulary
    /// is the gesture's — flat OSC primitives, in the owner's terms rather than
    /// the screen's — and the document's is the crate's; what a host adds is
    /// knowing which node the widget was drawing. A payload it does not
    /// recognize, or one on a widget bound to no node, is `None`: an owner that
    /// invented an intent for an event it did not understand would be editing
    /// on a guess.
    pub fn read_event(&self, widget_id: i32, args: &[OscType]) -> Option<(Intent, &'static str)> {
        let node = self.node_of(widget_id)?;
        let tag = match args.first() {
            Some(OscType::String(tag)) => tag.as_str(),
            _ => return None,
        };
        match tag {
            // A clip moved or resized: where it now sits inside the aggregate that
            // holds it. Absolute, so applying it twice is applying it once.
            // **The payload is in timeline units and the document is in
            // beats**, so this is where the two meet. A clip reports where the
            // hand put it on the shared axis, which measures samples; a
            // placement is musical time. Forgetting the conversion does not
            // fail — it writes the sample number into the beat field, so a clip
            // dropped two beats along is saved at beat ninety-six thousand.
            "clip" => {
                let units = self.units_per_beat.max(f64::MIN_POSITIVE);
                let offset = float_at(args, 1)? as f64 / units;
                // **And the length leaves by the unit of its own data.** A
                // take's seconds are a wall-clock fact; dividing them by the
                // beat would write a length that the next tempo change moves.
                let per_length = match self.document.find(node).map(|n| n.duration_unit()) {
                    Some(TimeUnit::Seconds) => self.units_per_second.max(f64::MIN_POSITIVE),
                    _ => units,
                };
                let dur = float_at(args, 2)
                    .map(|d| d as f64 / per_length)
                    .filter(|d| *d > 0.0);
                Some((Intent::Place { node, offset, dur }, "move a clip"))
            }

            // A lane header's toggle or fader. **The document's**, not the
            // window's: what is muted is a fact about the multitrack, so it goes
            // through the log like a clip's move and survives a save. It is the
            // same `Configure` a client emits, which is why the undo comes out
            // of the document identically whoever made the edit.
            //
            // A configuration is replaced **whole**, so this starts from what
            // the node already carries and writes one key over it — the rule
            // the intent states, and the reason a fader cannot quietly erase a
            // mute.
            "mute" | "solo" | "level" => {
                let value = match (tag, args.get(1)) {
                    ("level", _) => Value::from(float_at(args, 1)? as f64),
                    (_, Some(OscType::Int(flag))) => Value::from(*flag != 0),
                    (_, Some(OscType::Bool(flag))) => Value::from(*flag),
                    (_, Some(OscType::Float(flag))) => Value::from(*flag != 0.0),
                    _ => return None,
                };
                let mut config = self
                    .document
                    .find(node)
                    .and_then(|n| n.body.config())
                    .and_then(|c| c.0.as_object().cloned())
                    .unwrap_or_default();
                config.insert(tag.into(), value);
                Some((
                    Intent::Configure {
                        node,
                        config: Opaque(Value::Object(config)),
                    },
                    match tag {
                        "mute" => "mute the lane",
                        "solo" => "solo the lane",
                        _ => "level the lane",
                    },
                ))
            }
            // **A stroke and a single sample are one verb**, and the reading
            // is the projection's (`clausters_editing::samples`): what the run
            // is, whether there is one at all, and what the undo stack calls
            // it. The host held its own copy of those rules and was short two
            // of them -- it recorded an empty stroke as an edit that undoes to
            // itself, and an inverse that did not cover the write's span as one
            // that did.
            "sample" | "draw" => {
                let written = self.read_write(args)?;
                let label = written.label;
                Some((self.written_intent(node, &written, false)?, label))
            }
            _ => None,
        }
    }

    /// **What a `sample` or a `draw` report came to**, read once for the two
    /// doors that ask about it.
    ///
    /// The blob is the whole reason this is here rather than a call into the
    /// crate's JSON door: a stroke's run rides the wire as little-endian `f32`,
    /// and turning it into a JSON array so the reading could be asked for would
    /// be a conversion in each direction, on the one editing path whose payload
    /// is large by design. So the host decodes the wire and the *rule* is the
    /// crate's ([`clausters_editing::samples::write`]).
    fn read_write(&self, args: &[OscType]) -> Option<clausters_editing::samples::Write> {
        let tag = match args.first() {
            Some(OscType::String(tag)) => tag.as_str(),
            _ => return None,
        };
        let run = |at: usize| -> Vec<f64> {
            match args.get(at) {
                Some(OscType::Blob(bytes)) => decode_samples(bytes)
                    .iter()
                    .map(|v| f64::from(*v))
                    .collect(),
                Some(OscType::Float(v)) => vec![f64::from(*v)],
                Some(OscType::Double(v)) => vec![*v],
                _ => Vec::new(),
            }
        };
        clausters_editing::samples::write(
            tag,
            i64::from(channel_at(args)),
            long_at(args, 2)? as i64,
            &run(3),
            &run(4),
        )
    }

    /// The reading as the intent it is, forward or inverted — `None` where the
    /// run asked for is not there (an inverse the payload did not carry).
    fn written_intent(
        &self,
        node: NodeId,
        written: &clausters_editing::samples::Write,
        inverse: bool,
    ) -> Option<Intent> {
        let values = match inverse {
            true => written.previous.as_ref()?,
            false => &written.values,
        };
        Some(Intent::WriteSamples {
            node,
            channel: written.channel,
            start: written.start,
            values: values.iter().map(|v| *v as f32).collect(),
        })
    }

    /// The **inverse the payload carries**, for the one edit whose inverse the
    /// document does not hold.
    ///
    /// A destructive write's previous samples are the host's to report because
    /// the host was drawing them: `"sample"` carries the value it replaced and
    /// `"draw"` carries the run, as the second of its two blobs. A payload
    /// without that half gives `None`, and the caller logs what the document
    /// can — which is an edit that redoes but does not undo, and is why the
    /// gesture sends both.
    pub fn read_inverse(&self, widget_id: i32, args: &[OscType]) -> Option<Intent> {
        let node = self.node_of(widget_id)?;
        let written = self.read_write(args)?;
        self.written_intent(node, &written, true)
    }

    /// Applies one intent through the log, so it can be undone.
    ///
    /// `label` is what the undo stack shows for it — the vocabulary a user
    /// reads ("draw", "move a clip"), not the wire's.
    pub fn apply(&mut self, intent: &Intent, against: &Against, label: &str) -> Applied {
        let (document, rules) = (&mut self.document, &self.rules);
        let mut outcome = None;
        self.editing.record_with(self.tree, |history, structure| {
            let applied =
                apply_logged_in(document, intent, against, rules, history, structure, label);
            let recorded = applied.applied;
            outcome = Some(applied);
            recorded
        });
        // A tree that is not in the order records nothing and still edits.
        let outcome = match outcome {
            Some(outcome) => outcome,
            None => clausters_document::apply(&mut self.document, intent, against, &self.rules),
        };
        self.report(outcome)
    }

    /// Applies an edit whose inverse **the caller holds**, logging that one.
    ///
    /// There is exactly one such edit and the crate says so: a destructive
    /// write's previous samples are not in the document (the document describes
    /// where samples are, never what they hold), so `apply_logged` records an
    /// empty write as the inverse and an undo would restore nothing. What was
    /// there is known to whoever was **drawing** it — the gesture carried the
    /// span it painted over, which is why the payload has a `previous` half —
    /// and this is where that returns to the log.
    ///
    /// Everything else about it is the ordinary path: the same `apply`, the
    /// same outcome, the same entry shape. Only the backward step comes from
    /// the hand instead of from the document.
    pub fn apply_with_inverse(
        &mut self,
        intent: &Intent,
        inverse: &Intent,
        against: &Against,
        label: &str,
    ) -> Applied {
        use clausters_document::log::{Entry, Step};

        let outcome = clausters_document::apply(&mut self.document, intent, against, &self.rules);
        if outcome.applied
            && let Some(structure) = self.editing.structure(self.tree)
        {
            let entry = Entry::new(
                label,
                Step::Edit(outcome.effective.clone()),
                inverse.clone(),
            );
            self.editing.record_entry(entry.generic(structure));
        }
        self.report(outcome)
    }

    /// Undoes the last edit, returning what each inverse left. Empty when
    /// there is nothing to undo.
    ///
    /// The inverses are applied **without** logging: an undo is a walk through
    /// the log and not a new entry in it, which is what makes redo the other
    /// direction of one stack rather than a second one.
    pub fn undo(&mut self) -> Vec<Applied> {
        self.walk(Direction::Undo)
    }

    /// Redoes the last undone edit, in the direction it was made.
    pub fn redo(&mut self) -> Vec<Applied> {
        // What the walk leaves behind is `remaining`, which only its owner can
        // re-run -- and the host holds no algorithms, so a redo here applies
        // the ordinary edits and stops where the crate stopped.
        self.walk(Direction::Redo)
    }

    /// One step of the pile, in either direction.
    ///
    /// The step arrives **routed**: one run of payloads per structure, which is
    /// the same call the two clients make. Picking the side a direction reads
    /// and keeping the legs a structure owns are rules, and this is the third
    /// caller that would otherwise write them again.
    fn walk(&mut self, direction: Direction) -> Vec<Applied> {
        let stepped = self.editing.step(direction);
        self.carry(&stepped)
    }

    pub fn can_undo(&self) -> bool {
        self.editing.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.editing.can_redo()
    }

    /// **Carries out a step the context took**, on the descriptions this owner
    /// holds: the payloads the step hands back for the multitrack and for the tree,
    /// applied with the checks off — what the history holds is by definition
    /// against the state as it was left, and snapping something twice would
    /// move it. The multitrack editor's own copy of the multitrack was stepped by
    /// the context already.
    ///
    /// **Each effect says which member it is for**, which is the whole reason
    /// the two descriptions share one order: a multitrack's move and a tree's stroke
    /// walk back in the order the hand made them, not in two orders.
    pub fn carry(&mut self, stepped: &Stepped) -> Vec<Applied> {
        let mut out = Vec::new();
        if !stepped.stepped {
            return out;
        }
        for effect in &stepped.effects {
            let Effect::External { member, payloads } = effect else {
                continue;
            };
            for load in payloads {
                let load = Opaque(load.clone());
                if *member == self.multitrack_member {
                    let Some(intent) = clausters_document::multitrack::edit::intent_of(&load)
                    else {
                        continue;
                    };
                    let outcome = clausters_document::multitrack::edit::apply(
                        &mut self.multitrack,
                        &intent,
                        &Against::default(),
                        &Rules::none(),
                    );
                    out.push(Applied {
                        effective: None,
                        version: self.multitrack.version,
                        applied: outcome.applied,
                    });
                } else if *member == self.tree {
                    let Some(intent) = clausters_document::log::intent_of(&load) else {
                        continue;
                    };
                    let outcome = clausters_document::apply(
                        &mut self.document,
                        &intent,
                        &Against::default(),
                        &Rules::none(),
                    );
                    out.push(self.report(outcome));
                }
            }
        }
        out
    }

    fn report(&self, outcome: Outcome) -> Applied {
        Applied {
            effective: Some(outcome.effective),
            version: self.document.version,
            applied: outcome.applied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::{Body, Grouping, Member, Node, Opaque};

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

    #[test]
    fn an_edit_applies_through_the_log_and_undoes_out_of_the_document() {
        let root = aggregate(
            1,
            vec![Member {
                offset: 0.0,
                dur: None,
                node: clang(2),
            }],
        );
        let mut owner = Owner::new(Document::new(root));
        let before = owner.document.version;

        let applied = owner.apply(
            &Intent::Place {
                node: NodeId(2),
                offset: 4.0,
                dur: None,
            },
            &Against::default(),
            "move a clip",
        );
        assert!(applied.applied, "the edit landed");
        assert!(applied.version > before, "and the version moved with it");
        assert!(owner.can_undo(), "and it can be taken back");

        let undone = owner.undo();
        assert_eq!(undone.len(), 1, "one inverse for one edit");
        assert!(owner.can_redo(), "and put back again");
        // The inverse came out of the document, not out of the gesture: the
        // host never remembered where the clip was.
        assert!(
            matches!(undone[0].effective, Some(Intent::Place { offset, .. }) if offset == 0.0),
            "{:?}",
            undone[0].effective
        );
    }

    /// Undo and redo are two directions of **one** stack, which is what keeps
    /// a redone edit the same edit rather than a new one.
    #[test]
    fn redo_puts_back_what_undo_took() {
        let root = aggregate(
            1,
            vec![Member {
                offset: 0.0,
                dur: None,
                node: clang(2),
            }],
        );
        let mut owner = Owner::new(Document::new(root));
        owner.apply(
            &Intent::Place {
                node: NodeId(2),
                offset: 4.0,
                dur: None,
            },
            &Against::default(),
            "move a clip",
        );
        owner.undo();
        let redone = owner.redo();
        assert_eq!(redone.len(), 1);
        assert!(
            matches!(redone[0].effective, Some(Intent::Place { offset, .. }) if offset == 4.0),
            "{:?}",
            redone[0].effective
        );
        assert!(!owner.can_redo(), "and there is nothing further forward");
    }

    /// A lane header's toggle is the document's, so it travels the road a
    /// clip's move does: one `Configure`, through the log, undoable out of the
    /// document — the same intent a client emits, which is what makes the two
    /// undo alike.
    #[test]
    fn a_lane_header_configures_the_element_and_undoes() {
        let mut track = aggregate(2, vec![]);
        if let Body::Aggregate { config, .. } = &mut track.body {
            *config = Opaque(serde_json::json!({"level": 0.5}));
        }
        let mut owner = Owner::new(Document::new(aggregate(
            1,
            vec![Member {
                offset: 0.0,
                dur: None,
                node: track,
            }],
        )));
        owner.bind(70, NodeId(2));

        let args = [OscType::String("mute".into()), OscType::Int(1)];
        let (intent, label) = owner.read_event(70, &args).expect("a header is an edit");
        assert_eq!(label, "mute the lane");
        // **Whole, so it starts from what is there**: a mute that dropped the
        // level would be a fader nobody moved.
        match &intent {
            Intent::Configure { node, config } => {
                assert_eq!(*node, NodeId(2));
                assert_eq!(config.0["mute"], true);
                assert_eq!(config.0["level"], 0.5, "and the level it already had");
            }
            other => panic!("{other:?}"),
        }

        let applied = owner.apply(&intent, &Against::default(), label);
        assert!(applied.applied);
        let undone = owner.undo();
        assert_eq!(undone.len(), 1);
        match &undone[0].effective {
            Some(Intent::Configure { config, .. }) => assert!(
                config.0.get("mute").is_none_or(|v| v == false),
                "unmuted again: {config:?}"
            ),
            other => panic!("{other:?}"),
        }
    }

    /// The translation, and the two ways it declines: a payload it does not
    /// know, and a widget bound to no node. Either would be editing on a guess.
    #[test]
    fn a_payload_becomes_an_intent_only_where_it_can_be_read() {
        // One unit to the beat, so the payload's numbers *are* beats and the
        // test is about the translation rather than about the scale.
        let mut owner = Owner::new(Document::new(clang(1))).with_units_per_beat(1.0);
        let clip = vec![
            OscType::String("clip".into()),
            OscType::Float(4.0),
            OscType::Float(2.0),
        ];
        assert_eq!(
            owner.read_event(50, &clip),
            None,
            "a widget bound to no node addresses nothing"
        );
        owner.bind(50, NodeId(9));
        let (intent, label) = owner.read_event(50, &clip).expect("a clip moved");
        assert_eq!(label, "move a clip");
        assert!(
            matches!(intent, Intent::Place { node, offset, dur }
                     if node == NodeId(9) && offset == 4.0 && dur == Some(2.0)),
            "{intent:?}"
        );
        assert_eq!(
            owner.read_event(50, &[OscType::String("view".into()), OscType::Float(0.0)]),
            None,
            "a payload that is not an edit is not one"
        );
    }

    /// A dragged sample and a whole stroke are the same intent at two lengths,
    /// which is what makes one owner answer both — **including what the undo
    /// stack calls it**, which is the one place they were still two. The
    /// reading is the projection's now
    /// (`clausters_editing::samples::write`), and it names the verb once: a
    /// host's undo entry and a client's say the same thing over the same
    /// gesture.
    #[test]
    fn a_sample_and_a_stroke_are_one_intent() {
        let mut owner = Owner::new(Document::new(clang(1)));
        owner.bind(50, NodeId(9));

        let (one, label) = owner
            .read_event(
                50,
                &[
                    OscType::String("sample".into()),
                    OscType::Int(0),
                    OscType::Long(12),
                    OscType::Float(0.5),
                    OscType::Float(0.1),
                ],
            )
            .expect("a dragged sample");
        assert_eq!(label, "draw the samples");
        assert!(
            matches!(&one, Intent::WriteSamples { start, values, .. }
                     if *start == 12 && values == &[0.5]),
            "{one:?}"
        );

        let run: Vec<u8> = [0.25f32, -0.25, 0.75]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let (many, label) = owner
            .read_event(
                50,
                &[
                    OscType::String("draw".into()),
                    OscType::Int(0),
                    OscType::Long(12),
                    OscType::Blob(run.clone()),
                    OscType::Blob(run),
                ],
            )
            .expect("a stroke");
        assert_eq!(label, "draw the samples", "one verb, one name");
        assert!(
            matches!(&many, Intent::WriteSamples { start, values, .. }
                     if *start == 12 && values.len() == 3),
            "{many:?}"
        );

        // **And the two rules the host used to be short of.** A stroke that
        // covered no frame is not an edit -- recorded, it would be a pile entry
        // that undoes to itself -- and an inverse that does not cover the
        // write's span is no inverse, which is better said than pretended.
        assert_eq!(
            owner.read_event(
                50,
                &[
                    OscType::String("draw".into()),
                    OscType::Int(0),
                    OscType::Long(12),
                    OscType::Blob(Vec::new()),
                    OscType::Blob(Vec::new()),
                ]
            ),
            None,
            "a stroke over nothing is not an edit"
        );
        let short: Vec<u8> = 0.5f32.to_le_bytes().to_vec();
        let long: Vec<u8> = [0.25f32, -0.25]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            owner.read_inverse(
                50,
                &[
                    OscType::String("draw".into()),
                    OscType::Int(0),
                    OscType::Long(12),
                    OscType::Blob(long),
                    OscType::Blob(short),
                ]
            ),
            None,
            "an inverse that covers half the write undoes half of it"
        );
    }

    /// The milestone's own acceptance, in the half a Rust test can run: a
    /// session is opened, edited by an intent the host translated, undone,
    /// redone and saved — and what comes back is the document as edited, in the
    /// format the crate defines and nothing here re-implements.
    #[test]
    fn a_session_opens_is_edited_undone_redone_and_saved() {
        let root = aggregate(
            1,
            vec![Member {
                offset: 0.0,
                dur: None,
                node: clang(2),
            }],
        );
        let dir = std::env::temp_dir();
        let path = dir.join(format!("clausters_h3_{}.json", std::process::id()));
        let written = Session::new(Document::new(root));
        std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();

        let mut owner = Owner::open(&path)
            .expect("the session opens")
            .with_units_per_beat(1.0);
        owner.bind(50, NodeId(2));

        // Edited the way a gesture would edit it: the payload, translated.
        let (intent, label) = owner
            .read_event(
                50,
                &[
                    OscType::String("clip".into()),
                    OscType::Float(4.0),
                    OscType::Float(0.0),
                ],
            )
            .expect("a clip moved");
        assert!(owner.apply(&intent, &Against::default(), label).applied);

        // Taken back, and put back.
        assert_eq!(owner.undo().len(), 1);
        assert_eq!(owner.redo().len(), 1);

        let out = dir.join(format!("clausters_h3_out_{}.json", std::process::id()));
        owner.save(&out).expect("it saves");
        let reopened = Owner::open(&out).expect("and reopens");
        let Body::Aggregate { members, .. } = &reopened.document.root.body else {
            panic!("an aggregate")
        };
        assert_eq!(
            members[0].offset, 4.0,
            "the edit survived the round trip through the file"
        );
        assert!(
            reopened.document.version > 1,
            "and so did the version the edits moved"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn a_widget_addresses_the_node_the_tree_bound_it_to() {
        let mut owner = Owner::new(Document::new(clang(1)));
        assert_eq!(owner.node_of(50), None, "nothing is inferred");
        owner.bind(50, NodeId(7));
        assert_eq!(owner.node_of(50), Some(NodeId(7)));
        owner.unbind(50);
        assert_eq!(owner.node_of(50), None, "a closed window forgets");
    }
}

#[cfg(test)]
mod wiring_tests {
    use super::*;
    use crate::host::Host;
    use clausters_document::{Body, Grouping, Member, Node, Opaque};

    fn doc() -> Document {
        Document::new(Node::new(
            NodeId(1),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members: vec![Member {
                    offset: 0.0,
                    dur: None,
                    node: Node::new(
                        NodeId(2),
                        Body::Clang {
                            config: Opaque::default(),
                            fires: None,
                        },
                    ),
                }],
                config: Opaque::none(),
            },
        ))
    }

    /// The seam that makes the owner more than a type nobody calls: a host that
    /// owns what it draws answers its own gesture, and one that does not says
    /// so, so the event goes out on the wire exactly as it always has.
    #[test]
    fn a_host_answers_its_own_gesture_only_when_it_owns_one() {
        let mut host = Host::new();
        let args = [
            crate::host::OscType::String("clip".into()),
            crate::host::OscType::Float(4.0),
            crate::host::OscType::Float(0.0),
        ];
        assert!(
            !host.answer_own(1, 50, 1, &args),
            "with no document there is nobody here to answer"
        );

        let mut owner = Owner::new(doc()).with_units_per_beat(1.0);
        owner.bind(50, NodeId(2));
        host.owner = Some(owner);
        let seq = host.outbox.borrow_mut().stamp(1, 50);
        assert!(host.answer_own(1, 50, seq, &args), "and with one, it does");

        let owner = host.owner.as_ref().expect("still there");
        let Body::Aggregate { members, .. } = &owner.document.root.body else {
            panic!("an aggregate")
        };
        assert_eq!(members[0].offset, 4.0, "the edit landed in the document");
        assert!(owner.can_undo(), "through the log, so it can be taken back");
        assert!(
            !host.outbox.borrow().is_pending(1, 50),
            "and the host acknowledged itself, so nothing is still in flight"
        );
    }

    /// A payload that is not an edit is not answered: it goes out, so a script
    /// attached to a host that happens to own a document still sees what it
    /// always saw.
    #[test]
    fn a_payload_that_is_not_an_edit_still_leaves() {
        let mut host = Host::new();
        let mut owner = Owner::new(doc());
        owner.bind(50, NodeId(2));
        host.owner = Some(owner);
        assert!(!host.answer_own(
            1,
            50,
            1,
            &[
                crate::host::OscType::String("view".into()),
                crate::host::OscType::Float(0.0)
            ]
        ));
    }
}

#[cfg(test)]
mod window_verb_tests {
    use super::*;
    use crate::host::{Host, OscType};
    use clausters_document::{Body, Grouping, Member, Node, Opaque};

    fn clang(id: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Clang {
                config: Opaque::default(),
                fires: None,
            },
        )
    }

    fn aggregate(id: u64, config: Value, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Aggregate {
                grouping: Grouping::Concrete,
                members,
                config: match config {
                    Value::Null => Opaque::none(),
                    table => Opaque(table),
                },
            },
        )
    }

    fn at(offset: f64, dur: Option<f64>, node: Node) -> Member {
        Member { offset, dur, node }
    }

    /// One clang on the multitrack, at one unit to the beat: the verbs' own tests
    /// are about the verbs, not the scale.
    fn owner_with_a_clip() -> Owner {
        let doc = Document::new(aggregate(1, Value::Null, vec![at(0.0, None, clang(2))]));
        let mut owner = Owner::new(doc).with_units_per_beat(1.0);
        owner.bind_multitrack(50);
        owner
    }

    /// The `"clips"` payload for one box: the multitrack as a hand left it.
    fn clips(entries: &[(&str, &str, f32, f32)]) -> Vec<OscType> {
        let mut args = vec![OscType::String("clips".into())];
        for (name, lane, at, dur) in entries {
            args.extend([
                OscType::String((*name).into()),
                OscType::String((*lane).into()),
                OscType::Float(*at),
                OscType::Float(*dur),
                OscType::Float(0.0),
                OscType::String(String::new()),
                OscType::Int(-1),
            ]);
        }
        args
    }

    /// The `"lanes"` payload: name, label, height, mute, solo, gain.
    fn lanes(entries: &[(&str, bool, bool, f32)]) -> Vec<OscType> {
        let mut args = vec![OscType::String("lanes".into())];
        for (name, mute, solo, gain) in entries {
            args.extend([
                OscType::String((*name).into()),
                OscType::String(String::new()),
                OscType::Float(96.0),
                OscType::Int(i32::from(*mute)),
                OscType::Int(i32::from(*solo)),
                OscType::Float(*gain),
                OscType::Int(1),
            ]);
        }
        args
    }

    fn offset(owner: &Owner) -> f64 {
        let Body::Aggregate { members, .. } = &owner.document.root.body else {
            panic!("an aggregate")
        };
        members[0].offset
    }

    /// A session host over `doc`, drawn the way `--session` draws it: the
    /// window opened on the real tree, the owner bound to the multitrack. The
    /// widget id comes back, because everything a hand does arrives on it.
    /// A host drawing a **multitrack** — the standalone shape: the session carries
    /// tracks, the document is empty, and the window is the same one the
    /// `--session` host opens.
    fn with_multitrack(multitrack: clausters_document::multitrack::Multitrack) -> (Host, i32, i32) {
        let def_id = 1;
        let doc = Document::new(aggregate(1, Value::Null, Vec::new()));
        let mut owner = Owner::new(doc).with_units_per_beat(100.0);
        owner.multitrack = multitrack;
        let (def, view) = composed(&mut owner, def_id);
        let mut host = Host::new();
        host.handle_packet(
            crate::host::OscPacket::Message(crate::host::OscMessage {
                addr: "/gui_def".into(),
                args: vec![OscType::Int(def_id), OscType::String(def.to_string())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        host.owner = Some(owner);
        (host, def_id, view)
    }

    /// **The multitrack editor's own window** over the owner's multitrack, numbered
    /// from past `def_id` the way `--session` numbers it, and the id of the
    /// multitrack's widget in it.
    fn composed(owner: &mut Owner, def_id: i32) -> (Value, i32) {
        (owner.open_editor(def_id, "t", (1000, 640)), def_id + 1)
    }

    /// **The transport row and the space bar are the editor's**: a click on a
    /// button of the row and the window's own `play` reach it and are answered,
    /// rather than leaving on the wire to a script that is not there.
    #[test]
    fn the_transport_row_and_the_space_bar_reach_the_editor() {
        use clausters_document::multitrack::{Multitrack, Track};

        // A multitrack with a track, which is what makes a session a multitrack.
        let multitrack = Multitrack {
            tracks: vec![Track::new(NodeId(10), NodeId(11))],
            ..Multitrack::default()
        };
        let (mut host, def_id, _view) = with_multitrack(multitrack);
        let play = def_id + 5;
        let seq = host.outbox.borrow_mut().stamp(def_id, play);
        assert!(
            host.answer_own(def_id, play, seq, &[OscType::String("click".into())]),
            "the play button"
        );
        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(
            host.answer_own(def_id, def_id, seq, &[OscType::String("play".into())]),
            "the space bar, addressed to the window"
        );
    }

    /// **The host answers the recorded exchange the way a client does.** The
    /// turns of `editor_exchange` in the web client's `editing-vectors.json`
    /// were made through the Python client's `MultitrackEditor` and are replayed
    /// through the web client's; here they are delivered to the standalone
    /// host, and what it tells itself, what it asks of the playback and the
    /// multitrack it is left with are the same, turn by turn. Widgets are compared
    /// by role, since which id an allocator hands out is each endpoint's own.
    ///
    /// Replaying them found two things the host did not do: settle a name it
    /// minted after a split, and answer the window's undo as a step of the
    /// history the editor reads.
    #[test]
    fn the_recorded_exchange_is_answered_the_same_by_the_host() {
        use crate::host::document::sources::{Take, Takes};
        use clausters_document::SourceId;
        use clausters_document::multitrack::Multitrack;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../web/tests/editing-vectors.json"
        );
        let vectors: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(path).expect("the vectors")).unwrap();
        let v = vectors
            .iter()
            .find(|v| v["kind"] == "editor_exchange")
            .expect("the exchange was generated");

        let def_id = 1;
        let mut owner = Owner::new(Document::new(aggregate(1, Value::Null, Vec::new())))
            .with_units_per_second(v["rate"].as_f64().unwrap());
        owner.multitrack = serde_json::from_value::<Multitrack>(v["multitrack"].clone()).unwrap();
        let mut takes = Takes::default();
        for (source, bufnum) in v["sources"].as_object().unwrap() {
            takes.insert(
                SourceId(source.parse().unwrap()),
                Take {
                    bufnum: bufnum.as_i64().unwrap() as i32,
                    channels: None,
                    frames: None,
                },
            );
        }
        let mut owner = owner.with_takes(takes);
        let def = owner.open_editor(def_id, "t", (1000, 640));
        let mut host = Host::new();
        host.handle_packet(
            crate::host::OscPacket::Message(crate::host::OscMessage {
                addr: "/gui_def".into(),
                args: vec![OscType::Int(def_id), OscType::String(def.to_string())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        host.owner = Some(owner);

        // The window numbers itself from the def's id: the multitrack, the ruler,
        // then the transport row after its own layout.
        let id_of = |role: &str| match role {
            "window" => def_id,
            "multitrack" => def_id + 1,
            "ruler" => def_id + 2,
            "rewind" => def_id + 4,
            "play" => def_id + 5,
            "stop" => def_id + 6,
            "clock" => def_id + 7,
            other => panic!("no widget plays {other}"),
        };
        let role_of = |widget: &Value| match widget.as_i64().map(|w| w - i64::from(def_id)) {
            Some(1) => serde_json::json!("multitrack"),
            Some(2) => serde_json::json!("ruler"),
            _ => widget.clone(),
        };
        let atom = |value: &Value| match value {
            Value::String(s) => OscType::String(s.clone()),
            Value::Number(n) if n.is_i64() => OscType::Int(n.as_i64().unwrap() as i32),
            Value::Number(n) => OscType::Float(n.as_f64().unwrap() as f32),
            other => panic!("a report carries no {other}"),
        };

        for turn in v["turns"].as_array().unwrap() {
            let name = turn["name"].as_str().unwrap();
            host.exchange = Default::default();
            let before = host.owner.as_ref().unwrap().multitrack.version;
            let mut args = vec![OscType::String(turn["tag"].as_str().unwrap().into())];
            args.extend(turn["values"].as_array().unwrap().iter().map(atom));
            let mut message = host.event_message(
                id_of(turn["target"].as_str().unwrap()),
                turn["seq"].as_i64().unwrap() as i32,
                args,
            );
            if let Some(against) = turn["against"].as_i64() {
                message.args[2] = OscType::Long(against);
            }
            assert!(
                host.deliver(def_id, &message),
                "{name}: the host answers it"
            );

            // `link` names the multitrack's own widget, which each endpoint numbers
            // for itself: compared by role, like every other widget.
            let by_role = |said: &mut Value| {
                for correction in said[4].as_array_mut().unwrap() {
                    if let Some(props) = correction[1].as_object_mut()
                        && props.contains_key("link")
                    {
                        props.insert("link".into(), serde_json::json!("multitrack"));
                    }
                }
            };
            let mut expected = turn["messages"].clone();
            for said in expected.as_array_mut().unwrap() {
                by_role(said);
            }
            let told: Vec<Value> = host
                .exchange
                .told
                .iter()
                .map(|said| {
                    let mut said = said.clone();
                    for correction in said[4].as_array_mut().unwrap() {
                        correction[0] = role_of(&correction[0]);
                    }
                    by_role(&mut said);
                    said
                })
                .collect();
            assert_eq!(Value::Array(told), expected, "{name}: what it was told");
            assert_eq!(
                Value::Array(host.exchange.asked.clone()),
                turn["playback"],
                "{name}: what the playback was asked"
            );
            let multitrack = &host.owner.as_ref().unwrap().multitrack;
            assert_eq!(
                multitrack.version != before,
                turn["changed"].as_bool().unwrap(),
                "{name}: whether the multitrack changed"
            );
            let regions: Vec<Value> = multitrack
                .tracks
                .iter()
                .flat_map(|t| {
                    t.lanes.iter().flat_map(move |lane| {
                        lane.regions.iter().map(move |r| {
                            serde_json::json!([t.id.0, r.id.0, r.position.0, r.length.0])
                        })
                    })
                })
                .collect();
            assert_eq!(
                Value::Array(regions),
                turn["regions"],
                "{name}: the multitrack"
            );
        }
    }

    /// **A join's box is drawn over the buffer the host made for it**, after
    /// the whole turn and its settle (found 2026-09-13, measured in a
    /// standalone host: a joined box that sounded right drew empty until it was
    /// moved to another track, because the settle projected the multitrack with the
    /// editor's table from before the join's source was minted).
    #[test]
    fn a_joined_box_is_drawn_over_the_buffer_its_source_was_made_in() {
        use crate::host::document::sources::{Take, Takes};
        use clausters_document::multitrack::{Content, Multitrack, Region, Track};
        use clausters_document::{
            Lifetime, Second, SegmentRef, SegmentSource, SourceId, SourceRef,
        };

        // One take cut in two, the halves swapped: a join that mints.
        let mut track = Track::new(NodeId(1), NodeId(2));
        for (id, at, start) in [(10, 1.0, 0.0), (11, 0.0, 1.0)] {
            let mut region = Region::new(
                NodeId(id),
                Second(at),
                Second(1.0),
                Content::Unknown(Value::Null),
            );
            region.content = Content::window(SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(7),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start,
                duration: 1.0,
            });
            track.lanes[0].regions.push(region);
        }
        let def_id = 1;
        let mut owner = Owner::new(Document::new(aggregate(1, Value::Null, Vec::new())))
            .with_units_per_second(48_000.0);
        owner.multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let mut takes = Takes::default();
        takes.insert(
            SourceId(7),
            Take {
                bufnum: 3,
                channels: Some(1),
                frames: Some(96_000),
            },
        );
        let mut owner = owner.with_takes(takes);
        let def = owner.open_editor(def_id, "t", (1000, 640));
        let mut host = Host::new();
        host.handle_packet(
            crate::host::OscPacket::Message(crate::host::OscMessage {
                addr: "/gui_def".into(),
                args: vec![OscType::Int(def_id), OscType::String(def.to_string())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        host.owner = Some(owner);
        let view = def_id + 1;

        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(
            def_id,
            view,
            seq,
            &[
                OscType::String("join".into()),
                OscType::String("10".into()),
                OscType::String("11".into()),
            ],
        ));
        let owner = host.owner.as_ref().unwrap();
        let joined = &owner.multitrack.tracks[0].lanes[0].regions;
        assert_eq!(joined.len(), 1, "one box");
        let minted = match &joined[0].content {
            Content::Window { window, .. } => window.source.samples().map(|s| s.source),
            _ => None,
        }
        .expect("a window onto the minted source");
        let bufnum = owner
            .buffer_table()
            .get(&minted)
            .copied()
            .expect("the host made the join a buffer");
        let clips = host.registry().get(view).expect("the multitrack").props["clips"].clone();
        let clips = clips.as_array().expect("the boxes");
        let drawn = clips
            .chunks(7)
            .find(|b| b[0].as_str().and_then(|s| s.parse::<u64>().ok()) == Some(joined[0].id.0))
            .expect("the joined box is drawn");
        assert_eq!(
            drawn[6],
            Value::from(bufnum),
            "over the buffer its source was made in, not over none"
        );
    }

    /// **A session host opens the editor a script opens**: the ruler above the
    /// multitrack and the transport row under it, every widget of it registered —
    /// an editor of the host's own had neither.
    #[test]
    fn a_multitrack_opens_in_its_editors_own_window() {
        let (host, def_id, view) =
            with_multitrack(clausters_document::multitrack::Multitrack::default());
        let tree = host.window_def(def_id).expect("the window is defined");
        assert!(tree.find(view).is_some(), "the multitrack");
        for widget in 3..=7 {
            assert!(
                tree.find(def_id + widget - 1).is_some(),
                "the ruler and the transport row, widget {}",
                def_id + widget - 1
            );
        }
    }

    /// **The head sweeps from the moment the window opens** (found 2026-09-13,
    /// by eye: a standalone host placed the cursor and never moved a line). The
    /// ruler is the window's first timeline member and seeds the group, so an
    /// anchor stated only on the multitrack was dropped; a client hid it by setting
    /// one afterwards.
    #[test]
    fn a_multitrack_window_opens_with_its_head_anchored() {
        let (host, _def_id, view) =
            with_multitrack(clausters_document::multitrack::Multitrack::default());
        let key = host
            .timeline_key(view)
            .expect("the multitrack is on a timeline");
        let state = host.timelines().state(key).expect("its group");
        assert_eq!(
            state.playhead_at, 0.0,
            "anchored at the multitrack's position"
        );
    }

    /// **A multitrack that sounds tells its window where its meters are** (found
    /// 2026-09-13, by eye: a standalone host's strips never moved). The window
    /// is composed before anything is played, so the buses are only known once
    /// the playback has made the tracks; a client's editor is handed them on
    /// every turn, and the host's is handed them here.
    #[test]
    fn a_sounding_multitrack_tells_its_window_where_the_meters_are() {
        use clausters_document::multitrack::{Multitrack, Track};

        let multitrack = Multitrack {
            tracks: vec![Track::new(NodeId(10), NodeId(11))],
            ..Multitrack::default()
        };
        let (mut host, _def_id, view) = with_multitrack(multitrack);
        // A throwaway socket standing in for the server that sounds.
        let server = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let leg = crate::host::ServerLeg::connect(server.local_addr().unwrap()).unwrap();
        host.set_server_link(crate::host::ServerLink::Udp(leg));
        host.sound_multitrack();
        let told = host
            .owner
            .as_ref()
            .and_then(|o| o.editor())
            .map_or(0, |e| e.meters().len());
        assert_eq!(told, 1, "the editor knows the track's buses");
        let meters = &host.registry().get(view).expect("the multitrack").props["meters"];
        assert_eq!(
            meters.as_array().map_or(0, Vec::len),
            4,
            "and the widget reads them: track, first bus, end, channels"
        );
    }

    /// A box on a lane, a window onto source 1.
    fn region(id: u64, position: f64, length: f64) -> clausters_document::multitrack::Region {
        use clausters_document::multitrack::{Content, Region};
        use clausters_document::{
            Lifetime, Second, SegmentRef, SegmentSource, SourceId, SourceRef,
        };
        let mut region = Region::new(
            NodeId(id),
            Second(position),
            Second(length),
            Content::Window {
                window: SegmentRef {
                    source: SegmentSource::Samples(SourceRef {
                        source: SourceId(1),
                        lifetime: Lifetime::Session,
                        generation: 0,
                        range: None,
                    }),
                    start: 0.0,
                    duration: length,
                },
                playrate: 1.0,
                looping: false,
                args: Default::default(),
            },
        );
        region.name = Some(format!("r{id}"));
        region
    }

    fn opened(doc: Document, units_per_beat: f64) -> (Host, i32, i32) {
        let def_id = 1;
        let drawn = super::tree::draw(
            &doc,
            &super::tree::Look {
                first_id: def_id + 1,
                units_per_beat,
                ..super::tree::Look::default()
            },
            "t",
        );
        let mut owner = Owner::new(doc).with_units_per_beat(units_per_beat);
        for b in &drawn.bindings {
            owner.bind(b.widget, b.node);
        }
        owner.bind_multitrack(drawn.widget);
        let mut host = Host::new();
        host.handle_packet(
            crate::host::OscPacket::Message(crate::host::OscMessage {
                addr: "/gui_def".into(),
                args: vec![OscType::Int(def_id), OscType::String(drawn.def.to_string())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        host.owner = Some(owner);
        (host, def_id, drawn.widget)
    }

    /// What the **widget** holds, as groups — the picture, not the document.
    fn drawn_prop(host: &Host, def_id: i32, widget: i32, key: &str, n: usize) -> Vec<Vec<Value>> {
        let info = host
            .widget_kind(def_id, widget)
            .and_then(|k| {
                k.as_element()
                    .map(crate::host::widget::element::Element::info)
            })
            .unwrap_or_else(|| panic!("widget {widget} is not the multitrack"));
        let flat = info
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_array().cloned())
            .unwrap_or_default();
        flat.chunks_exact(n).map(<[Value]>::to_vec).collect()
    }

    fn drawn_clips(host: &Host, def_id: i32, widget: i32) -> Vec<Vec<Value>> {
        drawn_prop(host, def_id, widget, "clips", 7)
    }

    fn drawn_lanes(host: &Host, def_id: i32, widget: i32) -> Vec<Vec<Value>> {
        drawn_prop(host, def_id, widget, "lanes", 7)
    }

    /// Undo and redo reach the **owner** where there is one, which is what
    /// makes a standalone editor's history its own rather than a message it
    /// sends to nobody. They address the window, so they are read before
    /// anything looks for a node.
    #[test]
    fn the_windows_own_verbs_reach_the_owner() {
        let mut host = Host::new();
        host.owner = Some(owner_with_a_clip());
        let seq = host.outbox.borrow_mut().stamp(1, 50);
        assert!(host.answer_own(1, 50, seq, &clips(&[("2", "2", 4.0, 0.0)])));
        assert_eq!(offset(host.owner.as_ref().unwrap()), 4.0);

        // Addressed to the window (id 1 here), not to the multitrack.
        let seq = host.outbox.borrow_mut().stamp(1, 1);
        assert!(host.answer_own(1, 1, seq, &[OscType::String("undo".into())]));
        assert_eq!(offset(host.owner.as_ref().unwrap()), 0.0, "taken back");

        let seq = host.outbox.borrow_mut().stamp(1, 1);
        assert!(host.answer_own(1, 1, seq, &[OscType::String("redo".into())]));
        assert_eq!(offset(host.owner.as_ref().unwrap()), 4.0, "and put back");
    }

    /// **A host that owns a multitrack answers for the multitrack's whole vocabulary**,
    /// and not for the two tags this happened to route by name.
    ///
    /// The defect this pins (found 2026-09-12, auditing the owner): the
    /// dispatch read `tag == "clips" || tag == "lanes"` while the crate that
    /// *reads* a report answers for four — `points` and `join` as well. So a
    /// standalone host drew curves it could not edit and had a `j` that reached
    /// nobody: the payload fell through to the tree's reader, which has no arm
    /// for either, and left on the wire to a client that is not there.
    ///
    /// A tag list written twice is the shape of it, which is why the fix is to
    /// ask the domain rather than to add two words here.
    #[test]
    fn the_multitracks_own_vocabulary_reaches_the_owner_whole() {
        use clausters_document::multitrack::{Automation, Multitrack, Track};
        use clausters_document::{Opaque, Point};

        let mut track = Track::new(NodeId(10), NodeId(11));
        track.name = Some("t10".into());
        track.automation.push(Automation {
            points: vec![
                Point {
                    at: 0.0,
                    value: 1.0,
                    data: Opaque::default(),
                },
                Point {
                    at: 4.0,
                    value: 1.0,
                    data: Opaque::default(),
                },
            ],
            visible: true,
            ..Automation::new(NodeId(30), Opaque::default())
        });
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let (mut host, def_id, view) = with_multitrack(multitrack);

        // The payload a dragged break-point leaves: every curve there is, each
        // point a quintuple, in the axis' own unit (100 units a beat here).
        let args = vec![
            OscType::String("points".into()),
            OscType::String("30".into()),
            OscType::Float(0.0),
            OscType::Float(1.0),
            OscType::Int(1),
            OscType::Float(0.0),
            OscType::String("30".into()),
            OscType::Float(400.0),
            OscType::Float(0.25),
            OscType::Int(1),
            OscType::Float(0.0),
        ];
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(
            host.answer_own(def_id, view, seq, &args),
            "the multitrack owns `points`, so the host answers for it"
        );
        let moved = host
            .owner
            .as_ref()
            .and_then(|o| o.multitrack.automation(NodeId(30)))
            .map(|a| a.points[1].value);
        assert_eq!(moved, Some(0.25), "and the curve is where the hand left it");
    }

    /// **The host delivers the event a client receives**, the version it was
    /// made against included, so an edit a route the hand never saw has
    /// overtaken is refused here as it is in a script. The host used to build
    /// its own event with no version, which applied every edit unchecked.
    #[test]
    fn an_edit_made_against_a_picture_that_is_gone_is_refused_here_too() {
        use clausters_document::multitrack::{Automation, Multitrack, Track};
        use clausters_document::{Opaque, Point};

        let mut track = Track::new(NodeId(10), NodeId(11));
        track.name = Some("t10".into());
        track.automation.push(Automation {
            points: vec![
                Point {
                    at: 0.0,
                    value: 1.0,
                    data: Opaque::default(),
                },
                Point {
                    at: 4.0,
                    value: 1.0,
                    data: Opaque::default(),
                },
            ],
            visible: true,
            ..Automation::new(NodeId(30), Opaque::default())
        });
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let (mut host, def_id, view) = with_multitrack(multitrack);
        let points = |value: f32| {
            vec![
                OscType::String("points".into()),
                OscType::String("30".into()),
                OscType::Float(0.0),
                OscType::Float(1.0),
                OscType::Int(1),
                OscType::Float(0.0),
                OscType::String("30".into()),
                OscType::Float(400.0),
                OscType::Float(value),
                OscType::Int(1),
                OscType::Float(0.0),
            ]
        };
        let second = |host: &Host| {
            host.owner
                .as_ref()
                .and_then(|o| o.multitrack.automation(NodeId(30)))
                .map(|a| a.points[1].value)
        };

        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(def_id, view, seq, &points(0.25)));
        let answered = host.outbox.borrow().version();
        assert!(answered > 0, "the answer stated the version it left");

        // A route no gesture took moves the multitrack.
        if let Some(owner) = host.owner.as_mut() {
            for _ in 0..5 {
                owner.editing.moved();
            }
        }
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        let message = host.event_message(view, seq, points(0.75));
        assert_eq!(
            message.args[2],
            OscType::Long(answered),
            "made against what the host was drawing"
        );
        assert!(host.deliver(def_id, &message), "the editor's to answer");
        assert_eq!(
            second(&host),
            Some(0.25),
            "the overtaken edit is not applied"
        );
        assert_eq!(
            host.outbox
                .borrow()
                .last()
                .and_then(|a| a.reason.clone())
                .as_deref(),
            Some("the data changed since this edit"),
            "and the window is told why"
        );
    }

    /// **A join makes the source it minted**, so the box that names it draws,
    /// sounds and has a length its edges stop at.
    ///
    /// The defect this pins (found 2026-09-13 by the user, on a standalone
    /// host): a join mints a source — spans of the takes the table already
    /// holds — and the document says plainly that it does not build one:
    /// *"whoever has the samples fills it in when it realizes the join"*. The
    /// Python client realizes it at edit time; this host realized it only when
    /// a session was **opened**, so a join made while one was open was built by
    /// nobody. The box then named a source with no buffer, which is a box that
    /// draws empty, plays nothing and — since an edge stops at the source's
    /// length — pulls forever in both directions.
    #[test]
    fn a_join_made_by_a_hand_installs_the_source_it_minted() {
        use clausters_document::multitrack::{Multitrack, Track};

        // Two boxes that touch on the lane and do **not** read on from each
        // other: the second is the earlier half of the take, put back second.
        // That is the case with no window over it, so the join has to mint.
        let mut track = Track::new(NodeId(10), NodeId(11));
        track.name = Some("t10".into());
        let mut first = region(12, 0.0, 2.0);
        let mut second = region(13, 2.0, 2.0);
        if let clausters_document::multitrack::Content::Window { window, .. } = &mut first.content {
            window.start = 2.0;
        }
        if let clausters_document::multitrack::Content::Window { window, .. } = &mut second.content
        {
            window.start = 0.0;
        }
        track.lanes[0].regions = vec![first, second];
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let (mut host, def_id, view) = with_multitrack(multitrack);
        // The take the two boxes read, as a session's open would have resolved
        // it: without this the join is over samples nobody loaded. Four
        // seconds, since the boxes read all four: a take stated shorter is a
        // join past its end, which the editor refuses.
        if let Some(owner) = host.owner.as_mut() {
            owner.takes.insert(
                clausters_document::SourceId(1),
                super::sources::Take {
                    bufnum: 7,
                    channels: Some(2),
                    frames: Some(192_000),
                },
            );
        }

        let args = vec![
            OscType::String("join".into()),
            OscType::String("12".into()),
            OscType::String("13".into()),
        ];
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(
            host.answer_own(def_id, view, seq, &args),
            "the multitrack's verb"
        );

        let owner = host.owner.as_ref().expect("an owner");
        let minted: Vec<_> = owner
            .takes
            .iter()
            .filter(|(id, _)| id.0 != 1)
            .map(|(id, take)| (id.0, take.bufnum, take.frames))
            .collect();
        assert_eq!(minted.len(), 1, "the join's own source, in the table");
        let (_, bufnum, frames) = minted[0];
        assert_ne!(bufnum, 7, "and in a buffer of its own, not over the take");
        assert!(frames.is_some_and(|f| f > 0), "as long as its parts");
    }

    /// **A take whose length nobody stated learns it once it is read**
    /// (decided 2026-09-13): the length of a take is the source's fact, what a
    /// join is bounded by and what a save writes down. One already stated is
    /// kept, and a buffer with nothing live answers nothing.
    #[test]
    fn a_read_take_learns_its_length_and_keeps_a_stated_one() {
        use clausters_document::multitrack::Multitrack;

        let (mut host, _def_id, _view) = with_multitrack(Multitrack::default());
        let owner = host.owner.as_mut().expect("an owner");
        let take = |bufnum, frames| super::sources::Take {
            bufnum,
            channels: Some(1),
            frames,
        };
        owner
            .takes
            .insert(clausters_document::SourceId(1), take(7, None));
        owner
            .takes
            .insert(clausters_document::SourceId(2), take(8, Some(4_800)));
        owner
            .takes
            .insert(clausters_document::SourceId(3), take(9, None));
        let frames_of = |bufnum: i32| match bufnum {
            7 => Some(96_000),
            8 => Some(1),
            _ => None,
        };
        assert_eq!(
            owner.learn_lengths(frames_of),
            1,
            "only the unstated, live one"
        );
        assert_eq!(
            owner.buffer_lengths(),
            HashMap::from([
                (clausters_document::SourceId(1), 96_000),
                (clausters_document::SourceId(2), 4_800),
            ])
        );
        assert_eq!(owner.learn_lengths(frames_of), 0, "and once");
    }

    /// **The picture a host pushes back is the whole picture**, not the two
    /// props somebody listed.
    ///
    /// The defect this pins (found 2026-09-12 by the user, on a standalone host
    /// opened on a session: "no crea los lanes para las curvas al presionar A").
    /// The header's `A` exists to **make** a track's gain curve, and the multitrack
    /// made it: the edit applied, the document kept it, and what went back onto
    /// the widget was `lanes` and `clips` — so nothing that draws a curve ever
    /// arrived and the toggle read as a dead key. The projection had said
    /// `curves`, `layers`, `points`, `hidden` and `loops` all along.
    ///
    /// A client answers with all of them, which is why this was only ever
    /// visible with nobody attached — and why the fix is to push what the
    /// projection produced rather than a list written at the call site.
    #[test]
    fn the_toggle_that_makes_a_curve_puts_the_row_on_the_widget() {
        use clausters_document::multitrack::{Multitrack, Track};

        let mut track = Track::new(NodeId(10), NodeId(11));
        track.name = Some("t10".into());
        track.lanes[0].regions = vec![region(12, 0.0, 4.0)];
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let (mut host, def_id, view) = with_multitrack(multitrack);
        assert!(
            drawn_prop(&host, def_id, view, "curves", 6).is_empty(),
            "the track starts with no automation"
        );

        // The `A` toggle's payload: the strips as they now stand, this one
        // saying its automation is shown.
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(def_id, view, seq, &lanes(&[("10", false, false, 1.0)])));
        assert!(
            host.owner
                .as_ref()
                .is_some_and(|o| o.multitrack.automations().count() == 1),
            "the multitrack minted the gain curve"
        );
        let curves = drawn_prop(&host, def_id, view, "curves", 6);
        assert_eq!(curves.len(), 1, "and the widget was told about it");
        assert_eq!(curves[0][1], serde_json::json!("10"), "under its track");
        // Its break-points came with it, or the row would draw an empty box.
        assert!(
            !drawn_prop(&host, def_id, view, "points", 5).is_empty(),
            "the curve's own points reached the widget too"
        );
    }

    /// **And a verb the multitrack refuses says why, in the window of the host that
    /// refused it** — the same sentence a client would have put there.
    ///
    /// A join is the one tag whose refusal is about the *material* rather than
    /// about the picture, so it is the one that had a reason to lose: the host
    /// read the intents and dropped the `Err`, which is a key that does nothing
    /// and says nothing.
    #[test]
    fn a_multitrack_that_refuses_a_verb_says_so_on_the_bar() {
        use clausters_document::multitrack::{Multitrack, Track};

        let mut track = Track::new(NodeId(10), NodeId(11));
        track.name = Some("t10".into());
        track.lanes[0].regions = vec![region(12, 0.0, 2.0), region(13, 4.0, 2.0)];
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let (mut host, def_id, view) = with_multitrack(multitrack);

        // Two boxes with a gap between them: they are on one lane and they do
        // not touch, which the document refuses and the picture cannot say.
        let args = vec![
            OscType::String("join".into()),
            OscType::String("12".into()),
            OscType::String("13".into()),
        ];
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(
            host.answer_own(def_id, view, seq, &args),
            "it is the multitrack's"
        );
        let statuses = host.statuses();
        let line = statuses
            .get(&def_id)
            .and_then(|s| s.last())
            .expect("a line");
        assert_eq!(line.kind, crate::host::status::Kind::Refused);
        assert!(
            line.text.contains("gap"),
            "the document's own reason, in the window: {}",
            line.text
        );
    }

    /// **An undo has to move the picture, not only the document.** A drag needs
    /// no help — the gesture already moved the box on screen — so the failure
    /// this pins is the one that looks like the key doing nothing: the document
    /// goes back, the widget stays where the hand left it, and nothing on
    /// screen changes.
    #[test]
    fn an_undo_moves_the_picture_back_and_not_only_the_document() {
        let doc = Document::new(aggregate(
            1,
            Value::Null,
            vec![at(0.0, Some(1.0), clang(2))],
        ));
        let (mut host, def_id, view) = opened(doc, 100.0);
        let offset_of = |host: &Host| drawn_clips(host, def_id, view)[0][2].as_f64().unwrap();
        assert_eq!(offset_of(&host), 0.0);

        // The edit a drag reports: the multitrack as it now stands, in the axis'
        // own unit.
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(def_id, view, seq, &clips(&[("2", "2", 400.0, 100.0)])));
        assert_eq!(
            host.owner.as_ref().map(offset),
            Some(4.0),
            "4 beats at 100 units a beat"
        );

        // ...and taken back: the widget follows the document.
        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(host.answer_own(def_id, def_id, seq, &[OscType::String("undo".into())]));
        assert_eq!(
            offset_of(&host),
            0.0,
            "the undo moved the picture, not only the document"
        );
    }

    /// The strip, end to end in a host that owns what it draws: press mute, and
    /// the document is muted; undo, and the **lane** comes back up with it.
    #[test]
    fn a_muted_lane_undoes_the_strip_and_not_only_the_document() {
        let doc = Document::new(aggregate(
            1,
            Value::Null,
            vec![at(
                0.0,
                None,
                aggregate(
                    2,
                    serde_json::json!({"mute": false}),
                    vec![at(0.0, Some(1.0), clang(3))],
                ),
            )],
        ));
        let (mut host, def_id, view) = opened(doc, 100.0);
        // The widget carries a flag as the wire's own 0/1.
        let muted = |host: &Host| drawn_lanes(host, def_id, view)[0][3].as_i64();
        assert_eq!(muted(&host), Some(0), "drawn from the document");

        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(def_id, view, seq, &lanes(&[("2", true, false, 1.0)])));
        assert_eq!(
            host.owner
                .as_ref()
                .and_then(|o| o.document.find(NodeId(2)))
                .and_then(|n| n.body.config())
                .map(|c| c.0["mute"].clone()),
            Some(serde_json::json!(true)),
            "the multitrack is muted, and it is the multitrack that says so"
        );

        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(host.answer_own(def_id, def_id, seq, &[OscType::String("undo".into())]));
        assert_eq!(
            muted(&host),
            Some(0),
            "the undo lifted the strip, not only the document"
        );
    }

    /// **A lane a hand did not touch is not an edit.** The mixer reports every
    /// strip after any of them moves, so a reader that took the payload at face
    /// value would log a `Configure` per lane on every fader drag — and the
    /// undo of one fader would be one step per lane.
    #[test]
    fn only_the_strip_that_moved_becomes_an_edit() {
        let doc = Document::new(aggregate(
            1,
            Value::Null,
            vec![
                at(
                    0.0,
                    None,
                    aggregate(2, Value::Null, vec![at(0.0, None, clang(3))]),
                ),
                at(
                    0.0,
                    None,
                    aggregate(4, Value::Null, vec![at(0.0, None, clang(5))]),
                ),
            ],
        ));
        let owner = {
            let mut owner = Owner::new(doc).with_units_per_beat(1.0);
            owner.bind_multitrack(50);
            owner
        };
        let payload = lanes(&[("2", false, false, 1.0), ("4", false, false, 0.5)]);
        let edits = owner.read_events(50, &payload);
        assert_eq!(edits.len(), 1, "one fader moved: {edits:?}");
        assert!(matches!(edits[0].0, Intent::Configure { node, .. } if node == NodeId(4)));
    }

    /// The length half of the same rule, and the case that was broken: a clip
    /// whose placement states **no** length is drawn at the element's own, so
    /// the inverse of the first resize of one carries no `dur` at all — and an
    /// adopter reading that as "leave the width alone" left the box at the size
    /// the hand had given it while the document went back.
    #[test]
    fn an_undone_first_resize_puts_the_clips_width_back() {
        let mut clang = clang(2);
        // The element's own length, and no placement length over it: exactly
        // what a clip nobody has resized yet is.
        clang.duration = Some(2.0);
        let doc = Document::new(aggregate(1, Value::Null, vec![at(0.0, None, clang)]));
        let (mut host, def_id, view) = opened(doc, 100.0);
        let width = |host: &Host| drawn_clips(host, def_id, view)[0][3].as_f64().unwrap();
        assert_eq!(width(&host), 200.0, "two beats, the element's own length");

        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(def_id, view, seq, &clips(&[("2", "2", 0.0, 500.0)])));
        assert_eq!(width(&host), 500.0, "the hand's");

        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(host.answer_own(def_id, def_id, seq, &[OscType::String("undo".into())]));
        assert_eq!(
            width(&host),
            200.0,
            "and the undo gave the element's own length back to the picture"
        );
    }

    /// **The milestone's own acceptance, and the bug it was named for.** A
    /// block move and a lane change reach the document, which they could not
    /// before: the host's own owner read `"clip"` and neither of the two tags a
    /// gesture switched to, so a selection dragged in `--session` moved on
    /// screen and nowhere else.
    ///
    /// They arrive as one payload with no gesture in it — the clips, as they
    /// now stand — and they undo as **one step**, because a block move is one
    /// thing a hand did.
    #[test]
    fn a_block_move_and_a_lane_change_reach_the_document() {
        let doc = Document::new(aggregate(
            1,
            Value::Null,
            vec![
                at(
                    0.0,
                    None,
                    aggregate(
                        2,
                        Value::Null,
                        vec![at(0.0, Some(1.0), clang(3)), at(1.0, Some(1.0), clang(4))],
                    ),
                ),
                at(
                    0.0,
                    None,
                    aggregate(5, Value::Null, vec![at(0.0, Some(1.0), clang(6))]),
                ),
            ],
        ));
        let (mut host, def_id, view) = opened(doc, 100.0);
        let where_is = |host: &Host, node: u64| {
            host.owner
                .as_ref()
                .and_then(|o| o.shown().lane_of(NodeId(node)).map(|l| l.holder))
        };
        assert_eq!(where_is(&host, 3), Some(NodeId(2)));

        // **Both boxes of the first lane, dragged two beats along** — the
        // payload a marquee's block drag leaves, addressed to the one widget
        // and naming every clip there is.
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(
            def_id,
            view,
            seq,
            &clips(&[
                ("3", "2", 200.0, 100.0),
                ("4", "2", 300.0, 100.0),
                ("6", "5", 0.0, 100.0),
            ])
        ));
        let placed = |host: &Host, node: u64| {
            host.owner
                .as_ref()
                .and_then(|o| o.member_of(NodeId(node)).map(|m| m.offset))
        };
        assert_eq!(placed(&host, 3), Some(2.0), "both moved, in one message");
        assert_eq!(placed(&host, 4), Some(3.0));
        assert_eq!(
            placed(&host, 6),
            Some(0.0),
            "and the one nobody touched did not"
        );

        // **And a lane crossed**: clip 4 leaves the first track and joins the
        // second. That is not a placement at all — it changes which aggregate
        // holds it — which is why a reader of `"clip"` could never have done it.
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(
            def_id,
            view,
            seq,
            &clips(&[
                ("3", "2", 200.0, 100.0),
                ("4", "5", 400.0, 100.0),
                ("6", "5", 0.0, 100.0),
            ])
        ));
        assert_eq!(where_is(&host, 4), Some(NodeId(5)), "it changed lanes");
        assert_eq!(placed(&host, 4), Some(4.0), "at the beat the hand left it");
        assert_eq!(
            drawn_clips(&host, def_id, view)
                .iter()
                .find(|c| c[0] == "4")
                .map(|c| c[1].clone()),
            Some(Value::from("5")),
            "and the picture says so too, redrawn from the document"
        );

        // One gesture, one step back.
        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(host.answer_own(def_id, def_id, seq, &[OscType::String("undo".into())]));
        assert_eq!(where_is(&host, 4), Some(NodeId(2)), "back on its own lane");

        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(host.answer_own(def_id, def_id, seq, &[OscType::String("undo".into())]));
        assert_eq!(
            placed(&host, 3),
            Some(0.0),
            "and the block, in one more step"
        );
        assert_eq!(placed(&host, 4), Some(1.0));
    }

    /// **A session written today is a multitrack, and the host edits it.** The whole
    /// leg, end to end: the window is drawn from `Multitrack`, a hand's report
    /// arrives on the one widget, the crate's own vocabulary applies it, and
    /// `Ctrl`+`Z` walks it back.
    ///
    /// This is the case that opened as an **empty window**: the general tree is
    /// the leg being walked off, a session carries none, and a host that read
    /// only the tree drew nothing and said so in one log line.
    #[test]
    fn a_multitrack_is_drawn_edited_and_undone_by_a_host_that_owns_it() {
        use clausters_document::multitrack::{Content, Multitrack, Region, Track};
        use clausters_document::{Lifetime, SourceRef};
        use clausters_document::{Opaque as Op, Second, SegmentRef, SegmentSource, SourceId};

        let region = |id: u64, at: f64| {
            Region::new(
                NodeId(id),
                Second(at),
                Second(2.0),
                Content::Window {
                    window: SegmentRef {
                        source: SegmentSource::Samples(SourceRef {
                            source: SourceId(1),
                            lifetime: Lifetime::Session,
                            generation: 0,
                            range: None,
                        }),
                        start: 0.0,
                        duration: 2.0,
                    },
                    playrate: 1.0,
                    args: Op::none(),
                    looping: false,
                },
            )
        };
        let mut multitrack = Multitrack::default();
        let mut first = Track::new(NodeId(10), NodeId(11));
        first.lanes[0].regions = vec![region(12, 0.0), region(13, 4.0)];
        let second = Track::new(NodeId(20), NodeId(21));
        multitrack.tracks = vec![first, second];

        let mut session = clausters_document::Session::new(Document::empty());
        session.multitrack = multitrack;
        let owner = Owner::from_session(session)
            .with_units_per_beat(100.0)
            .with_units_per_second(48_000.0);
        assert!(
            owner.draws_multitrack(),
            "the file carries a multitrack and no tree"
        );

        let def_id = 1;
        let mut owner = owner;
        let (def, view) = composed(&mut owner, def_id);
        let mut host = Host::new();
        host.handle_packet(
            crate::host::OscPacket::Message(crate::host::OscMessage {
                addr: "/gui_def".into(),
                args: vec![OscType::Int(def_id), OscType::String(def.to_string())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        host.owner = Some(owner);

        let drawn_clips = |host: &Host| drawn_clips(host, def_id, view);
        assert_eq!(drawn_clips(&host).len(), 2, "two boxes on the first row");
        assert_eq!(drawn_clips(&host)[0][1], "10", "and both on it");

        // The hand drags one onto the other row: the multitrack as it now stands.
        let seq = host.outbox.borrow_mut().stamp(def_id, view);
        assert!(host.answer_own(
            def_id,
            view,
            seq,
            &clips(&[("12", "20", 100.0, 200.0), ("13", "10", 400.0, 200.0)])
        ));
        let on = |host: &Host, region: u64| {
            host.owner.as_ref().and_then(|o| {
                o.multitrack.tracks.iter().find_map(|t| {
                    t.lanes
                        .iter()
                        .any(|l| l.regions.iter().any(|r| r.id == NodeId(region)))
                        .then_some(t.id)
                })
            })
        };
        assert_eq!(on(&host, 12), Some(NodeId(20)), "it changed track");
        assert_eq!(
            drawn_clips(&host)
                .iter()
                .find(|c| c[0] == "12")
                .map(|c| c[1].clone()),
            Some(Value::from("20")),
            "and the picture says so, redrawn from the multitrack"
        );

        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(host.answer_own(def_id, def_id, seq, &[OscType::String("undo".into())]));
        assert_eq!(on(&host, 12), Some(NodeId(10)), "back where it was");
        assert_eq!(
            drawn_clips(&host)
                .iter()
                .find(|c| c[0] == "12")
                .map(|c| c[2].as_f64()),
            Some(Some(0.0)),
            "and the picture went back with it"
        );
    }

    /// A save writes where the caller said and nowhere else: overwriting what
    /// you opened is a decision, so a session with no path says so instead.
    #[test]
    fn a_save_writes_only_where_a_path_was_named() {
        let mut host = Host::new();
        host.owner = Some(owner_with_a_clip());
        // Answered either way -- the verb *is* the window's -- but nothing is
        // written without a path.
        assert!(host.answer_own(1, 1, 1, &[OscType::String("save".into())]));

        let path =
            std::env::temp_dir().join(format!("clausters_h3_verb_{}.json", std::process::id()));
        host.owner = Some(owner_with_a_clip().saving_to(&path));
        assert!(host.answer_own(1, 1, 2, &[OscType::String("save".into())]));
        let written = std::fs::read_to_string(&path).expect("it wrote");
        assert!(
            written.contains("\"document\""),
            "a session, not a fragment"
        );
        let _ = std::fs::remove_file(&path);
    }
}
