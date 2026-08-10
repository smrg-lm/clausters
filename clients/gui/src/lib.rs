//! Reusable building blocks for navigable audio views (waveform, spectrogram).
//!
//! Core, windowing-agnostic (and web-portable) machinery:
//!
//! - [`viewport`]: zoom/pan navigation shared by every view.
//! - [`peaks`]: resolution-matched min/max peak pyramid (waveform analysis),
//!   re-exported from [`clausters_core::peaks`] — the analysis lives once in the
//!   shared core so every client builds the identical mmap-able cache.
//! - [`spectrogram`]: STFT analysis (its forward FFT is `clausters_core::fft`)
//!   and its GPU renderer.
//! - [`waveform`]: waveform data holder and its GPU renderer.
//! - [`view`]: the `TimelineView` trait both views implement.
//!
//! Native-only helpers (excluded from wasm):
//!
//! - [`native`]: a winit + wgpu windowing harness that drives any `TimelineView`.
//! - [`demo`]: a synthetic test signal for the prototype binaries.
//!
//! The widget protocol itself — the GUI host that speaks `/gui_*` over OSC and
//! drives these views — lives in [`host`]. It is split along the platform seam:
//! a web-portable agnostic core (the widget tree, layout, protocol dispatch, the
//! `Transport`/`BusSource`/`BulkLoader`/`DefStore` traits) that compiles for
//! `wasm32`, and a native I/O shell behind those traits (UDP transport, on-disk
//! persistence, shared-memory bus source, mmap bulk loader, the winit/wgpu
//! driver) that is excluded from `wasm32`.

// This crate is the GUI host, read as *developer* documentation (much of it
// with `--document-private-items`): a module's docs routinely name the private
// function that does the work — `frame`'s `render`, `widget`'s `build`/`apply`,
// `gui`'s submodules. Those links are correct and useful in that mode, so the
// private-link lint is off here; `broken_intra_doc_links` (a link to something
// that does not exist at all) stays on and still fails the doc gate.
#![allow(rustdoc::private_intra_doc_links)]

pub mod host;

/// Adding an element **from a program that links this crate**: implement
/// [`Element`], register a constructor under the wire name it answers to, and
/// every def that names it — sent over OSC or built in Rust — gets it. The
/// leaves the host ships are still enum arms; a registration sits beside them
/// and is consulted only when no built-in name matched, so nothing an existing
/// def means can change under it. See [`host::widget::element`].
pub use host::widget::element::{self, Claim, Element, Needs, register, unregister};

/// The peak pyramid lives in the shared core (general client functionality);
/// re-exported here so the renderer (`waveform`) and the host keep using
/// `crate::peaks`.
pub use clausters_core::peaks;

pub mod spectrogram;
pub mod view;
pub mod viewport;
pub mod waveform;

// The wgpu device/surface bring-up, agnostic (it compiles to the WebGPU backend
// on wasm); shared by the native harness, the windowed host front and the
// browser entry point.
pub(crate) mod gpu;

#[cfg(not(target_arch = "wasm32"))]
pub mod demo;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
