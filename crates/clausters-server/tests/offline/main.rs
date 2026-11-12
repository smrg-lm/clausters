//! The server's offline tests, in one binary.
//!
//! Each module here was a test binary of its own, and each binary linked the
//! whole server: some forty links for tests that share nothing but the crate
//! they test. They render offline and read the result, with no socket, no
//! child process, no wall clock and no process-wide state, so they can share a
//! process, and one link serves them all.
//!
//! A test that does depend on its process stays a binary of its own beside
//! this directory: the ones that install a global allocator to count
//! allocations, the signal guard, the denormal mode, the shared-memory
//! segment, and every test that boots a live server on a socket or runs a
//! plugin's process.
//!
//! Run one module's tests with its name as the filter:
//! `cargo test --test offline envgen::`.

// The helpers the modules below share, declared once: a file loaded as a
// module by each test that uses it would be compiled once per test.
#[cfg(feature = "synth")]
#[path = "../common/bench.rs"]
mod bench;
#[path = "../common/signal.rs"]
mod signal;
#[cfg(feature = "synth")]
#[path = "../common/sine.rs"]
mod sine;

mod audio_io;
mod buffer_commands;
mod capacity;
mod controls;
mod conv;
mod core_parity;
mod delays;
mod demand;
mod engine;
mod envgen;
mod feedback;
mod filters;
mod golden;
mod graphdef;
mod group_nset;
mod mapping;
mod meters;
mod midi;
mod mixer_graph;
mod mpe;
mod noise;
mod nrt_session;
mod ops;
mod oscillators;
mod pan;
mod pv_reference;
mod rates;
mod registry;
mod schema;
mod signal_asserts;
mod soundfile;
mod spectral;
mod subjects;
mod synthdef;
mod transport;
mod triggers;
mod voice_reset;
mod wavetable;
