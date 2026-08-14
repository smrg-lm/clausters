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

/// An edit, in the owner's terms and stating the value it results in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "lowercase")]
pub enum Intent {
    /// Where a node sits inside the set that holds it.
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
    /// What a set contains now, whole.
    ///
    /// The roll's edit: notes added, moved and removed arrive as the resulting
    /// list. Members keep their ids, so what survived an edit is still the same
    /// node to a log and to a view.
    SetMembers {
        /// The set being rewritten.
        node: NodeId,
        /// Its members, in whatever order the owner keeps them.
        members: Vec<Member>,
    },
    /// What a span of a node's material now holds.
    ///
    /// The destructive edit: a dragged sample, a pencil stroke. It names the
    /// node rather than the source so that the document decides which material
    /// a node refers to, and it carries values rather than a delta for the same
    /// reason every other intent does.
    WriteSamples {
        /// The node whose material is written.
        node: NodeId,
        /// First frame of the span.
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
}

impl Outcome {
    fn changed(effective: Intent) -> Self {
        Self {
            effective,
            applied: true,
            reason: None,
        }
    }

    fn transformed(effective: Intent, reason: impl Into<String>) -> Self {
        Self {
            effective,
            applied: true,
            reason: Some(reason.into()),
        }
    }

    fn refused(effective: Intent, reason: impl Into<String>) -> Self {
        Self {
            effective,
            applied: false,
            reason: Some(reason.into()),
        }
    }
}

/// Apply an edit to a document.
///
/// The only door. Bumps [`Document::version`] when the document changed, and
/// leaves it alone when it did not — a refusal is not an edit, and a version
/// that moved for one would make every other reader re-sync for nothing.
pub fn apply(document: &mut Document, intent: &Intent, rules: &Rules) -> Outcome {
    match intent {
        Intent::Place { node, offset, dur } => place(document, *node, *offset, *dur, rules),
        Intent::Configure { node, config } => configure(document, *node, config),
        Intent::SetMembers { node, members } => set_members(document, *node, members),
        Intent::WriteSamples {
            node,
            start,
            values,
        } => write_samples(document, *node, *start, values),
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
        return Outcome {
            effective: previous,
            applied: false,
            reason: None,
        };
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
        return Outcome {
            effective: Intent::Configure {
                node: id,
                config: config.clone(),
            },
            applied: false,
            reason: None,
        };
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
        return Outcome {
            effective: Intent::SetMembers {
                node: id,
                members: members.to_vec(),
            },
            applied: false,
            reason: None,
        };
    }
    *slot = members.to_vec();
    document.version += 1;
    Outcome::changed(Intent::SetMembers {
        node: id,
        members: members.to_vec(),
    })
}

fn write_samples(document: &mut Document, id: NodeId, start: u64, values: &[f32]) -> Outcome {
    let refuse = |reason: &str| {
        Outcome::refused(
            Intent::WriteSamples {
                node: id,
                start,
                values: Vec::new(),
            },
            reason.to_string(),
        )
    };
    let Some(node) = find_mut(&mut document.root, id) else {
        return refuse("no such node");
    };
    let Body::Buffer { source, .. } = &mut node.body else {
        return refuse("only material can be written");
    };
    if values.is_empty() {
        return Outcome {
            effective: Intent::WriteSamples {
                node: id,
                start,
                values: Vec::new(),
            },
            applied: false,
            reason: None,
        };
    }
    // The samples are not in the document -- the document describes where
    // material is, never what it holds -- so what applying does here is bump the
    // source's generation, which is the signal every reader of that material
    // needs in order to know its copy is stale. Writing the samples themselves
    // is the owner's next step, against the working buffer.
    source.generation += 1;
    document.version += 1;
    Outcome::changed(Intent::WriteSamples {
        node: id,
        start,
        values: values.to_vec(),
    })
}

// ---- lookup ----

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
        Body::Set { members, .. } | Body::Sequence { members, .. } => Some(members),
        _ => None,
    }
}

fn config_mut(body: &mut Body) -> Option<&mut Opaque> {
    match body {
        Body::Event { config, .. }
        | Body::Sequence { config, .. }
        | Body::Buffer { config, .. }
        | Body::Generator { config } => Some(config),
        Body::Set { .. } | Body::Unknown(_) => None,
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
