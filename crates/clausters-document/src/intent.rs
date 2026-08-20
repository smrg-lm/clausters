//! What an edit **is**, and the one place that applies one.
//!
//! An [`Intent`] is an edit expressed in the owner's terms: not a pixel delta,
//! not a gesture, not a widget. A GUI host produces one from a gesture, a client
//! produces one from a script, and both hand it here — because the crate's
//! central discipline is that **nothing else applies an intent**. A caller does
//! not apply and then report; it hands over the document and the intent and
//! receives the new document plus an [`Outcome`]. One implementation of the edit
//! semantics, in one language, is what keeps three clients from meaning three
//! different things by the same edit.
//!
//! # Every intent is absolute
//!
//! An intent states the **resulting value**, never the increment: `Place` says
//! where the node now sits, `Configure` says what its configuration now is.
//! Nothing here says "move by", "transpose by" or "gain by". Two things follow,
//! and both are load-bearing:
//!
//! - **Nothing has to be replayed.** A view that drew an edit optimistically can
//!   leave its picture standing over whatever authoritative state arrives, with
//!   nothing to recompute — which is what lets the host draw immediately without
//!   holding an executable copy of the document.
//! - **An intent is idempotent.** Applying one twice leaves the same document,
//!   so a resend over a lossy leg is harmless.
//!
//! Expressing an edit in the owner's terms is a rule about **units**, not about
//! deltas: a pitch travels as a pitch and a beat as a beat, but as the value it
//! became. A transposition is a [`Intent::Configure`] carrying the resulting
//! pitches, which is why no `transpose` intent exists here.
//!
//! # The outcome is what the acknowledgement carries
//!
//! [`apply`] never returns a bare success. It returns the **effective** intent —
//! the edit that describes the document as it now stands — so that *applied
//! verbatim*, *applied transformed* (a snap, a clamp) and *refused* are one
//! shape. A refusal is the previous value handed back, not an error: the caller
//! adopts what it is given either way, and only the log cares which happened.
//!
//! # Staleness is detected, never rebased
//!
//! An absolute intent needs no rebase — that is what makes it absolute — but it
//! still needs to know whether the document moved underneath the picture it was
//! made against, because "absolute" and "safe" are not the same thing: a
//! [`Intent::SetMembers`] states an aggregate's contents *whole*, so one made against a
//! stale picture silently deletes whatever arrived in between. The document can
//! move by routes that are not gestures at all — a script editing the
//! arrangement, a second editor, a re-render — and none of them is visible to a
//! log.
//!
//! So [`apply`] takes an [`Against`]: the state the editor believed it was
//! editing. When that state has been superseded the edit is **refused as stale**
//! and the current value handed back, which needs no new path on either side —
//! the caller adopts the returned value exactly as it adopts a snap or a
//! refusal, and [`Outcome::stale`] is there for the one thing that does differ,
//! which is what to tell the person: *someone else changed this*, not *not
//! here*.
//!
//! Refusing rather than merging is deliberate and conservative. Merging two
//! absolute edits means deciding which one wins per field, which is a document
//! format's decision and not an edit vocabulary's, and getting it wrong loses
//! work silently — the failure this whole mechanism exists to make impossible.
//! An [`Against::unstated`] skips the check entirely, which is what a script
//! that just read the document wants, and what an older client looks like.

use crate::{Beats, Body, Document, Member, Node, NodeId, Opaque};
use serde::{Deserialize, Serialize};

/// How the owner transforms an edit as it applies it.
///
/// The transformations live here rather than in the caller for the same reason
/// `apply` does: a view that snapped an edit itself would be a second
/// implementation of the rule, and the two would disagree exactly where nobody
/// looks. What the caller learns is the [`Outcome`]'s effective value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rules {
    /// The musical grid a placement snaps to, in beats. Zero snaps nothing.
    pub quant: Beats,
}

impl Rules {
    /// No transformation at all: what a caller wants when the edit is already
    /// in the terms the document keeps.
    pub fn none() -> Self {
        Self::default()
    }

    /// Snapping to a grid of `quant` beats.
    pub fn quantized(quant: Beats) -> Self {
        Self { quant }
    }

    fn snap(&self, beats: Beats) -> Beats {
        if self.quant <= 0.0 {
            return beats;
        }
        (beats / self.quant).round() * self.quant
    }
}

/// The state an edit was made against.
///
/// The two counters, named by whoever is proposing the edit. They are separate
/// because they answer different questions and one number cannot do both: the
/// **document version** moves when the description changes (a clip is placed, an
/// aggregate is rewritten), the **source generation** moves when a source's *content*
/// changes while its identity stays put (a pencil stroke). A reader that holds
/// no document at all — a waveform view over one source — can name a generation
/// and nothing else, which is why the generation is optional rather than a
/// second required field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Against {
    /// The document version the editor was looking at. Zero means **unstated**,
    /// and unstated skips the check — a script that just read the document is
    /// not editing a stale picture, and an older client cannot say.
    #[serde(default)]
    pub version: u64,
    /// The generation of the samples the editor was looking at, when it was
    /// looking at samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

impl Against {
    /// No claim at all: apply without checking.
    pub fn unstated() -> Self {
        Self::default()
    }

    /// Made against this document version.
    pub fn at(version: u64) -> Self {
        Self {
            version,
            generation: None,
        }
    }

    /// Also made against this generation of the samples.
    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = Some(generation);
        self
    }

    /// Whether this names a document version at all.
    pub fn is_stated(&self) -> bool {
        self.version != 0
    }
}

/// An edit, in the owner's terms and stating the value it results in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "lowercase")]
pub enum Intent {
    /// Where a node sits inside the aggregate that holds it.
    ///
    /// The node names itself rather than its index, so a placement survives its
    /// siblings moving — which is what an edit made against a picture drawn a
    /// moment ago depends on.
    Place {
        /// The node being placed.
        node: NodeId,
        /// Its offset within its parent, in beats.
        offset: Beats,
        /// Its placement length, or the element's own when `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dur: Option<Beats>,
    },
    /// What a leaf's configuration now is, whole.
    ///
    /// Whole rather than a patch, because a patch is a delta by another name:
    /// two overlapping patches applied out of order give two different results,
    /// and the absolute rule exists precisely to make order stop mattering.
    Configure {
        /// The node being configured.
        node: NodeId,
        /// Its configuration, replacing whatever was there.
        config: Opaque,
    },
    /// What an aggregate contains now, whole.
    ///
    /// The roll's edit: notes added, moved and removed arrive as the resulting
    /// list. Members keep their ids, so what survived an edit is still the same
    /// node to a log and to a view.
    SetMembers {
        /// The aggregate being rewritten.
        node: NodeId,
        /// Its members, in whatever order the owner keeps them.
        members: Vec<Member>,
    },
    /// What a span of a node's samples now holds.
    ///
    /// The destructive edit: a dragged sample, a pencil stroke. It names the
    /// node rather than the source so that the document decides which samples
    /// a node refers to, and it carries values rather than a delta for the same
    /// reason every other intent does.
    WriteSamples {
        /// The node whose samples are written.
        node: NodeId,
        /// **Which channel of those samples** the span belongs to.
        ///
        /// A frame span already addresses the samples' shape, and a channel
        /// is the same kind of coordinate — not a fact about the source, which
        /// stays the source's business. It is a channel rather than a run of
        /// interleaved frames because an edit is usually *one* channel of one:
        /// carrying every channel would double a stereo stroke's inverse to
        /// say that the other channel did not change.
        ///
        /// Defaults to 0 when absent, which is what every mono edit means and
        /// what a document written before this field says.
        #[serde(default)]
        channel: u32,
        /// First frame of the span, in that channel.
        start: u64,
        /// The values the span now holds.
        values: Vec<f32>,
    },
}

impl Intent {
    /// The node this edit addresses.
    pub fn node(&self) -> NodeId {
        match self {
            Intent::Place { node, .. }
            | Intent::Configure { node, .. }
            | Intent::SetMembers { node, .. }
            | Intent::WriteSamples { node, .. } => *node,
        }
    }
}

/// What applying an intent did, and what the document now says.
///
/// The shape the acknowledgement carries. There is no success-or-error split:
/// the caller adopts [`Outcome::effective`] whatever happened, and a refusal is
/// simply the previous value handed back. Only the log reads [`Outcome::applied`],
/// because only the log cares whether there is something to invert.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The edit describing the document as it now stands — the intent as given
    /// when it applied verbatim, the transformed one when the owner snapped or
    /// clamped it, and the **previous** value when it was refused.
    pub effective: Intent,
    /// Whether the document changed.
    pub applied: bool,
    /// Why it was refused, or why it was transformed. Optional because most
    /// outcomes have nothing to say, and present because an edit that springs
    /// back with no explanation teaches "sometimes it does not work".
    pub reason: Option<String>,
    /// Whether the refusal was **staleness** rather than a rule.
    ///
    /// The mechanism does not read this — a stale edit is refused like any
    /// other, and the caller adopts [`Outcome::effective`] either way. It exists
    /// because the two say different things to a person: a rule means *not
    /// here*, and staleness means *someone else changed this*, which is also the
    /// one case where re-proposing the same edit against the fresh state is a
    /// reasonable thing to offer.
    pub stale: bool,
}

impl Outcome {
    fn changed(effective: Intent) -> Self {
        Self {
            effective,
            applied: true,
            reason: None,
            stale: false,
        }
    }

    fn unchanged(effective: Intent) -> Self {
        Self {
            effective,
            applied: false,
            reason: None,
            stale: false,
        }
    }

    fn transformed(effective: Intent, reason: impl Into<String>) -> Self {
        Self {
            effective,
            applied: true,
            reason: Some(reason.into()),
            stale: false,
        }
    }

    fn refused(effective: Intent, reason: impl Into<String>) -> Self {
        Self {
            effective,
            applied: false,
            reason: Some(reason.into()),
            stale: false,
        }
    }

    fn superseded(effective: Intent, reason: impl Into<String>) -> Self {
        Self {
            stale: true,
            ..Self::refused(effective, reason)
        }
    }
}

/// Apply an edit to a document.
///
/// The only door. Bumps [`Document::version`] when the document changed, and
/// leaves it alone when it did not — a refusal is not an edit, and a version
/// that moved for one would make every other reader re-sync for nothing.
///
/// `against` is the state the editor believed it was editing; an edit made
/// against a superseded one is refused as stale rather than applied blind. Pass
/// [`Against::unstated`] to skip that check.
pub fn apply(
    document: &mut Document,
    intent: &Intent,
    against: &Against,
    rules: &Rules,
) -> Outcome {
    if let Some(stale) = superseded(document, intent, against) {
        return stale;
    }
    match intent {
        Intent::Place { node, offset, dur } => place(document, *node, *offset, *dur, rules),
        Intent::Configure { node, config } => configure(document, *node, config),
        Intent::SetMembers { node, members } => set_members(document, *node, members),
        Intent::WriteSamples {
            node,
            channel,
            start,
            values,
        } => write_samples(document, *node, *channel, *start, values),
    }
}

/// The staleness gate: `Some(outcome)` when the edit was made against a state
/// the document has left behind.
///
/// Two claims are checked and they are independent. The **document version**
/// catches the description moving — including by routes no log sees. The
/// **source generation** catches a source being rewritten while the description
/// stands still, which the document's version cannot express and which is the
/// case a sample editor lives in.
///
/// A claim ahead of the document is stale too, and it is the worse case rather
/// than a harmless one: it means the two are not talking about the same
/// document at all, and applying would write an edit meant for another piece.
fn superseded(document: &Document, intent: &Intent, against: &Against) -> Option<Outcome> {
    // A node the document does not hold has an answer already, and it is a
    // better one than "stale": let the ordinary refusal say "no such node".
    let current = current(document, intent)?;
    if against.is_stated() && against.version != document.version {
        let reason = if against.version < document.version {
            "the document changed since this edit was made"
        } else {
            "this edit was made against a different document"
        };
        return Some(Outcome::superseded(current, reason));
    }
    let seen = against.generation?;
    let held = generation(document, intent.node())?;
    (seen != held).then(|| Outcome::superseded(current, "the samples changed since this edit"))
}

/// The intent describing what the document says *now* about what `intent`
/// addresses — which is what a refusal of any kind hands back.
///
/// `None` when the document cannot describe it: the node is gone, or the body
/// holds nothing of that shape. Both already have their own refusals, with
/// better reasons than staleness.
pub(crate) fn current(document: &Document, intent: &Intent) -> Option<Intent> {
    let id = intent.node();
    match intent {
        Intent::Place { .. } => {
            let member = find_member(&document.root, id)?;
            Some(Intent::Place {
                node: id,
                offset: member.offset,
                dur: member.dur,
            })
        }
        Intent::Configure { .. } => {
            let node = document.find(id)?;
            Some(Intent::Configure {
                node: id,
                config: config(&node.body)?.clone(),
            })
        }
        Intent::SetMembers { .. } => {
            let node = document.find(id)?;
            match &node.body {
                Body::Aggregate { members, .. } | Body::Sequence { members, .. } => {
                    Some(Intent::SetMembers {
                        node: id,
                        members: members.clone(),
                    })
                }
                _ => None,
            }
        }
        // The samples are not in the document, so the only honest description
        // of the present state is the empty write: nothing to adopt, and the
        // generation in the acknowledgement is what tells the caller to re-read.
        Intent::WriteSamples { channel, start, .. } => {
            let node = document.find(id)?;
            matches!(node.body, Body::Vector { .. }).then_some(Intent::WriteSamples {
                node: id,
                channel: *channel,
                start: *start,
                values: Vec::new(),
            })
        }
    }
}

/// The generation of the samples a node names, for the nodes that name any.
fn generation(document: &Document, id: NodeId) -> Option<u64> {
    match &document.find(id)?.body {
        Body::Vector { source, .. } => Some(source.generation),
        // Assembled data is as fresh as its **stalest** piece: an edit made
        // against it was made against every window it shows, so any one of them
        // being rewritten is what a staleness check has to catch.
        Body::Segments { segments, .. } => segments.iter().map(|s| s.source.generation).max(),
        _ => None,
    }
}

fn place(
    document: &mut Document,
    id: NodeId,
    offset: Beats,
    dur: Option<Beats>,
    rules: &Rules,
) -> Outcome {
    let snapped = rules.snap(offset);
    let snapped_dur = dur.map(|d| rules.snap(d));
    let Some(member) = find_member_mut(&mut document.root, id) else {
        // Nothing to place, and nothing to hand back either: the caller's own
        // value is the only description of a node the document does not hold.
        return Outcome::refused(
            Intent::Place {
                node: id,
                offset,
                dur,
            },
            "no such node",
        );
    };
    let previous = Intent::Place {
        node: id,
        offset: member.offset,
        dur: member.dur,
    };
    if close(member.offset, snapped) && same_dur(member.dur, snapped_dur) {
        return Outcome::unchanged(previous);
    }
    member.offset = snapped;
    member.dur = snapped_dur;
    document.version += 1;
    let effective = Intent::Place {
        node: id,
        offset: snapped,
        dur: snapped_dur,
    };
    if close(snapped, offset) && same_dur(snapped_dur, dur) {
        Outcome::changed(effective)
    } else {
        Outcome::transformed(effective, "snapped to the grid")
    }
}

fn configure(document: &mut Document, id: NodeId, config: &Opaque) -> Outcome {
    let Some(node) = find_mut(&mut document.root, id) else {
        return Outcome::refused(
            Intent::Configure {
                node: id,
                config: config.clone(),
            },
            "no such node",
        );
    };
    let Some(slot) = config_mut(&mut node.body) else {
        return Outcome::refused(
            Intent::Configure {
                node: id,
                config: Opaque::none(),
            },
            "this body holds no configuration",
        );
    };
    if slot == config {
        return Outcome::unchanged(Intent::Configure {
            node: id,
            config: config.clone(),
        });
    }
    *slot = config.clone();
    document.version += 1;
    Outcome::changed(Intent::Configure {
        node: id,
        config: config.clone(),
    })
}

fn set_members(document: &mut Document, id: NodeId, members: &[Member]) -> Outcome {
    let Some(node) = find_mut(&mut document.root, id) else {
        return Outcome::refused(
            Intent::SetMembers {
                node: id,
                members: members.to_vec(),
            },
            "no such node",
        );
    };
    let Some(slot) = members_mut(&mut node.body) else {
        // The case the client's silent no-op used to be: a body that holds
        // nothing placed cannot take a list of placed things. Refusing *and
        // saying so* is what turns a note springing back into an answer.
        return Outcome::refused(
            Intent::SetMembers {
                node: id,
                members: Vec::new(),
            },
            "this body holds no members",
        );
    };
    if slot == members {
        return Outcome::unchanged(Intent::SetMembers {
            node: id,
            members: members.to_vec(),
        });
    }
    *slot = members.to_vec();
    document.version += 1;
    Outcome::changed(Intent::SetMembers {
        node: id,
        members: members.to_vec(),
    })
}

fn write_samples(
    document: &mut Document,
    id: NodeId,
    channel: u32,
    start: u64,
    values: &[f32],
) -> Outcome {
    let refuse = |reason: &str| {
        Outcome::refused(
            Intent::WriteSamples {
                node: id,
                channel,
                start,
                values: Vec::new(),
            },
            reason.to_string(),
        )
    };
    let Some(node) = find_mut(&mut document.root, id) else {
        return refuse("no such node");
    };
    let Body::Vector { source, .. } = &mut node.body else {
        return refuse("only samples can be written");
    };
    if values.is_empty() {
        return Outcome::unchanged(Intent::WriteSamples {
            node: id,
            channel,
            start,
            values: Vec::new(),
        });
    }
    // The samples are not in the document -- the document describes where
    // samples are, never what they hold -- so what applying does here is bump the
    // source's generation, which is the signal every reader of those samples
    // needs in order to know its copy is stale. Writing the samples themselves
    // is the owner's next step, against the working buffer.
    source.generation += 1;
    document.version += 1;
    Outcome::changed(Intent::WriteSamples {
        node: id,
        channel,
        start,
        values: values.to_vec(),
    })
}

// ---- lookup ----

fn find_member(node: &Node, id: NodeId) -> Option<&Member> {
    let members = node.members();
    if let Some(member) = members.iter().find(|m| m.node.id == id) {
        return Some(member);
    }
    members.iter().find_map(|m| find_member(&m.node, id))
}

fn config(body: &Body) -> Option<&Opaque> {
    match body {
        Body::Clang { config, .. }
        | Body::Sequence { config, .. }
        | Body::Vector { config, .. }
        | Body::Segments { config, .. }
        | Body::Generator { config, .. } => Some(config),
        Body::Aggregate { .. } | Body::Unknown(_) => None,
    }
}

fn find_mut(node: &mut Node, id: NodeId) -> Option<&mut Node> {
    if node.id == id {
        return Some(node);
    }
    members_mut(&mut node.body)?
        .iter_mut()
        .find_map(|m| find_mut(&mut m.node, id))
}

fn find_member_mut(node: &mut Node, id: NodeId) -> Option<&mut Member> {
    let members = members_mut(&mut node.body)?;
    if let Some(index) = members.iter().position(|m| m.node.id == id) {
        return members.get_mut(index);
    }
    members
        .iter_mut()
        .find_map(|m| find_member_mut(&mut m.node, id))
}

fn members_mut(body: &mut Body) -> Option<&mut Vec<Member>> {
    match body {
        Body::Aggregate { members, .. } | Body::Sequence { members, .. } => Some(members),
        _ => None,
    }
}

fn config_mut(body: &mut Body) -> Option<&mut Opaque> {
    match body {
        Body::Clang { config, .. }
        | Body::Sequence { config, .. }
        | Body::Vector { config, .. }
        | Body::Segments { config, .. }
        | Body::Generator { config, .. } => Some(config),
        Body::Aggregate { .. } | Body::Unknown(_) => None,
    }
}

fn close(a: Beats, b: Beats) -> bool {
    (a - b).abs() <= crate::EPSILON
}

fn same_dur(a: Option<Beats>, b: Option<Beats>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => close(a, b),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
