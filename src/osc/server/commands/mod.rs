//! The command handlers, one module per resource family -- the same families
//! the wire names (`/bus_*`, `/buffer_*`, `/def_*`, `/node_*`, `/server_*`,
//! `/transport_*`, `/midi_*`). Each is an `impl OscServer` block; the dispatch table that
//! reaches them is `super::dispatch`.

mod buffer;
mod bus;
mod def;
mod midi;
mod node;
mod server;
mod transport;
