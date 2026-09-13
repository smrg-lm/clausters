//! **The structures a hand edits**, and the verbs over them — apart from what
//! draws them.
//!
//! A note, a box on a time axis, a break-point: each is a shape with a list
//! behind it and a handful of verbs that act on a selection of one. They were
//! written under [`graphics`](super::graphics), beside the code that draws
//! them, because that is where the first one needed them — so the module whose
//! name says *drawing* held the model, its edits and its picture, in that order
//! of importance and in the wrong place. Nobody looking for where a note is cut
//! looked under `graphics`.
//!
//! # Why it is worth a layer of its own
//!
//! Three applications are being built over one document — an audio editor, a
//! multitrack editor, a score editor — and **the structures are the part they
//! share**. What differs between them is the picture and the hand's vocabulary;
//! what does not is that a note is a note and a box is a box. A layer that
//! names them is what makes the next application a composition of things that
//! already exist rather than a fourth copy of them.
//!
//! The boundary is stated negatively, which is what makes it worth separating:
//! **nothing here names a `Rect`, a `View`, a `Draw` or a `Metrics`.** A module
//! that took one would be a drawing again. The mappings between a structure and
//! the pixels it is drawn at stay with the renderer that chose them
//! ([`graphics::pianoroll`](super::graphics::pianoroll) maps a pitch to a row,
//! [`interact::coords`](super::interact) maps a cursor to a sample), and both
//! hand the *numbers* here — the same rule [`boxes`] has stated since it was
//! the only module in this layer.
//!
//! # What is in it
//!
//! - [`boxes`] — **a box on a time axis**, which a clip and a note both are: a
//!   span on a row, grabbed by one of three parts, snapped, bounded. The
//!   geometry, and the verbs over a selection of them (cut, drop, put down).
//! - [`notes`] — **a note**, and the list verbs that are a roll's: insert,
//!   remove, move, resize, velocity, and the clipboard block.
//! - [`clips`] — **a box with contents on a lane**, plus the lane and the
//!   curve: what a multitrack is made of, and the payloads each list reports as.
//! - [`points`] — **a break-point**, the shape of an envelope and of an
//!   automation alike: where the points are, what the curve is worth between
//!   two of them, and the edits that move one.

pub mod boxes;
pub mod clips;
pub mod notes;
pub mod points;
