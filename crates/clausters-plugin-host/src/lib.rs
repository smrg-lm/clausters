//! Native plugin hosting outside the server process.
//!
//! Discovery and lifecycle use a bounded administrative channel. Audio uses
//! `clausters_core::plugin_shm`, with one helper and producer per instance.
//! This crate adds no synth family: the server's node integration is separate.

#[cfg(all(target_os = "linux", feature = "clap"))]
mod clap;
pub mod format;
#[cfg(target_os = "linux")]
mod mapping;
#[cfg(target_os = "linux")]
mod protocol;
#[cfg(target_os = "linux")]
pub mod runtime;
