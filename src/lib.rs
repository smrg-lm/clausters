//! Clausters: a real-time audio synthesis server in the style of scsynth,
//! controlled over OSC. This crate is both the server binary and a library you
//! can drive from your own code.
//!
//! The engine ([`server::engine`]) knows nothing about cpal: it processes
//! blocks of [`server::engine::BLOCK_SIZE`] frames against in-memory slices, so
//! the cpal callback, the offline (NRT) renderer and the tests all use it the
//! same way. The audio side never allocates, locks or does I/O — commands
//! arrive pre-built over lock-free FIFOs and freed memory leaves the same way.
//!
//! # Entry points
//!
//! - [`server::render::render_to_wav`] / [`server::render::render_to_vec`] —
//!   render a [`server::render::Score`] offline; the simplest way in.
//! - [`server::engine::engine_pair`] — the [`server::engine::Engine`] (audio
//!   side: [`process_block`](server::engine::Engine::process_block)) and the
//!   [`server::engine::EngineHandle`] (control side:
//!   [`send`](server::engine::EngineHandle::send) a [`server::engine::Cmd`]).
//! - [`osc::translate::CmdTranslator`] — turn OSC messages into engine
//!   commands, exactly as the server does.
//! - [`osc::server::OscServer`] — the full UDP server loop.
//!
//! # Feature flags
//!
//! - `synth` (default) — the SynthDef family: the UGen library, the def
//!   compiler (`/d_recv`) and the `synthdef` module.
//! - `faust` — the FaustDef family: libfaust embedding (Box API + LLVM JIT);
//!   adds the `faust` module.
//!
//!   The two def families are independent and combinable: enable both, or
//!   build a single-family server (`--no-default-features --features
//!   faust,realtime,midi` is a Faust-only build). With neither, the engine
//!   core (groups, buses, buffers, transports) still builds and runs, but
//!   every `/s_new` fails for lack of defs.
//! - `realtime` (default) — the cpal backend (the live server). Disable it for
//!   offline or embedded use with no audio device.
//! - `embed` — the C ABI for embedding the server in-process; adds the `embed`
//!   module.
//!
//! The prose documentation (user guide, OSC reference, architecture) is the
//! mdBook in `docs/`; build it with `mdbook build`.

pub mod dsp;
#[cfg(feature = "embed")]
pub mod embed;
#[cfg(feature = "faust")]
pub mod faust;
pub mod logging;
pub mod midi;
pub mod node;
pub mod osc;
pub mod server;
#[cfg(feature = "synth")]
pub mod synthdef;

// Re-exported so integration tests and client code can build OSC packets
// with the exact same version the server uses.
pub use rosc;
