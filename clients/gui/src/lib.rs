//! Reusable building blocks for navigable audio views (waveform, spectrogram).
//!
//! Core, windowing-agnostic (and web-portable) machinery:
//!
//! - [`viewport`]: zoom/pan navigation shared by every view.
//! - [`peaks`]: resolution-matched min/max peak pyramid (waveform analysis).
//! - [`spectrogram`]: STFT analysis and its GPU renderer.
//! - [`waveform`]: waveform data holder and its GPU renderer.
//! - [`view`]: the `TimelineView` trait both views implement.
//! - [`bytes`]: shared little-endian cache (de)serialization.
//!
//! Native-only helpers (excluded from wasm):
//!
//! - [`native`]: a winit + wgpu windowing harness that drives any `TimelineView`.
//! - [`demo`]: a synthetic test signal for the prototype binaries.
//!
//! See `DESIGN.md` for the scriptable widget protocol these views plug into.
//!
//! The widget protocol itself — the GUI host that speaks `/gui_*` over OSC and
//! drives these views — lives in [`host`] (native-only; it owns sockets).

mod bytes;

#[cfg(not(target_arch = "wasm32"))]
pub mod host;

pub mod peaks;
pub mod spectrogram;
pub mod view;
pub mod viewport;
pub mod waveform;

#[cfg(not(target_arch = "wasm32"))]
pub mod demo;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
