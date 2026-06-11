//! The server: processing engine, audio backend and NRT thread.

#[cfg(feature = "realtime")]
pub mod backend;
pub mod engine;
pub mod nrt;
