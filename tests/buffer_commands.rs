//! S15: the three buffer commands S6 declared shipped and did not ship —
//! `/buffer_fill`, `/buffer_readChannel` and `/buffer_allocReadChannel`.
//!
//! The fill is the writing family's member and addresses samples **flat and
//! interleaved**, like `/buffer_set` beside it and unlike the editing verbs,
//! whose spans are frames; that difference is what the first test is for. The
//! channel reads are the ones that could not be done at all: `/buffer_read`
//! fails outright on a channel-count mismatch, so one channel of a stereo file
//! meant loading both and discarding one.
//!
//! Driven through an offline session, the cheapest driver that owns a whole
//! server; these are ordinary buffer commands and run on any server.

#![cfg(feature = "synth")]

use clausters::rosc::{OscMessage, OscType};
use clausters::server::nrtsession::{NrtSession, SessionConfig};

fn session(channels: usize) -> NrtSession {
    NrtSession::open(&SessionConfig {
        sample_rate: 48_000.0,
        channels,
        ..Default::default()
    })
    .expect("open")
}

fn send(s: &mut NrtSession, addr: &str, args: Vec<OscType>) {
    assert!(s.send_msg(addr, args).expect("encode"), "ring full");
    s.settle_for(4);
}

fn blob(values: &[f32]) -> OscType {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    OscType::Blob(bytes)
}

fn reply(s: &mut NrtSession, addr: &str) -> Option<OscMessage> {
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..64 {
        while let Some(len) = s.poll_into(&mut buf) {
            if let Ok(clausters::rosc::OscPacket::Message(m)) =
                clausters::osc::decode_packet(&buf[..len])
                && m.addr == addr
            {
                return Some(m);
            }
        }
        s.settle();
    }
    None
}

fn read_back(s: &mut NrtSession, bufnum: i32, count: usize) -> Vec<f32> {
    send(
        s,
        "/buffer_getRange",
        vec![
            OscType::Int(bufnum),
            OscType::Int(0),
            OscType::Int(count as i32),
        ],
    );
    let m = reply(s, "/buffer_getRange.reply").expect("reads back");
    let OscType::Blob(bytes) = m.args.last().expect("a blob") else {
        panic!("expected a blob, got {:?}", m.args)
    };
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn fails(s: &mut NrtSession) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    s.settle_for(4);
    while let Some(len) = s.poll_into(&mut buf) {
        if let Ok(clausters::rosc::OscPacket::Message(m)) =
            clausters::osc::decode_packet(&buf[..len])
            && m.addr == "/fail"
        {
            out.push(format!("{:?}", m.args));
        }
    }
    out
}

/// A stereo buffer whose two channels are told apart by sign: L positive,
/// R negative, both counting up.
fn stereo_ramp(frames: usize) -> Vec<f32> {
    (0..frames * 2)
        .map(|i| {
            let v = (i / 2 + 1) as f32;
            if i % 2 == 0 { v } else { -v }
        })
        .collect()
}

#[test]
fn fill_addresses_samples_flat_the_way_buffer_set_does() {
    let mut s = session(2);
    let frames = 4;
    let data = stereo_ramp(frames);
    send(
        &mut s,
        "/buffer_alloc",
        vec![
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Int(2),
        ],
    );
    send(
        &mut s,
        "/buffer_setRange",
        vec![OscType::Int(0), OscType::Int(0), blob(&data)],
    );
    // Two runs in one message, flat indices: samples 0..2 and 5..7.
    send(
        &mut s,
        "/buffer_fill",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(2),
            OscType::Float(9.0),
            OscType::Int(5),
            OscType::Int(2),
            OscType::Float(-9.0),
        ],
    );
    let mut expected = data;
    expected[0..2].fill(9.0);
    expected[5..7].fill(-9.0);
    assert_eq!(read_back(&mut s, 0, frames * 2), expected);
}

#[test]
fn a_fill_past_the_end_fails_and_writes_nothing() {
    let mut s = session(1);
    send(
        &mut s,
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(4), OscType::Int(1)],
    );
    send(
        &mut s,
        "/buffer_setRange",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            blob(&[1.0, 1.0, 1.0, 1.0]),
        ],
    );
    let _ = fails(&mut s);
    send(
        &mut s,
        "/buffer_fill",
        vec![
            OscType::Int(0),
            OscType::Int(2),
            OscType::Int(4),
            OscType::Float(0.0),
        ],
    );
    let reported = fails(&mut s);
    assert!(
        reported.iter().any(|f| f.contains("past the end")),
        "the caller is told, got {reported:?}"
    );
    assert_eq!(read_back(&mut s, 0, 4), vec![1.0; 4], "and nothing changed");
}

/// The pair that could not be done at all: one channel of a stereo file into a
/// mono buffer. Written to disk first, so the round trip is the real one.
#[test]
fn a_single_channel_of_a_stereo_file_loads_on_its_own() {
    let mut s = session(2);
    let frames = 8;
    let data = stereo_ramp(frames);
    let path = std::env::temp_dir().join(format!("clausters_s15_{}.wav", std::process::id()));
    let path_str = path.to_string_lossy().to_string();

    send(
        &mut s,
        "/buffer_alloc",
        vec![
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Int(2),
        ],
    );
    send(
        &mut s,
        "/buffer_setRange",
        vec![OscType::Int(0), OscType::Int(0), blob(&data)],
    );
    send(
        &mut s,
        "/buffer_write",
        vec![
            OscType::Int(0),
            OscType::String(path_str.clone()),
            OscType::String("wav".into()),
            OscType::String("float".into()),
        ],
    );
    assert!(path.exists(), "the file was written");

    // The right channel alone, into a buffer of its own.
    send(
        &mut s,
        "/buffer_allocReadChannel",
        vec![
            OscType::Int(1),
            OscType::String(path_str.clone()),
            OscType::Int(0), // fileStart
            OscType::Int(0), // numFrames: all
            OscType::Int(1), // the channel
        ],
    );
    let right: Vec<f32> = data.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(read_back(&mut s, 1, frames), right, "channel 1 alone");

    // And into an existing buffer, which is the other half of the pair.
    send(
        &mut s,
        "/buffer_alloc",
        vec![
            OscType::Int(2),
            OscType::Int(frames as i32),
            OscType::Int(1),
        ],
    );
    send(
        &mut s,
        "/buffer_readChannel",
        vec![
            OscType::Int(2),
            OscType::String(path_str.clone()),
            OscType::Int(0),  // fileStart
            OscType::Int(-1), // numFrames: all
            OscType::Int(0),  // bufStart
            OscType::Int(0),  // the left channel this time
        ],
    );
    let left: Vec<f32> = data.iter().step_by(2).copied().collect();
    assert_eq!(read_back(&mut s, 2, frames), left, "channel 0 alone");

    let _ = std::fs::remove_file(&path);
}

/// The order is honoured and repeats are allowed — which is what naming
/// channels explicitly is for, and costs nothing to permit.
#[test]
fn channels_may_be_reordered_and_repeated() {
    let mut s = session(2);
    let frames = 4;
    let data = stereo_ramp(frames);
    let path = std::env::temp_dir().join(format!("clausters_s15b_{}.wav", std::process::id()));
    let path_str = path.to_string_lossy().to_string();
    send(
        &mut s,
        "/buffer_alloc",
        vec![
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Int(2),
        ],
    );
    send(
        &mut s,
        "/buffer_setRange",
        vec![OscType::Int(0), OscType::Int(0), blob(&data)],
    );
    send(
        &mut s,
        "/buffer_write",
        vec![
            OscType::Int(0),
            OscType::String(path_str.clone()),
            OscType::String("wav".into()),
            OscType::String("float".into()),
        ],
    );
    // Swapped.
    send(
        &mut s,
        "/buffer_allocReadChannel",
        vec![
            OscType::Int(1),
            OscType::String(path_str.clone()),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    let swapped: Vec<f32> = data.chunks_exact(2).flat_map(|f| [f[1], f[0]]).collect();
    assert_eq!(read_back(&mut s, 1, frames * 2), swapped);

    // A channel the file does not have fails rather than reading silence.
    let _ = fails(&mut s);
    send(
        &mut s,
        "/buffer_allocReadChannel",
        vec![
            OscType::Int(3),
            OscType::String(path_str.clone()),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(7),
        ],
    );
    let reported = fails(&mut s);
    assert!(
        reported.iter().any(|f| f.contains("channel 7")),
        "asking for a channel that is not there is a mistake worth hearing about, got {reported:?}"
    );
    let _ = std::fs::remove_file(&path);
}
