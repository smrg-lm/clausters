//! Explicit application services, independent of a renderer or client.
//!
//! A composition owns its transport and receives its editing and resource
//! scopes. Services do not discover applications through a global registry.

pub mod histories;

pub mod clipboard;

pub mod resources;

/// Typed preparation and completion of pending work.
pub mod operations;

/// Explicit application composition and its typed lifecycle.
pub mod services;

/// Reusable operation and recovery orchestration.
pub mod runtime;

/// Multi-object history confirmation over typed application participants.
pub mod walks;
