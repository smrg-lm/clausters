//! Clausters shared native core.
//!
//! The single source of truth for the value- and time-level computations that
//! must agree between the server (`clausters`) and every client (Python now,
//! JavaScript later). The builtins/rng/tempoclock hot paths (and [`fft`]) are
//! allocation-free so the audio thread can call them directly; the server
//! refactors its native UGens onto them, which makes client-side results match
//! the server **by construction** for the operations the server computes
//! natively. See `clients/PLAN.md` (milestone C0).
//!
//! Boundary rule (project-wide): only flat data crosses any binding —
//! `f32`/`f64`/integers and slices of them, never a library type. The C ABI
//! over this crate lives in the sibling `clausters-ffi` crate.
//!
//! # Modules
//!
//! - [`builtins`] — unary/binary numeric operators on scalars and slices. The
//!   four arithmetic ops mirror the server's `BinaryOp` exactly; the rest
//!   mirror Faust's Signal API (`crate::faust::signals` in the server) using
//!   the same formula. Bit-exactness is guaranteed only for the ops the server
//!   computes natively, not against Faust's own LLVM codegen.
//! - [`rng`] — the seeded white-noise generator, identical to the server's
//!   `dsp::noise`, so a client can reproduce a noise stream sample for sample.
//! - [`tempoclock`] — beat/second/sample arithmetic and a beat-ordered event
//!   queue: the timing math a `TempoClock` is built on.
//! - [`osc`] — the OSC seam shared by the server and every client: the single
//!   `decode_packet` door, bundle/timetag assembly and timetag↔sample
//!   conversion (depends on `rosc`; not allocation-free).
//! - [`config`] — the shared TOML configuration model (user + project layers,
//!   the same schema the server and every client read), with the native path
//!   resolution gated off `wasm32`.
//! - [`fft`] — forward **and** inverse real FFT (over `microfft`,
//!   zero-allocation), shared by the GUI spectrogram and the server's
//!   `FFT`/`IFFT` UGens so the transform lives once.
//! - [`window`] — the smoothing windows (Hann, Welch, …) the FFT chain applies,
//!   shared with the clients for bit-identical analysis.
//! - [`peaks`] — the min/max peak pyramid behind any client's navigable
//!   waveform view, with its memory-mappable cache. General client
//!   functionality (not real-time), shared so every client builds the identical
//!   cache.
//! - [`bytes`] — little-endian cache (de)serialization the analysis caches
//!   share.

pub mod builtins;
pub mod bytes;
pub mod config;
pub mod fft;
pub mod osc;
pub mod peaks;
pub mod rng;
pub mod tempoclock;
pub mod window;
