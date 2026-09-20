//! **The applications over the document**, written once.
//!
//! The project is built around three classic applications over one document --
//! the audio editor, the multitrack editor and the score editor. The document
//! crate holds the model and the editing crate holds the projections a
//! structure owes its endpoints. What neither holds is the **application**: the
//! window it opens, which controls sit beside the structure, and what each
//! thing a hand does in that window is answered with.
//!
//! # An application draws nothing
//!
//! It speaks the GUI protocol, the same one every client speaks: it answers a
//! window as a GuiDef and a correction as props, and the host draws them and
//! reports the gestures back. That is what lets one application run in three
//! places without knowing which:
//!
//! - **a Python script or a page** binds it through the C ABI or wasm and
//!   carries its answers over the socket to the host;
//! - **the GUI host with no client in the process** links it and hands its
//!   answers to itself.
//!
//! It is therefore never a dependency *of* the host's renderer and never one
//! *on* it: a client cannot link a renderer, so a type of the host's in here
//! would be a door no client could open.
//!
//! # What is here and what is the caller's
//!
//! Here: what an application decides -- the composition, and the props of every
//! widget it composed. The caller's: every fact of a running system -- the
//! widget ids a host hands out, the buffers a source was read into, the buses a
//! playback's meters write. They come in as arguments, which is the rule the
//! projections already follow.

pub mod editing;
pub mod multitrack;
pub mod samples;
pub mod turn;
