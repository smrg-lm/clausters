//! **The host's native I/O shell**: everything the agnostic core reaches
//! through a trait, filled with what a desktop process has.
//!
//! The core never names a socket, a file or a segment: it names the seams in
//! the host's root ([`Transport`](super::Transport), the GuiDef store, the
//! bulk loader, the font source, the bus source) and a front hands it an
//! implementation. These are the native ones, grouped so that what a browser
//! build leaves out is one module; the page fills the same seams over
//! WebSocket and fetch (`host::web`), and the windowed front that drives them
//! natively is `host::gui`.

// The client leg, on-disk GuiDef persistence and the UDP/TCP/WebSocket server
// fronts.
pub mod client;
pub mod store;
pub mod transport;

// Reading the audio server's shared-memory segment for zero-message meters and
// scopes: the native [`BusSource`]. Unix-only, as the server's segment is.
#[cfg(unix)]
pub mod shm;

// The samples behind that segment's directory: a take's samples, mapped
// read/write, so drawing one costs no conversation and editing one costs no
// message. Unix-only, like `shm`.
#[cfg(unix)]
pub mod mapped;

// The in-process embedded server for the standalone mode, a direct dependency on
// the `clausters` crate behind the optional `standalone` feature (off by default,
// since it pulls the engine + audio backend). Native-only.
#[cfg(feature = "standalone")]
pub mod embed;

// Mapping a local file (raw samples or a prebuilt peak cache) for the bulk-data
// path: a multi-megabyte buffer read from a shared resource, not over OSC.
// Unix-only, like `shm`.
#[cfg(unix)]
pub mod mapfile;

// The native [`BulkLoader`]: resolves a waveform/plot's local `path`/`cache` to
// samples or a peak pyramid through the mmap path above. The browser resolves
// the same references as URLs through `fetch`.
pub mod bulk;

// The native [`FontSource`] (the `font-atlas` feature): the typeface a build
// draws with, read from a file the command line names or from the system's own
// faces. The browser fetches one instead and pushes the bytes through the same
// seam.
#[cfg(feature = "font-atlas")]
pub mod fontfile;
