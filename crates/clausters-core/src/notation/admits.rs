//! **What a hand may do to an engraved element**, by what the element is.
//!
//! A page used to be editable or not as a whole, and the host -- to which an id
//! is an id -- answered every element the same way: a slur, a time signature,
//! a rest and a staff dragged like a notehead and grew ledger lines on the way.
//! The engraving walk knows what each id is ([`super::DisplayList::kinds`]);
//! this is the one table of what each kind admits, read by every renderer, so
//! the rule exists once rather than once per host or per client.
//!
//! **Where an element sits is the engraver's.** No element is placed by
//! dragging it: a slur's ends are notes, a dynamic belongs to the note it is
//! written on, and the spacing of a system is not a statement anybody made. So
//! the only drag a page offers is the one that changes what is *written* -- a
//! note's pitch -- and everything else is selected and edited by a verb.

use serde::Serialize;

/// The gestures an element of one kind admits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Admits {
    /// A vertical drag moves it by diatonic steps along its staff, and the page
    /// draws the ledger lines a notehead needs on the way -- what a written
    /// pitch is, and nothing else on a page is.
    pub pitch: bool,
}

/// What an element of `kind` admits. `kind` is the engraver's class for it, as
/// [`super::DisplayList::kinds`] names it; a kind this table does not know
/// admits nothing beyond being selected, which every element is.
///
/// A note's accidental, stem, flag and dots are not kinds of their own: the
/// walk gives them the note's id, so a press on the sign is a press on the
/// note and its drag is the note's.
#[must_use]
pub fn admits(kind: &str) -> Admits {
    Admits {
        pitch: kind == "note",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_note_drags_its_pitch() {
        assert!(admits("note").pitch);
        for kind in [
            "rest", "mRest", "chord", "slur", "tie", "hairpin", "dynam", "clef", "keySig",
            "meterSig", "barLine", "staff", "beam", "tuplet", "",
        ] {
            assert!(!admits(kind).pitch, "{kind} drags a pitch");
        }
    }
}
