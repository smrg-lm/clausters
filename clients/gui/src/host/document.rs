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

    /// Snapping placements to a grid of `quant` beats (0 snaps nothing).
    pub fn with_quant(mut self, quant: f64) -> Self {
        self.rules = Rules { quant };
        self
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
            "clip" => {
                let offset = float_at(args, 1)? as f64;
                let dur = float_at(args, 2).map(|d| d as f64).filter(|d| *d > 0.0);
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
                        start,
                        values,
                    },
                    "draw",
                ))
            }
            _ => None,
        }
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
        let mut owner = Owner::new(Document::new(event(1)));
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
