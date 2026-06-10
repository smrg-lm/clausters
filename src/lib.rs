//! claudesufa: a real-time audio server in the style of scsynth.
//!
//! The engine (`server::engine`) knows nothing about cpal: it processes blocks
//! of [`server::engine::BLOCK_SIZE`] frames against in-memory slices, so tests
//! and the future NRT mode use it exactly like the real audio backend does.

pub mod dsp;
pub mod node;
pub mod osc;
pub mod server;
