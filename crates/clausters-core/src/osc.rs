//! OSC timetag math and bundle assembly.
//!
//! The client side of timing: turn a wall-clock instant into an NTP timetag,
//! wrap messages in a timestamped bundle, and convert an instant to the
//! server's sample counter given an anchor (the `/clock` reply or the shm
//! data-plane sample clock). Unlike the rest of the crate this allocates and
//! depends on `rosc`, so it is for the network/client side, not the audio
//! thread.

pub use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType};

/// Seconds between the NTP epoch (1900-01-01) and the Unix epoch (1970-01-01).
const NTP_UNIX_OFFSET: f64 = 2_208_988_800.0;

/// The "now / as soon as possible" timetag (`{0, 1}`), which scsynth and
/// Clausters treat as immediate.
pub const IMMEDIATE: OscTime = OscTime {
    seconds: 0,
    fractional: 1,
};

/// A Unix timestamp (seconds since 1970, fractional allowed) → NTP [`OscTime`].
pub fn unix_to_ntp(unix_secs: f64) -> OscTime {
    let ntp = unix_secs + NTP_UNIX_OFFSET;
    let seconds = ntp.floor();
    let fractional = ((ntp - seconds) * 4_294_967_296.0).round(); // × 2^32
    OscTime {
        seconds: seconds as u32,
        fractional: fractional.min(4_294_967_295.0) as u32,
    }
}

/// NTP [`OscTime`] → Unix timestamp (seconds since 1970).
pub fn ntp_to_unix(t: OscTime) -> f64 {
    t.seconds as f64 + t.fractional as f64 / 4_294_967_296.0 - NTP_UNIX_OFFSET
}

/// The server's sample counter at Unix instant `unix_secs`, given an anchor
/// (`anchor_sample` was the counter at `anchor_unix`) and the sample rate.
/// This is the conversion a client uses to schedule by absolute sample with
/// `/sched`, removing wall-clock/crystal drift once the anchor is modelled.
pub fn unix_to_sample(
    unix_secs: f64,
    anchor_unix: f64,
    anchor_sample: i64,
    sample_rate: f64,
) -> i64 {
    anchor_sample + ((unix_secs - anchor_unix) * sample_rate).round() as i64
}

/// Wraps messages in a bundle stamped at `time`.
pub fn bundle(time: OscTime, messages: Vec<OscMessage>) -> OscBundle {
    OscBundle {
        timetag: time,
        content: messages.into_iter().map(OscPacket::Message).collect(),
    }
}

/// Encodes a packet to bytes (the single door, mirroring the server's use of
/// `rosc`).
pub fn encode(packet: &OscPacket) -> Result<Vec<u8>, rosc::OscError> {
    rosc::encoder::encode(packet)
}

/// Decodes one OSC packet through rosc — the single **decode** entry point every
/// transport funnels through, on the server and on every client alike (UDP
/// datagrams, the IPC ring, WebSocket frames, the GUI host's fronts), so
/// decoding and any future hardening live in one place. A thin wrapper over
/// `rosc::decoder::decode_udp`; the error is stringified so callers need not
/// depend on `rosc`'s error type.
pub fn decode_packet(bytes: &[u8]) -> Result<OscPacket, String> {
    let (_, packet) = rosc::decoder::decode_udp(bytes).map_err(|e| e.to_string())?;
    Ok(packet)
}

/// Convenience: build a message from an address and arguments.
pub fn message(addr: impl Into<String>, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntp_round_trip() {
        let unix = 1_700_000_000.5;
        let back = ntp_to_unix(unix_to_ntp(unix));
        assert!((unix - back).abs() < 1e-3, "{unix} vs {back}");
    }

    #[test]
    fn sample_anchor_conversion() {
        // 1 second past the anchor at 48 kHz is 48000 samples later.
        let s = unix_to_sample(101.0, 100.0, 1_000, 48_000.0);
        assert_eq!(s, 1_000 + 48_000);
    }

    #[test]
    fn bundle_encodes_and_decodes() {
        let msg = message("/n_set", vec![OscType::Int(1000), OscType::Float(440.0)]);
        let b = bundle(unix_to_ntp(1_700_000_000.0), vec![msg]);
        let bytes = encode(&OscPacket::Bundle(b)).unwrap();
        // Decodes back through the single shared door.
        match decode_packet(&bytes).unwrap() {
            OscPacket::Bundle(b) => assert_eq!(b.content.len(), 1),
            _ => panic!("expected a bundle"),
        }
    }
}
