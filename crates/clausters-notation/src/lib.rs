//! libverovio binding for Clausters — engrave a score into resolution-independent
//! geometry, the native half of the notation layer.
//!
//! This crate owns everything that touches **libverovio**, a C++ engraving
//! library ([verovio](https://verovio.org)) reached through its C wrapper
//! (`third_party/verovio/tools/c_wrapper.h`): it lays a digital score (MEI,
//! MusicXML, ABC or Plaine & Easie) out into an SVG of SMuFL glyph outlines and
//! engraving strokes. The format-agnostic parts — walking that SVG into the
//! host's display list, and the MEI encoder — live in `clausters-core` instead,
//! so they compile to wasm; this crate is the piece that cannot.
//!
//! Behind the `verovio` feature, **off by default**: a plain build links no
//! libverovio and the crate is empty, exactly as a SynthDef-only server carries
//! no libfaust. The Python reference wheel turns the feature on
//! (`clients/python/build_native.py`). libverovio is located at build time
//! through `VEROVIO_PREFIX` and linked with a relocatable `DT_RPATH` (see
//! `build.rs`); its SMuFL resource data is resolved at run time (verovio bakes
//! the configure-time path in, overridable through `CLAUSTERS_VEROVIO`).
//!
//! Two ways in: `engrave_svg` is the one-shot form (load, draw, discard), and
//! `open` builds the stateful one — the document held open so it can be edited
//! and re-engraved against the same ids. The C ABI over both lives in
//! `clausters-ffi`.
//!
//! That stateful model is **not** this crate's: `Score` is
//! `clausters_core::notation::Score` over the `clausters_core::notation::Engraver`
//! port, which `Toolkit` implements here. The
//! order an edit is made in, the reload that keeps the timemap honest and the
//! undo stack of MEI snapshots are logic, and both clients run the same one —
//! this crate is what that logic calls when the engraver is a C++ library
//! rather than a wasm module in a page.

#[cfg(feature = "verovio")]
mod score;
#[cfg(feature = "verovio")]
mod verovio;

#[cfg(feature = "verovio")]
pub use score::{NoteEvent, Page, Score, open};
#[cfg(feature = "verovio")]
pub use verovio::{
    EngraveError, EngraveOptions, Toolkit, default_resource_path, engrave_svg, ffi_lock,
};
