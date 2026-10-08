//! The table of the crate's own vocabularies -- **so nobody else has to keep
//! one**.
//!
//! A [`History`](crate::History) reads no vocabulary: an entry's payload is
//! opaque, and each leg names the structure it belongs to so a caller can route
//! it to whatever reads that domain. That is what lets one pile hold the
//! multitrack, a curve, a span of samples and a timeline at once.
//!
//! One thing does not survive that, and it is the reason this module exists:
//! **the coalesce key is a sentence in a vocabulary**. "The same thing done the
//! same way" is *place, node 7* for the arrangement and *this span of this
//! channel* for samples, so the pile cannot compute it and a caller recording
//! its own entry has to state it. Left there, every binding would spell every
//! domain's rule again -- the divergence
//! [`log::coalesce_key`](crate::log::coalesce_key) was given a door of its own
//! to prevent, now with four vocabularies instead of one.
//!
//! So the domains are named here once and asked here once. A caller in any
//! language says which vocabulary its payload is written in and gets the
//! sentence back; a domain the crate does not know answers `None`, which is
//! also how a misspelled domain name stops being silent.

use serde::Serialize;

use crate::Opaque;
use crate::events::{EVENTS, EventSequence};
use crate::history::Editable;
use crate::log::TREE;
use crate::multitrack::Multitrack;
use crate::multitrack::edit::{MULTITRACK, MultitrackEdit};
use crate::points::{POINTS, Points};
use crate::samples::SAMPLES;

/// Every vocabulary this crate speaks, in registration order.
///
/// What a caller registers a structure under, and the whole of what
/// [`coalesce_key`] dispatches on.
pub const DOMAINS: [&str; 5] = [TREE, MULTITRACK, POINTS, SAMPLES, EVENTS];

/// Whether the crate knows this vocabulary.
pub fn known(domain: &str) -> bool {
    DOMAINS.contains(&domain)
}

/// What makes two of `domain`'s edits *the same thing done the same way*, or
/// `None` when the payload is not written in that vocabulary -- or the domain is
/// not one the crate speaks.
///
/// `None` is a real answer and not only an error: a domain whose edits are not
/// comparable never coalesces, and a caller passing it on to
/// [`History::record`](crate::History::record) leaves the entry unkeyed, which
/// is the same thing said in the pile's own terms.
pub fn coalesce_key(domain: &str, payload: &Opaque) -> Option<String> {
    match domain {
        TREE => crate::log::intent_of(payload).map(|intent| crate::log::coalesce_key(&intent)),
        MULTITRACK => crate::multitrack::edit::intent_of(payload)
            .map(|intent| crate::multitrack::edit::coalesce_key(&intent)),
        POINTS => crate::points::coalesce_key(payload),
        SAMPLES => crate::samples::coalesce_key(payload),
        EVENTS => crate::events::coalesce_key(payload),
        _ => None,
    }
}

/// One edit applied to a structure held **as its own state**: what the
/// structure now is, whether anything changed, and the payload that would put
/// it back.
///
/// Both directions in one answer, because the inverse has to be read *before*
/// the edit lands -- the trait's own argument
/// ([`Editable`]'s own), and a door that let a caller apply
/// first and read second would let it record the wrong thing. It is also what
/// makes the seam worth crossing at all: an edit and its inverse are one
/// vocabulary's rule, and a binding that had to compute the inverse itself
/// would be spelling that rule again per language.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Edited {
    /// The structure as it now stands, in its own vocabulary.
    pub state: Opaque,
    /// Whether anything moved. A resend states what is already there and is
    /// applied by nobody and recorded by nobody.
    pub applied: bool,
    /// Why not, when the payload was refused for a rule rather than for being a
    /// resend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The payload that puts the structure back -- read before the edit landed.
    /// `None` when the structure cannot describe it, which is what makes an
    /// edit unloggable rather than uninvertible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<Opaque>,
}

/// Apply `payload` to a structure this crate can hold as **JSON state**, and
/// say what it now is and what would put it back.
///
/// `None` for a vocabulary whose state does not live in a caller's hand:
///
/// - [`TREE`] has a door of its own ([`apply`](crate::apply) over a
///   [`Document`](crate::Document)), because a tree's edit needs a version to
///   check against and a grid to snap to, and neither is state a caller can
///   hand over in one value.
/// - [`SAMPLES`] is a **borrowed view** by construction
///   ([`Samples`](crate::Samples)): the frames are in a server buffer or in a
///   host's own memory, never in a JSON value, and a door that took them here
///   would be copying a take through a string per stroke. What that domain
///   shares is its vocabulary and its coalesce key, which are here; where its
///   state lives is the caller's, and reading a span back is what its inverse
///   costs.
///
/// [`MULTITRACK`] is served, and it is the case that shows what the [`TREE`]
/// entry above is really about. A multitrack's whole state *is* one JSON value the
/// caller holds, version included, so the door works -- it simply applies
/// against whatever that state says and snaps to nothing, which is exactly what
/// a client that just read the multitrack wants. An editor that has a grid, or a
/// claim about a picture drawn a moment ago, uses the typed door
/// ([`multitrack::edit::apply`](crate::multitrack::edit::apply)) instead. The
/// tree cannot be served this way for a different reason: what it edits is a
/// handle that lives across the seam, not a value.
///
/// So this serves the domains whose state *is* the data -- the multitrack, a curve's
/// points, a timeline's events -- which is also every domain a client holds as
/// an ordinary list.
pub fn edit(domain: &str, state: &Opaque, payload: &Opaque) -> Option<Edited> {
    match domain {
        MULTITRACK => {
            let mut multitrack: Multitrack = serde_json::from_value(state.0.clone()).ok()?;
            let mut editing = MultitrackEdit::new(&mut multitrack);
            let current = editing.current(payload);
            let applied = editing.apply(payload);
            Some(Edited {
                state: Opaque(serde_json::to_value(&multitrack).ok()?),
                applied: applied.applied,
                reason: applied.reason,
                current,
            })
        }
        POINTS => {
            let mut points: Points = serde_json::from_value(state.0.clone()).ok()?;
            edited(&mut points, payload)
        }
        EVENTS => {
            let mut events: EventSequence = serde_json::from_value(state.0.clone()).ok()?;
            edited(&mut events, payload)
        }
        _ => None,
    }
}

/// One edit of a gesture that landed: the payload, and the one that puts it
/// back, read before it did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pair {
    /// The payload as it was applied.
    pub forward: Opaque,
    /// What puts the structure back where this payload found it -- `None`
    /// for an edit the structure cannot describe back, which a gesture holds
    /// only when that edit is the whole of it: the version moves and no entry
    /// is left, as for any edit with no inverse.
    pub backward: Option<Opaque>,
}

/// A gesture applied as one: **every one of its edits, or none of them**.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gesture {
    /// The structure after the whole gesture, or as it was when the gesture
    /// was refused.
    pub state: Opaque,
    /// Whether anything changed. A gesture made only of resends changes
    /// nothing and is refused by nobody.
    pub applied: bool,
    /// Why the gesture was refused, when one of its edits was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The edits that landed, in the order they did: what a history records
    /// as the gesture's legs, undone in the reverse order.
    pub pairs: Vec<Pair>,
}

/// Apply the edits one gesture came to -- an `Intake`'s payloads -- to a
/// structure held as its own state, **all of them or none**.
///
/// A gesture is one thing a hand did, so it lands whole or not at all: an edit
/// of it that is refused, or that the vocabulary cannot read, refuses the
/// gesture, and the state comes back as it was. So does an edit the structure
/// cannot describe back among several -- an entry holding the others would
/// undo only part of what the hand did; alone, it is an edit with no inverse
/// like any other, and lands without an entry. A gesture half applied would
/// leave a structure no window drew. A resend inside a gesture is not a
/// refusal: it changed nothing, and the rest still lands.
///
/// The one door every caller applies a gesture through -- the applications
/// over the document and both clients' handlers -- so the rule is written
/// once. `None` only when [`edit`] would answer it: a vocabulary whose state
/// does not live in a caller's hand, or a state that will not read.
pub fn edit_all(domain: &str, state: &Opaque, payloads: &[Opaque]) -> Option<Gesture> {
    if !matches!(domain, MULTITRACK | POINTS | EVENTS) {
        return None;
    }
    let refused = |reason: String| Gesture {
        state: state.clone(),
        applied: false,
        reason: Some(reason),
        pairs: Vec::new(),
    };
    let mut held = state.clone();
    let mut pairs = Vec::new();
    for payload in payloads {
        // `None` is the domain's or the state's, never a payload's: a
        // payload the vocabulary cannot read is refused with a reason
        let edited = edit(domain, &held, payload)?;
        if !edited.applied {
            match edited.reason {
                Some(reason) => return Some(refused(reason)),
                None => continue,
            }
        }
        pairs.push(Pair {
            forward: payload.clone(),
            backward: edited.current,
        });
        held = edited.state;
    }
    if pairs.len() > 1 && pairs.iter().any(|pair| pair.backward.is_none()) {
        return Some(refused(
            "an edit of this gesture could not be undone with the rest, so none of it is made"
                .into(),
        ));
    }
    Some(Gesture {
        applied: !pairs.is_empty(),
        state: held,
        reason: None,
        pairs,
    })
}

/// The two directions, in the order that makes them true: the inverse first.
fn edited<E: Editable + Serialize>(structure: &mut E, payload: &Opaque) -> Option<Edited> {
    let current = structure.current(payload);
    let applied = structure.apply(payload);
    Some(Edited {
        state: Opaque(serde_json::to_value(&*structure).ok()?),
        applied: applied.applied,
        reason: applied.reason,
        current,
    })
}

#[cfg(test)]
mod tests;
