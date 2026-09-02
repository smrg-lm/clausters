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

use crate::Opaque;
use crate::events::EVENTS;
use crate::log::TREE;
use crate::points::POINTS;
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

#[cfg(test)]
mod tests;
