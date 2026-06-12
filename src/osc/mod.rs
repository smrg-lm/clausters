//! OSC layer: UDP server, parsing with rosc and translation into engine
//! commands.

pub mod graph;
pub mod server;
pub mod translate;

use std::net::SocketAddr;

use rosc::{OscBundle, OscPacket, OscTime, decoder};

/// Where a request came from and where its replies go (M14): the OSC
/// *encoding* is transport-independent, so client identity is too. `Udp` is
/// a remote socket; `Ring` is the single shared-memory / in-process ring
/// client of `server::ipc`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClientId {
    Udp(SocketAddr),
    Ring,
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientId::Udp(addr) => write!(f, "{addr}"),
            ClientId::Ring => write!(f, "ring client"),
        }
    }
}

/// Decodes one OSC packet, working around the rosc 0.10 blob bug (see
/// CLAUDE.md): the decoder over-reads the padding of blobs whose length is a
/// multiple of 4 and fails on valid packets. For a top-level message, four
/// appended zero bytes fix it (harmless: they stay as unparsed remainder) —
/// but a bundle *element* is parsed from its own size-prefixed slice that
/// outer padding cannot reach, and rosc silently returns the bundle with the
/// content decoded so far. So bundles are split into elements here,
/// recursively, and only leaf messages go through rosc.
pub fn decode_packet(bytes: &[u8]) -> Result<OscPacket, String> {
    const BUNDLE_TAG: &[u8] = b"#bundle\0";
    if bytes.starts_with(BUNDLE_TAG) {
        if bytes.len() < 16 {
            return Err("truncated bundle header".into());
        }
        let timetag = OscTime {
            seconds: u32::from_be_bytes(bytes[8..12].try_into().unwrap()),
            fractional: u32::from_be_bytes(bytes[12..16].try_into().unwrap()),
        };
        let mut content = Vec::new();
        let mut pos = 16;
        while pos < bytes.len() {
            if bytes.len() - pos < 4 {
                return Err("truncated bundle element size".into());
            }
            let size = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if size == 0 || pos + size > bytes.len() {
                return Err(format!("bad bundle element size {size}"));
            }
            content.push(decode_packet(&bytes[pos..pos + size])?);
            pos += size;
        }
        Ok(OscPacket::Bundle(OscBundle { timetag, content }))
    } else {
        let mut padded = Vec::with_capacity(bytes.len() + 4);
        padded.extend_from_slice(bytes);
        padded.extend_from_slice(&[0; 4]);
        let (_, packet) = decoder::decode_udp(&padded).map_err(|e| e.to_string())?;
        Ok(packet)
    }
}
