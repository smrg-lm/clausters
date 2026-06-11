//! The server: processing engine, audio backend, NRT thread and offline
//! renderer.

#[cfg(feature = "realtime")]
pub mod backend;
pub mod engine;
pub mod nrt;
pub mod render;
