//! OSC layer: UDP server, parsing with rosc and translation into engine
//! commands.

pub mod graph;
pub mod server;
pub mod translate;

use rosc::{OscBundle, OscPacket, OscTime, decoder};

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
