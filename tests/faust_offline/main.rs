//! The Faust family's offline tests, in one binary.
//!
//! The same grouping as `tests/offline`, for the tests that compile a FaustDef
//! and render it offline. The Faust tests that boot a live server
//! (`faust_compiler`, `faust_synth`, `faust_smoke`) stay binaries of their own.
//!
//! Run one module's tests with its name as the filter:
//! `cargo test --test faust_offline faust_box::`.

// The helper the modules below share, declared once.
#[path = "../common/signal.rs"]
mod signal;

mod faust_box;
mod faust_json;
mod faust_parity;
mod faust_signal;
