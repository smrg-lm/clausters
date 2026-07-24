//! The pure half of the notation layer — the format-agnostic logic every
//! client shares, so a JS/wasm client rebinds it rather than reimplementing it.
//!
//! Two entry points, both wasm-safe and dependency-light:
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
//!
//! The stateful editable score model and the libverovio binding are **not**
//! here: they are native (they call the C++ engraver) and live in
//! `clausters-notation`. This module is only what compiles to wasm.

mod mei;
mod svg;

pub use mei::{Slot, voice_to_mei};
pub use svg::{DisplayList, Prim, svg_to_display_list};
