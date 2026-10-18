//! Explicit application services, independent of a renderer or client.
//!
//! A composition owns its transport and receives its editing and resource
//! scopes. Services do not discover applications through a global registry.

pub mod histories;

pub mod clipboard;

pub mod resources;
