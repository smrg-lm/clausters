//! **The audio editor**: a take edited as a list of parts over immutable files.
//!
//! What it opens is **a file**, read where it is, and it never writes a file
//! a list reads: every edit leaves a new list -- [`clausters_document::parts`]
//! over the files it names. A cut takes a span out of the list; a paste puts
//! the copy's parts in; a pencil stroke, a mix, a gain and a reverse render
//! **a new file** into the editor's working folder and splice it over the
//! frames they changed. Nothing that a history entry names is ever written
//! again, so an undo is the list before.
//!
//! # What a take is here
//!
//! The list held as a tree of shared nodes ([`clausters_document::parts::Tree`]),
//! so an edit makes the nodes on the paths to its seams and shares the rest.
//! What holds a take holds a reference to it ([`shared`]): the clipboard names
//! a copy the editor keeps, a history entry names the take before and after
//! by number, and the window is handed the tree. A take of tens of thousands
//! of parts is edited, copied, undone and drawn in what a short one is.
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
pub mod renders;
pub mod shared;
