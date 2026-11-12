//! **The GUI host's drawing base**: what every picture the host draws is
//! made of, with nothing of a device, a widget or a window in it.
//!
//! Seven modules, in the order they read one another: the rectangle every
//! pass places, clips and hits by ([`rect`]); the colors as named roles
//! ([`theme`]); an outline's SVG path, scanned, filled and flattened
//! (`path`, the `path` feature); the batch of flat-colored triangles a
//! frame is drawn from ([`paint`]); the face, bitmap or outline, that draws
//! text into that batch and measures it ([`font`]); the sizes as named roles,
//! measured on that face ([`metrics`]); and the context a draw function takes,
//! the batch with the two role tables ([`draw`]). None of them names a module
//! above it, which is why it is a crate of its own: the host
//! (`clausters-gui`) draws with it and puts the batch on a GPU, and a test of
//! a picture needs nothing else.
//!
//! Its features are the host's, forwarded: `font-atlas` draws text with an
//! outline typeface through a glyph atlas (the bitmap face is the floor under
//! every build), and `path` reads outlines, which the score's glyphs and the
//! symbols a client sends as paths need. `probe` exposes the questions a test
//! asks a batch (its extent, its colors, its alphas) to the host's tests.

// Read as the host's developer documentation is, with
// `--document-private-items`: a module's docs name the private function or
// constant that does the work, and those links are correct in that mode, so
// the private-link lint is off as it is in the host; `broken_intra_doc_links`
// stays on.
#![allow(rustdoc::private_intra_doc_links)]

pub mod draw;
pub mod font;
pub mod metrics;
pub mod paint;
#[cfg(feature = "path")]
pub mod path;
pub mod rect;
pub mod theme;
