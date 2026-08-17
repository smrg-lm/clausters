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

pub mod sources;
pub mod tree;

use std::collections::HashMap;

use clausters_core::osc::OscType;
use clausters_document::clipboard::decode_samples;
use clausters_document::{
    Against, Document, Intent, NodeId, Outcome, Rules, Session, apply_logged, log::Log,
};

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

fn long_at(args: &[OscType], n: usize) -> Option<u64> {
    match args.get(n) {
        Some(OscType::Long(v)) if *v >= 0 => Some(*v as u64),
        Some(OscType::Int(v)) if *v >= 0 => Some(*v as u64),
        _ => None,
    }
}

/// What the host holds when it is the one answering its own gestures.
pub struct Owner {
    /// The composition, as the crate keeps it.
    pub document: Document,
    /// The undo stack — the crate's, so an inverse is read out of the document
    /// rather than remembered by the gesture that made it.
    pub log: Log,
    /// The session this document came from, when it came from one: the sources
    /// its material lives in, which is what a save has to write back.
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
    /// Where a save writes, when the caller named a file.
    ///
    /// `None` is a session opened read-only, or one built in memory: **saving
    /// over what you opened is a decision, not a default**, so a caller that
    /// wants it says where.
    pub save_path: Option<std::path::PathBuf>,
    /// Which document node each widget's gestures address.
    ///
    /// The one thing this module adds to the crate, and the one thing only a
    /// host can know: a widget is a picture *of* a node, and an intent names
    /// the node. Nothing infers it — the tree that built the widgets records
    /// it, so a picture and the material under it cannot drift apart.
    nodes: HashMap<i32, NodeId>,
}

/// What applying an edit left behind, for the caller to draw and answer with.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    /// The edit describing the document as it now stands — the intent as given
    /// when it applied verbatim, the transformed one when it was snapped, and
    /// the **previous** value when it was refused.
    pub effective: Intent,
    /// The document's version afterwards, which is what an acknowledgement
    /// carries so a later edit can say what it was made against.
    pub version: u64,
    /// Whether anything actually moved. A refusal is not an error — it is the
    /// previous value — but nothing needs redrawing for one.
    pub applied: bool,
}

impl Owner {
    /// An owner of `document`, with no session behind it (a composition built
    /// in memory) and no grid.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            log: Log::new(),
            session: None,
            rules: Rules::none(),
            units_per_beat: 48_000.0,
            save_path: None,
            nodes: HashMap::new(),
        }
    }

    /// An owner of a session's document, keeping the session so a save has the
    /// sources to write with it.
    pub fn from_session(session: Session) -> Self {
        let mut owner = Self::new(session.document.clone());
        owner.session = Some(session);
        owner
    }

    /// The unit the picture was drawn in (samples per beat).
    pub fn with_units_per_beat(mut self, units: f64) -> Self {
        self.units_per_beat = units;
        self
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
        let session: Session =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.as_ref().display()))?;
        Ok(Self::from_session(session))
    }

    /// Writes the session back, with the document as it now stands.
    ///
    /// The sources travel unchanged: what an editing session edits is the
    /// arrangement and the material, and where the material *lives* is the
    /// session's own bookkeeping, which this host has no business rewriting.
    pub fn save(&self, path: impl AsRef<std::path::Path>) -> Result<(), String> {
        let mut session = self
            .session
            .clone()
            .unwrap_or_else(|| Session::new(self.document.clone()));
        session.document = self.document.clone();
        let text = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
        std::fs::write(path.as_ref(), text).map_err(|e| format!("{}: {e}", path.as_ref().display()))
    }

    /// Says which node a widget's gestures address. The tree that built the
    /// widgets is what calls this; nothing guesses.
    pub fn bind(&mut self, widget_id: i32, node: NodeId) {
        self.nodes.insert(widget_id, node);
    }

    /// Forgets a widget — a window closing, or a tree rebuilt.
    pub fn unbind(&mut self, widget_id: i32) {
        self.nodes.remove(&widget_id);
    }

    /// The node a widget addresses, if it addresses one.
    pub fn node_of(&self, widget_id: i32) -> Option<NodeId> {
        self.nodes.get(&widget_id).copied()
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
            // A clip moved or resized: where it now sits inside the set that
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
                let dur = float_at(args, 2)
                    .map(|d| d as f64 / units)
                    .filter(|d| *d > 0.0);
                Some((Intent::Place { node, offset, dur }, "move a clip"))
            }
            // One sample dragged (D1) — a run of one, so it and a stroke are
            // the same intent at two lengths.
            "sample" => {
                let start = long_at(args, 2)?;
                let value = float_at(args, 3)?;
                Some((
                    Intent::WriteSamples {
                        node,
                        channel: channel_at(args),
                        start,
                        values: vec![value],
                    },
                    "edit a sample",
                ))
            }
            // A whole stroke (D2), the run as a blob.
            "draw" => {
                let start = long_at(args, 2)?;
                let values = match args.get(3) {
                    Some(OscType::Blob(bytes)) => decode_samples(bytes),
                    _ => return None,
                };
                Some((
                    Intent::WriteSamples {
                        node,
                        channel: channel_at(args),
                        start,
                        values,
                    },
                    "draw",
                ))
            }
            _ => None,
        }
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
        let tag = match args.first() {
            Some(OscType::String(tag)) => tag.as_str(),
            _ => return None,
        };
        let start = long_at(args, 2)?;
        let values = match tag {
            "sample" => vec![float_at(args, 4)?],
            "draw" => match args.get(4) {
                Some(OscType::Blob(bytes)) => decode_samples(bytes),
                _ => return None,
            },
            _ => return None,
        };
        (!values.is_empty()).then_some(Intent::WriteSamples {
            node,
            channel: channel_at(args),
            start,
            values,
        })
    }

    /// Applies one intent through the log, so it can be undone.
    ///
    /// `label` is what the undo stack shows for it — the vocabulary a user
    /// reads ("draw", "move a clip"), not the wire's.
    pub fn apply(&mut self, intent: &Intent, against: &Against, label: &str) -> Applied {
        let outcome = apply_logged(
            &mut self.document,
            intent,
            against,
            &self.rules,
            &mut self.log,
            label,
        );
        self.report(outcome)
    }

    /// Applies an edit whose inverse **the caller holds**, logging that one.
    ///
    /// There is exactly one such edit and the crate says so: a destructive
    /// write's previous samples are not in the document (the document describes
    /// where material is, never what it holds), so `apply_logged` records an
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
        if outcome.applied {
            self.log.record(Entry::new(
                label,
                Step::Edit(outcome.effective.clone()),
                inverse.clone(),
            ));
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
        let Some(intents) = self.log.undo() else {
            return Vec::new();
        };
        self.replay(&intents)
    }

    /// Redoes the last undone edit, in the direction it was made.
    pub fn redo(&mut self) -> Vec<Applied> {
        let Some(steps) = self.log.redo() else {
            return Vec::new();
        };
        let intents: Vec<Intent> = steps.iter().filter_map(|s| s.intent().cloned()).collect();
        self.replay(&intents)
    }

    pub fn can_undo(&self) -> bool {
        self.log.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.log.can_redo()
    }

    /// Applies a run of intents with the checks off — the shape an undo or a
    /// redo needs, since what the log holds is by definition against the
    /// document as it was left, and snapping something twice would move it.
    fn replay(&mut self, intents: &[Intent]) -> Vec<Applied> {
        let mut out = Vec::with_capacity(intents.len());
        for intent in intents {
            let outcome = clausters_document::apply(
                &mut self.document,
                intent,
                &Against::default(),
                &Rules::none(),
            );
            out.push(self.report(outcome));
        }
        out
    }

    fn report(&self, outcome: Outcome) -> Applied {
        Applied {
            effective: outcome.effective,
            version: self.document.version,
            applied: outcome.applied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::{Body, Grouping, Member, Node, Opaque};

    fn event(id: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Event {
                config: Opaque::default(),
                fires: None,
            },
        )
    }

    fn set(id: u64, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Set {
                grouping: Grouping::Concrete,
                members,
                config: Opaque::none(),
            },
        )
    }

    #[test]
    fn an_edit_applies_through_the_log_and_undoes_out_of_the_document() {
        let root = set(
            1,
            vec![Member {
                offset: 0.0,
                dur: None,
                node: event(2),
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
            matches!(undone[0].effective, Intent::Place { offset, .. } if offset == 0.0),
            "{:?}",
            undone[0].effective
        );
    }

    /// Undo and redo are two directions of **one** stack, which is what keeps
    /// a redone edit the same edit rather than a new one.
    #[test]
    fn redo_puts_back_what_undo_took() {
        let root = set(
            1,
            vec![Member {
                offset: 0.0,
                dur: None,
                node: event(2),
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
            matches!(redone[0].effective, Intent::Place { offset, .. } if offset == 4.0),
            "{:?}",
            redone[0].effective
        );
        assert!(!owner.can_redo(), "and there is nothing further forward");
    }

    /// The translation, and the two ways it declines: a payload it does not
    /// know, and a widget bound to no node. Either would be editing on a guess.
    #[test]
    fn a_payload_becomes_an_intent_only_where_it_can_be_read() {
        // One unit to the beat, so the payload's numbers *are* beats and the
        // test is about the translation rather than about the scale.
        let mut owner = Owner::new(Document::new(event(1))).with_units_per_beat(1.0);
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
    /// which is what makes one owner answer both.
    #[test]
    fn a_sample_and_a_stroke_are_one_intent() {
        let mut owner = Owner::new(Document::new(event(1)));
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
        assert_eq!(label, "edit a sample");
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
        assert_eq!(label, "draw");
        assert!(
            matches!(&many, Intent::WriteSamples { start, values, .. }
                     if *start == 12 && values.len() == 3),
            "{many:?}"
        );
    }

    /// The milestone's own acceptance, in the half a Rust test can run: a
    /// session is opened, edited by an intent the host translated, undone,
    /// redone and saved — and what comes back is the document as edited, in the
    /// format the crate defines and nothing here re-implements.
    #[test]
    fn a_session_opens_is_edited_undone_redone_and_saved() {
        let root = set(
            1,
            vec![Member {
                offset: 0.0,
                dur: None,
                node: event(2),
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
        let Body::Set { members, .. } = &reopened.document.root.body else {
            panic!("a set")
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
        let mut owner = Owner::new(Document::new(event(1)));
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
            Body::Set {
                grouping: Grouping::Concrete,
                members: vec![Member {
                    offset: 0.0,
                    dur: None,
                    node: Node::new(
                        NodeId(2),
                        Body::Event {
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
        let Body::Set { members, .. } = &owner.document.root.body else {
            panic!("a set")
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

    fn owner_with_a_clip() -> Owner {
        // One unit to the beat: these tests are about the verbs, not the scale.
        let mut owner = Owner::new(Document::new(Node::new(
            NodeId(1),
            Body::Set {
                grouping: Grouping::Concrete,
                members: vec![Member {
                    offset: 0.0,
                    dur: None,
                    node: Node::new(
                        NodeId(2),
                        Body::Event {
                            config: Opaque::default(),
                            fires: None,
                        },
                    ),
                }],
                config: Opaque::none(),
            },
        )))
        .with_units_per_beat(1.0);
        owner.bind(50, NodeId(2));
        owner
    }

    fn offset(owner: &Owner) -> f64 {
        let Body::Set { members, .. } = &owner.document.root.body else {
            panic!("a set")
        };
        members[0].offset
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
        assert!(host.answer_own(
            1,
            50,
            seq,
            &[
                OscType::String("clip".into()),
                OscType::Float(4.0),
                OscType::Float(0.0)
            ]
        ));
        assert_eq!(offset(host.owner.as_ref().unwrap()), 4.0);

        // Addressed to the window (id 1 here), not to the clip.
        let seq = host.outbox.borrow_mut().stamp(1, 1);
        assert!(host.answer_own(1, 1, seq, &[OscType::String("undo".into())]));
        assert_eq!(offset(host.owner.as_ref().unwrap()), 0.0, "taken back");

        let seq = host.outbox.borrow_mut().stamp(1, 1);
        assert!(host.answer_own(1, 1, seq, &[OscType::String("redo".into())]));
        assert_eq!(offset(host.owner.as_ref().unwrap()), 4.0, "and put back");
    }

    /// **An undo has to move the picture, not only the document.** A drag needs
    /// no help — the gesture already moved the clip on screen — so the failure
    /// this pins is the one that looks like the key doing nothing: the document
    /// goes back, the widget stays where the hand left it, and nothing on
    /// screen changes.
    #[test]
    fn an_undo_moves_the_widget_back_and_not_only_the_document() {
        use crate::host::widget::WidgetKind;

        let def_id = 1;
        let doc = Document::new(Node::new(
            NodeId(1),
            Body::Set {
                grouping: Grouping::Concrete,
                members: vec![Member {
                    offset: 0.0,
                    dur: Some(1.0),
                    node: Node::new(
                        NodeId(2),
                        Body::Event {
                            config: Opaque::default(),
                            fires: None,
                        },
                    ),
                }],
                config: Opaque::none(),
            },
        ));
        // Drawn as the session mode draws it, so the widget ids are real.
        let drawn = super::tree::draw(
            &doc,
            &super::tree::Look {
                first_id: def_id + 1,
                units_per_beat: 100.0,
                ..super::tree::Look::default()
            },
            "t",
        );
        let mut owner = Owner::new(doc).with_units_per_beat(100.0);
        for b in &drawn.bindings {
            owner.bind(b.widget, b.node);
        }
        let clip = drawn.bindings[0].widget;

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

        let offset_of = |host: &Host| match host.widget_kind(def_id, clip) {
            Some(WidgetKind::Clip { offset, .. }) => *offset,
            other => panic!("clip {clip} is {other:?}"),
        };
        assert_eq!(offset_of(&host), 0.0);

        // The edit a drag reports, in the widget's own unit.
        let seq = host.outbox.borrow_mut().stamp(def_id, clip);
        assert!(host.answer_own(
            def_id,
            clip,
            seq,
            &[
                OscType::String("clip".into()),
                OscType::Float(400.0),
                OscType::Float(100.0),
            ]
        ));
        assert_eq!(offset_of(&host), 400.0, "4 beats at 100 units a beat");

        // ...and taken back: the widget follows the document.
        let seq = host.outbox.borrow_mut().stamp(def_id, def_id);
        assert!(host.answer_own(def_id, def_id, seq, &[OscType::String("undo".into())]));
        assert_eq!(
            offset_of(&host),
            0.0,
            "the undo moved the picture, not only the document"
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
