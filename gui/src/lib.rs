//! Reusable building blocks for navigable audio views (waveform, spectrogram).
//!
//! - [`viewport`]: zoom/pan navigation shared by every view.
//! - [`peaks`]: resolution-matched min/max peak pyramid with an in-memory and
//!   on-disk (cache file) representation.
//! - [`waveform`]: the waveform data holder and its GPU renderer, built on the
//!   two modules above.
//!
//! See `DESIGN.md` for the scriptable widget protocol these views plug into.

pub mod peaks;
pub mod viewport;
pub mod waveform;
