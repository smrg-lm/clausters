//! The table of the crate's own vocabularies — **so nobody else has to keep
//! one**.
//!
//! A [`History`](crate::History) reads no vocabulary: an entry's payload is
//! opaque, and each leg names the structure it belongs to so a caller can route
//! it to whatever reads that domain. That is what lets one pile hold the
//! arrangement, a curve, a span of samples and a timeline at once.
//!
//! One thing does not survive that, and it is the reason this module exists:
//! **the coalesce key is a sentence in a vocabulary**. "The same thing done the
//! same way" is *place, node 7* for the arrangement and *this span of this
//! channel* for samples, so the pile cannot compute it and a caller recording
//! its own entry has to state it. Left there, every binding would spell every
//! domain's rule again — the divergence
//! [`log::coalesce_key`](crate::log::coalesce_key) was given a door of its own
//! to prevent, now with four vocabularies instead of one.
//!
//! So the domains are named here once and asked here once. A caller in any
//! language says which vocabulary its payload is written in and gets the
//! sentence back; a domain the crate does not know answers `None`, which is
//! also how a misspelled domain name stops being silent.

use serde::Serialize;

use crate::Opaque;
use crate::events::{EVENTS, Events};
use crate::history::Editable;
use crate::log::TREE;
use crate::points::{POINTS, Points};
use crate::samples::SAMPLES;

/// Every vocabulary this crate speaks, in registration order.
///
/// What a caller registers a structure under, and the whole of what
/// [`coalesce_key`] dispatches on.
pub const DOMAINS: [&str; 4] = [TREE, POINTS, SAMPLES, EVENTS];

/// Whether the crate knows this vocabulary.
pub fn known(domain: &str) -> bool {
    DOMAINS.contains(&domain)
}

/// What makes two of `domain`'s edits *the same thing done the same way*, or
/// `None` when the payload is not written in that vocabulary — or the domain is
/// not one the crate speaks.
///
/// `None` is a real answer and not only an error: a domain whose edits are not
/// comparable never coalesces, and a caller passing it on to
/// [`History::record`](crate::History::record) leaves the entry unkeyed, which
/// is the same thing said in the pile's own terms.
pub fn coalesce_key(domain: &str, payload: &Opaque) -> Option<String> {
    match domain {
        TREE => crate::log::intent_of(payload).map(|intent| crate::log::coalesce_key(&intent)),
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
/// the edit lands — the trait's own argument
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
    /// The payload that puts the structure back — read before the edit landed.
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
/// So this serves the two domains whose state *is* the data — a curve's points
/// and a timeline's events — which is also every domain a client holds as an
/// ordinary list.
pub fn edit(domain: &str, state: &Opaque, payload: &Opaque) -> Option<Edited> {
    match domain {
        POINTS => {
            let mut points: Points = serde_json::from_value(state.0.clone()).ok()?;
            edited(&mut points, payload)
        }
        EVENTS => {
            let mut events: Events = serde_json::from_value(state.0.clone()).ok()?;
            edited(&mut events, payload)
        }
        _ => None,
    }
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
