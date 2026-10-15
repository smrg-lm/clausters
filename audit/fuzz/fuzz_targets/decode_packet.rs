//! Fuzz the single OSC decoding door.
//!
//! Every transport (UDP datagram, TCP frame, WebSocket message, IPC ring
//! slot) hands its untrusted bytes to `clausters_core::osc::decode_packet`,
//! so this one target covers the whole inbound parse surface: arbitrary
//! bytes must decode to `Ok` or `Err`, never panic, hang or blow memory.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = clausters_core::osc::decode_packet(data);
});
