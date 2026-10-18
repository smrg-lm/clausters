//! **The audio editor**: a take edited as a list of parts over immutable takes.
//!
//! What it opens is **a file or a server buffer**, and it never writes a buffer
//! that exists while it edits: the window draws a **join** the editor owns
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
//! # A pasted block keeps its own rate
//!
//! A drawn take uses the edited file's rate. A pasted block keeps its source
//! rate and the parts list reads it at the existing ratio. Paste mixed needs
//! matching rates and channels; no implicit resampling or channel conversion
//! is performed. Clipboard publications and readers use the application
//! exchange rather than history entries.

pub mod dialogs;
pub mod editor;
