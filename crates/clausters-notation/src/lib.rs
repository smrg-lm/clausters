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
//! This is the G31h-a surface — the binding and one-shot engraving to SVG. The
//! stateful editable `Score` and the full C-ABI are the later steps of G31h.

#[cfg(feature = "verovio")]
mod verovio;

#[cfg(feature = "verovio")]
pub use verovio::{
    EngraveError, EngraveOptions, Toolkit, default_resource_path, engrave_svg, ffi_lock,
};
