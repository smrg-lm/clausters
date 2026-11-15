//! The **presentation** every application shares: what a picture of a
//! structure is drawn with, and which of a window's gestures are not edits.
//!
//! None of it is a structure of the document. The [`catalogue`] says what a
//! view of each structure is drawn from, the [`layers`] what a signal's
//! picture measures, and [`NOT_AN_EDIT`] which `/gui_event` tags name screen
//! state rather than an edit -- the line every application draws between what
//! enters a history and what a window keeps for itself. A multitrack's own
//! window state, saved in its session file, is
//! [`crate::multitrack::view`].

pub mod catalogue;
pub mod layers;

/// The `/gui_event` tags that are **not** edits of the structure: what a view
/// is looking at, and where the hand is.
///
/// The applications say the rule in prose -- a selection and a zoom are each
/// window's and never the multitrack's -- and the routing table is that rule as
/// a value, so a client can obey it without restating it. It is here rather
/// than in a client because it was written twice, once per language, and two
/// copies of a list of words drift the way every duplicated table drifts: a tag
/// added to one of them makes that client answer a gesture the other one edits
/// with.
///
/// What a client does with the tags is still the client's: it writes them into
/// its own screen state, which is not this crate's to hold. What is settled
/// here is only *which ones* those are.
pub const NOT_AN_EDIT: [&str; 9] = [
    "selection",
    "view",
    "view_x",
    "view_y",
    "layer",
    "focus",
    "locate",
    "height",
    "port_hold",
];

/// Whether `tag` names screen state rather than an edit -- [`NOT_AN_EDIT`] asked
/// of one tag.
#[must_use]
pub fn is_screen_state(tag: &str) -> bool {
    NOT_AN_EDIT.contains(&tag)
}

#[cfg(test)]
mod tests;
