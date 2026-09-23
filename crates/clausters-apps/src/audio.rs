//! **The audio editor**: a take edited as a list of parts over immutable takes.
//!
//! The samples editor writes a stroke into the buffer it draws. This one never
//! writes a buffer that exists: the window draws a **join** the editor owns
//! the recipe of, and every edit leaves a new recipe --
//! [`clausters_document::parts`] over the takes it names. A cut takes a span
//! out of the list; a paste puts a new take in; a pencil stroke writes **a new
//! take the size of the stroke** and splices it over the frames it was drawn
//! on. Nothing that a history entry names is ever written again, so an undo is
//! the list before, stitched, and costs the list and not the samples.
//!
//! # What a take is here
//!
//! A server buffer, and its source id is its number. The buffers the editor
//! writes new takes into are the caller's, handed over in advance (`sync`'s
//! `buffers`): a turn that needs one takes the next, and one that finds none is
//! refused with the reason rather than inventing a number some other part of
//! the program owns. What the history lets go of comes back to the caller as
//! buffers to free, through the editing context ([`crate::editing`]).
//!
//! # Every take is at the edited take's rate
//!
//! A take drawn or pasted is allocated at the rate of the take the editor
//! opened, whatever the server runs at, so a list is always one rate and a
//! frame of it is a frame of every part ([`clausters_document::parts`]). A
//! pasted block at another rate is refused: resampling is an edit, and nothing
//! here asked for one.

pub mod editor;
