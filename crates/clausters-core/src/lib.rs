//! Clausters shared native core.
//!
//! The single source of truth for the value- and time-level computations that
//! must agree between the server (`clausters`) and every client (Python now,
//! JavaScript later). The builtins/rng/tempoclock hot paths (and [`fft`]) are
//! allocation-free so the audio thread can call them directly; the server
//! refactors its native UGens onto them, which makes client-side results match
//! the server **by construction** for the operations the server computes
//! natively. See `clients/python/PLAN.md`.
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
//! - [`tempoclock`] — beat/second/sample arithmetic, quantization and a
//!   beat-ordered event queue: the timing math a `TempoClock` is built on.
//! - [`clocksync`] — the least-squares sample-clock tracking model
//!   (`sample = a + b·t` over a sliding anchor window) behind locking a client
//!   clock to a server over a network transport.
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
//! - [`scale`] — perceptual frequency-scale conversions (hertz ↔ mel/bark)
//!   shared by every frequency-axis display and analysis.
//! - [`envshape`] — the envelope segment shapes (the SuperCollider shape
//!   curves), shared by the server's `EnvGen` and any client drawing or
//!   editing envelopes, so what an editor draws is what the server plays.
//! - [`registry`] — the finite-resource id registry (node ids, buses,
//!   buffers): a bounded occupancy map where every release is reusable and
//!   exhaustion is explicit, shared so the server's reserved ranges and every
//!   client's allocators enforce the same invariants.
//! - [`measure`] — the stereo-field measurements (correlation, the Lissajous /
//!   goniometer projection) every meter and phasescope reads.
//! - [`oscil`] — the triggered oscilloscope's window sizing and trigger
//!   alignment, shared by the GUI host and by a client drawing its own trace.
//! - [`spectrum`] — the per-frame magnitude curve in decibels (window, FFT,
//!   coherent-gain normalization) every spectrum display reads.
//! - [`peaks`] — the min/max peak pyramid behind any client's navigable
//!   waveform view, with its memory-mappable cache. General client
//!   functionality (not real-time), shared so every client builds the identical
//!   cache.
//! - [`bytes`] — little-endian cache (de)serialization the analysis caches
//!   share.
//! - [`bundle`] — the component bundle's manifest and its resolver: the
//!   placeholder pass that turns one persisted GuiDef template into N
//!   non-colliding mounted instances, shared so a browser tab, a
//!   `clausters-gui --standalone` and a loopback host read one format.
//! - [`patch`] — the GUI patcher's cord → bus pass: a directed patch (typed
//!   inlets/outlets, cords) compiled to a GraphDef's bus wiring (one bus per
//!   connected net, its writers summing), shared so every client that draws a
//!   patch translates it identically.
//! - `notation` (feature `notation`, off by default) — the pure half of the
//!   notation layer: the verovio-SVG -> display-list walk and the voice -> MEI
//!   encoder, the format-agnostic parts every client shares (the native
//!   libverovio binding is the separate `clausters-notation` crate). Behind a
//!   feature so a default core carries no XML/regex weight; compiles to wasm.

pub mod builtins;
pub mod bundle;
pub mod bytes;
pub mod clocksync;
pub mod config;
pub mod envshape;
pub mod fft;
pub mod measure;
#[cfg(feature = "notation")]
pub mod notation;
pub mod osc;
pub mod oscil;
pub mod patch;
pub mod peaks;
pub mod pvprog;
pub mod registry;
pub mod rng;
pub mod scale;
pub mod spectrum;
pub mod tempoclock;
pub mod window;
