//! The pure half of the notation layer — the format-agnostic logic every
//! client shares, so a JS/wasm client rebinds it rather than reimplementing it.
//!
//! Three entry points, all wasm-safe and dependency-light:
//!
//! - [`svg_to_display_list`] walks a verovio SVG into a [`DisplayList`] — a
//!   SMuFL glyph-outline table keyed by codepoint plus placed glyphs, staff
//!   lines, stems, fills and text in verovio page units, each carrying the MEI
//!   `xml:id` it was engraved from. The host tessellates that; it knows nothing
//!   about MEI or verovio, which is what lets one host renderer serve every
//!   client. The producer that feeds this walk is a *native* libverovio (the
//!   `clausters-notation` crate) or a wasm verovio, both emitting the same SVG,
//!   so cross-client parity is structural.
//! - [`voice_to_mei`] lays a monophonic-per-slot [`Slot`] stream out into
//!   barred, tied MEI. A client reduces its own sequencing data (an `Event`
//!   run, a `Timeline`) to that voice — that reduction reads client-native
//!   types and stays per-client; this is the language-agnostic step below it.
//! - [`Score`] is the stateful, editable document: the order an edit is made
//!   in, when the layout has to be re-run and reloaded, and the undo stack of
//!   MEI snapshots. It drives an [`Engraver`] — the port a binding implements,
//!   and the only part of the layer that differs between a native caller and a
//!   page.
//! - [`cursor_track`] folds the engraver's timemap ([`TimemapEntry`]) together
//!   with that geometry into the playhead's [`Cursor`] track, joining the two on
//!   the `xml:id` they share. Pure over data the engraver already produced, so
//!   every client's playhead lands on the same pixel.
//!
//! The **libverovio binding** is not here: it calls a C++ library and lives in
//! `clausters-notation`, which implements [`Engraver`] over it. This module is
//! only what compiles to wasm — which is now the whole of the layer's logic,
//! the score model included, so a page runs the same state machine a window
//! does rather than a second one written in TypeScript.

mod cursors;
mod mei;
mod score;
mod svg;

pub use cursors::{Cursor, TimemapEntry, cursor_track};
pub use mei::{Slot, voice_to_mei};
pub use score::{Engraver, NoteEvent, Page, Score};
pub use svg::{DisplayList, Prim, svg_to_display_list};
