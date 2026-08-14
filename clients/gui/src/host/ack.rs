//! The outbox: what the host emitted, and what the owner said about it.
//!
//! The host owns no data, so every edit it produces is a proposal it hands to
//! whoever does — and between the gesture and the answer there is a gap the
//! host has to draw across. This module is the bookkeeping that makes that gap
//! resolvable rather than a guess.
//!
//! # The mechanism, not the picture
//!
//! Two halves, and only the first is here. **This** half is the sequence
//! number stamped on every emitted `/gui_event`, the set of edits still in
//! flight, and the one rule that retires them: *drop every pending at or below
//! the stamp, and adopt what arrived*. The other half — what a pending edit
//! **looks like** while it is in flight — belongs to whichever widget drew it,
//! because a pending sample and a pending clip do not look alike.
//!
//! # Why a sequence number and not a version
//!
//! They answer different questions and the design carries both. The sequence
//! says *which of my gestures is this an answer to*, which is what a host with
//! two edits in flight needs and what nothing else can supply. The versions —
//! the document's, and each source's generation — say *are we talking about the
//! same state*, which is what catches the document moving by a route that was
//! not a gesture at all: a script editing the arrangement, a second editor, a
//! re-render.
//!
//! # There is no branch for a refusal
//!
//! An owner answers an intent by pushing the state that now holds, stamped with
//! the last sequence it processed. Applied verbatim, applied transformed and
//! refused are the same message: the value is what the document says, and a
//! refusal is simply the previous value. So nothing here asks whether an edit
//! succeeded — it retires what the stamp covers and lets the pushed state be
//! the truth. The optional reason exists for the *interface*, not for the
//! mechanism: an edit that springs back with no explanation teaches "sometimes
//! it does not work" rather than "not here".

use std::collections::HashMap;

/// One edit the host emitted and has not heard back about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The stamp it went out with.
    pub seq: i32,
    /// The window it came from.
    pub def_id: i32,
    /// The widget the gesture was on.
    pub widget_id: i32,
}

/// What an owner said: how far it has processed, and what state that left.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Acked {
    /// The last stamp the owner processed. Monotonic, so one number retires
    /// everything at or below it — which is what makes a dropped or reordered
    /// acknowledgement harmless.
    pub seq: i32,
    /// The document's version after it.
    pub doc_version: i64,
    /// The generation of each source that moved, by source id. A destructive
    /// edit changes a source's content while its identity stays put, so this is
    /// the only thing that can tell a reader its copy is stale.
    pub generations: HashMap<i32, i64>,
    /// Why an edit was refused or transformed, when the owner had something to
    /// say. Informational: nothing in the mechanism reads it.
    pub reason: Option<String>,
}

/// The host's outbox for one client: the stamps it issues and the edits still
/// in flight.
#[derive(Debug, Default)]
pub struct Outbox {
    next: i32,
    pending: Vec<Pending>,
    last: Option<Acked>,
}

impl Outbox {
    /// Stamps an outgoing edit and records it as in flight.
    ///
    /// Stamps start at 1, so a zero on the wire reads as *unstamped* rather
    /// than as the first edit of the session.
    pub fn stamp(&mut self, def_id: i32, widget_id: i32) -> i32 {
        self.next = self.next.wrapping_add(1).max(1);
        let seq = self.next;
        self.pending.push(Pending {
            seq,
            def_id,
            widget_id,
        });
        seq
    }

    /// Takes an acknowledgement: retires every pending edit at or below its
    /// stamp and returns them, newest last.
    ///
    /// Returning them rather than swallowing them is what lets a front drop the
    /// right overlays — the host core knows *which* edits are settled, and only
    /// the front knows what they were drawn as.
    pub fn ack(&mut self, acked: Acked) -> Vec<Pending> {
        let (settled, still_open): (Vec<_>, Vec<_>) =
            self.pending.drain(..).partition(|p| p.seq <= acked.seq);
        self.pending = still_open;
        self.last = Some(acked);
        settled
    }

    /// The edits still in flight, oldest first.
    pub fn pending(&self) -> &[Pending] {
        &self.pending
    }

    /// Whether this widget has an edit in flight — what a front asks before it
    /// decides whether it is drawing a pending value or the owner's.
    pub fn is_pending(&self, def_id: i32, widget_id: i32) -> bool {
        self.pending
            .iter()
            .any(|p| p.def_id == def_id && p.widget_id == widget_id)
    }

    /// The last thing the owner said, or `None` before it has said anything.
    pub fn last(&self) -> Option<&Acked> {
        self.last.as_ref()
    }

    /// The generation this owner last reported for a source, if any.
    pub fn generation(&self, source: i32) -> Option<i64> {
        self.last.as_ref()?.generations.get(&source).copied()
    }

    /// Forgets everything about a window — what a `/gui_free` leaves behind.
    ///
    /// An edit naming a widget that no longer exists has nothing to resolve to,
    /// and keeping it would hold the pending set open forever against an
    /// acknowledgement that is never coming.
    pub fn forget(&mut self, def_id: i32) {
        self.pending.retain(|p| p.def_id != def_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_is_monotonic_and_never_zero() {
        let mut outbox = Outbox::default();
        assert_eq!(outbox.stamp(1, 10), 1);
        assert_eq!(outbox.stamp(1, 11), 2);
        // Zero is reserved for "unstamped", which is what a message from an
        // older client looks like.
        assert!(outbox.pending().iter().all(|p| p.seq > 0));
    }

    #[test]
    fn one_acknowledgement_retires_everything_at_or_below_it() {
        // Which is what makes a dropped acknowledgement harmless: the next one
        // covers what the lost one would have.
        let mut outbox = Outbox::default();
        let a = outbox.stamp(1, 10);
        let b = outbox.stamp(1, 11);
        let c = outbox.stamp(1, 12);

        let settled = outbox.ack(Acked {
            seq: b,
            ..Acked::default()
        });
        assert_eq!(
            settled.iter().map(|p| p.seq).collect::<Vec<_>>(),
            vec![a, b]
        );
        assert_eq!(outbox.pending().len(), 1);
        assert_eq!(outbox.pending()[0].seq, c);
    }

    #[test]
    fn two_gestures_in_flight_resolve_independently() {
        // The case a sequence number exists for: without one, two edits on the
        // same window are indistinguishable to whatever comes back.
        let mut outbox = Outbox::default();
        let first = outbox.stamp(1, 10);
        let second = outbox.stamp(1, 11);

        assert!(outbox.is_pending(1, 10) && outbox.is_pending(1, 11));
        outbox.ack(Acked {
            seq: first,
            ..Acked::default()
        });
        assert!(!outbox.is_pending(1, 10), "the first one settled");
        assert!(outbox.is_pending(1, 11), "the second one is still out");
        assert_eq!(outbox.pending()[0].seq, second);
    }

    #[test]
    fn an_acknowledgement_carries_the_state_it_left() {
        let mut outbox = Outbox::default();
        let seq = outbox.stamp(1, 10);
        outbox.ack(Acked {
            seq,
            doc_version: 7,
            generations: HashMap::from([(4, 2)]),
            reason: Some("snapped to the grid".into()),
        });
        assert_eq!(outbox.last().unwrap().doc_version, 7);
        assert_eq!(outbox.generation(4), Some(2));
        assert_eq!(outbox.generation(9), None);
    }

    #[test]
    fn freeing_a_window_drops_what_it_had_in_flight() {
        // Otherwise the pending set stays open against an acknowledgement that
        // is never coming, and the host waits forever for a widget that is gone.
        let mut outbox = Outbox::default();
        outbox.stamp(1, 10);
        let other = outbox.stamp(2, 20);
        outbox.forget(1);
        assert_eq!(outbox.pending().len(), 1);
        assert_eq!(outbox.pending()[0].seq, other);
    }
}
