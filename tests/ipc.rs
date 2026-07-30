//! M14: the IPC segment — ring transport, data plane, versioning, and the
//! embedded C ABI render (with `--features embed`).

#![cfg(feature = "synth")]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::{BLOCK_SIZE, engine_pair_full};
use clausters::server::ipc::{IpcPeer, Role, SEGMENT_SIZE, Segment};

const SR: f32 = 48_000.0;

fn encode(addr: &str, args: Vec<OscType>) -> Vec<u8> {
    encoder::encode(&OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    }))
    .unwrap()
}

#[test]
fn ring_roundtrip_and_wraparound() {
    let segment = Segment::in_memory();
    let client = IpcPeer::new(Arc::clone(&segment), Role::Client);
    let server = IpcPeer::new(Arc::clone(&segment), Role::Server);

    // Far more traffic than one ring capacity: the cursors must wrap.
    let mut buf = vec![0u8; 65536];
    for round in 0..200 {
        for i in 0..32 {
            let msg = encode("/n_set", vec![OscType::Int(round * 32 + i)]);
            assert!(client.push(&msg), "push must succeed while drained");
        }
        for i in 0..32 {
            let len = server.try_pop(&mut buf).expect("packet must be there");
            let expected = encode("/n_set", vec![OscType::Int(round * 32 + i)]);
            assert_eq!(&buf[..len], &expected[..], "FIFO order preserved");
        }
        assert!(server.try_pop(&mut buf).is_none(), "ring drained");
    }

    // Backpressure: an unbounded burst eventually reports full, loses nothing.
    let msg = encode("/status", vec![]);
    let mut pushed = 0;
    while client.push(&msg) {
        pushed += 1;
        assert!(pushed < 100_000, "a full ring must reject pushes");
    }
    for _ in 0..pushed {
        assert!(server.try_pop(&mut buf).is_some());
    }
    assert!(server.try_pop(&mut buf).is_none());
}

#[test]
fn corrupted_ring_contents_resync_instead_of_wedging() {
    let segment = Segment::in_memory();
    let client = IpcPeer::new(Arc::clone(&segment), Role::Client);
    let server = IpcPeer::new(Arc::clone(&segment), Role::Server);

    // A "packet" the consumer cannot fit in its buffer counts as garbage.
    let huge = vec![0x2f_u8; 9000];
    assert!(client.push(&huge));
    let mut small = vec![0u8; 256];
    assert!(server.try_pop(&mut small).is_none(), "oversized = dropped");
    // The ring keeps working afterwards.
    assert!(client.push(&encode("/status", vec![])));
    assert!(server.try_pop(&mut small).is_some());
}

#[cfg(unix)]
#[test]
fn file_segments_validate_magic_and_version() {
    let path = std::env::temp_dir().join(format!("clausters-ipc-test-{}", std::process::id()));
    let segment = Segment::create(&path).unwrap();
    segment.set_sample_rate(48_000.0);

    let attached = Segment::open(&path).expect("freshly created segment must open");
    assert_eq!(attached.sample_rate(), 48_000.0);
    // The two mappings see the same memory.
    segment.clock().store(12345, Ordering::Release);
    assert_eq!(attached.clock().load(Ordering::Acquire), 12345);
    drop(attached);

    // Corrupt the version field (offset 4): attach must refuse.
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(4)).unwrap();
    file.write_all(&999u32.to_le_bytes()).unwrap();
    drop(file);
    let err = match Segment::open(&path) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a version mismatch must refuse to attach"),
    };
    assert!(err.contains("ABI version"), "{err}");

    // Wrong size: refuse too.
    std::fs::write(&path, b"short").unwrap();
    let err = match Segment::open(&path) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a size mismatch must refuse to attach"),
    };
    assert!(err.contains("size"), "{err}");
    let _ = std::fs::remove_file(&path);
    // Pins the layout for out-of-process clients (clients/python parses
    // these offsets): changing the *structure* requires bumping ABI_VERSION.
    // This is the default-count instance of the v4 layout: header + rings +
    // 16384 default control buses, then the audio-bus region (128 buses × two
    // words: the bus -> tap directory and the block levels), aligned to 64,
    // plus 8 default taps × (64-byte cursor line + 16384 × f32 ring). The
    // counts travel in the header, so a non-default boot changes the size,
    // never the offsets' derivation.
    assert_eq!(SEGMENT_SIZE, 722_624);
}

/// The audio-bus region (ABI v4): the bus is the key. A reader names the audio
/// bus it wants and finds both where its samples land (the directory) and its
/// block level (the meter's number), so no ring index reaches an API.
#[test]
fn the_bus_region_maps_buses_to_taps_and_holds_their_levels() {
    let segment = Segment::in_memory_full(8, 2, 256);
    let buses = segment.audio_buses();
    assert!(buses > 0);

    // Nothing is recorded and everything is silent until something says so.
    assert_eq!(segment.tap_of_bus(0), None);
    assert_eq!(segment.level(0), 0.0);

    segment.set_tap_of_bus(5, Some(1));
    assert_eq!(segment.tap_of_bus(5), Some(1));
    assert_eq!(segment.tap_of_bus(4), None, "only the named bus moves");
    segment.set_tap_of_bus(5, None);
    assert_eq!(segment.tap_of_bus(5), None);

    // The levels are a second array over the same key, independent of it: a
    // metered bus needs no tap at all.
    segment.set_level(5, 0.25);
    segment.set_level(6, 1.0);
    assert_eq!(segment.level(5), 0.25);
    assert_eq!(segment.level(6), 1.0);
    assert_eq!(segment.tap_of_bus(5), None);

    // Out of range reads as absent/silent and writes are dropped, never UB.
    assert_eq!(segment.tap_of_bus(buses), None);
    assert_eq!(segment.level(buses), 0.0);
    segment.set_tap_of_bus(buses, Some(0));
    segment.set_level(buses, 1.0);
}

/// The audio-tap rings (ABI v3): block writes, the newest-window read, the
/// wrap, and every refusal (`None`) case of `tap_read_latest`.
#[test]
fn tap_rings_write_read_and_wrap() {
    // A tiny ring (256 samples = 4 blocks) so the wrap is exercised fast.
    let segment = Segment::in_memory_full(8, 2, 256);
    assert_eq!(segment.taps(), 2);
    assert_eq!(segment.tap_frames(), 256);

    // Before any write: no full window exists.
    let mut out = vec![0.0f32; BLOCK_SIZE];
    assert_eq!(segment.tap_read_latest(0, &mut out), None);

    // Write 5 ramp blocks (320 samples > the 256 ring: it wraps).
    let mut block = [0.0f32; BLOCK_SIZE];
    for b in 0..5u32 {
        for (i, s) in block.iter_mut().enumerate() {
            *s = (b as usize * BLOCK_SIZE + i) as f32;
        }
        segment.tap_write(0, &block);
    }

    // The newest 128 samples are 192..320, straddling the wrap point.
    let mut out = vec![0.0f32; 128];
    let end = segment.tap_read_latest(0, &mut out).expect("window ready");
    assert_eq!(end, 320);
    for (i, s) in out.iter().enumerate() {
        assert_eq!(*s, (192 + i) as f32, "sample {i}");
    }

    // Refusals: window over half the ring, empty window, bad tap index, and
    // a tap that never wrote.
    let mut too_big = vec![0.0f32; 129];
    assert_eq!(segment.tap_read_latest(0, &mut too_big), None);
    assert_eq!(segment.tap_read_latest(0, &mut []), None);
    assert_eq!(segment.tap_read_latest(2, &mut out), None);
    assert_eq!(segment.tap_read_latest(1, &mut out), None);
}

/// The full server speaking through the ring only: no UDP client at all.
#[test]
fn server_speaks_osc_over_the_ring() {
    let segment = Segment::in_memory();
    let (mut engine, handle) = engine_pair_full(
        SR,
        2,
        0,
        Some(Arc::clone(&segment)),
        128,
        1024,
        clausters::dsp::Limits::default(),
    );
    let info = ServerInfo {
        nominal_sample_rate: SR as f64,
        actual_sample_rate: SR as f64,
    };
    let mut server = OscServer::bind(("127.0.0.1", 0), info, handle).unwrap();
    server
        .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
        .unwrap();
    let thread = std::thread::spawn(move || server.run());
    let client = IpcPeer::new(Arc::clone(&segment), Role::Client);

    let mut buf = vec![0u8; 65536];
    let mut request = |packet: &[u8]| -> OscMessage {
        assert!(client.push(packet));
        for _ in 0..500 {
            if let Some(len) = client.try_pop(&mut buf) {
                let (_, packet) =
                    clausters::rosc::decoder::decode_udp(&buf[..len]).expect("valid reply");
                let OscPacket::Message(msg) = packet else {
                    panic!("expected a message reply");
                };
                return msg;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("no reply through the ring");
    };

    let status = request(&encode("/status", vec![]));
    assert_eq!(status.addr, "/status.reply");

    // A synth via the ring must reach the engine and make sound.
    assert!(client.push(&encode(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    )));
    std::thread::sleep(Duration::from_millis(50)); // let the server forward it
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut heard = false;
    for _ in 0..50 {
        engine.process_block(&mut out);
        heard |= out.iter().any(|s| *s != 0.0);
    }
    assert!(heard, "the ring /s_new must be audible");

    // The clock mirror in the segment is block-accurate.
    let clock = segment.clock().load(Ordering::Acquire);
    assert!(clock >= 50 * BLOCK_SIZE as u64, "clock = {clock}");

    let bad = request(&encode("/zzz", vec![]));
    assert_eq!(bad.addr, "/fail", "errors route back through the ring");

    assert!(client.push(&encode("/quit", vec![])));
    thread.join().unwrap().unwrap();
}

/// The data plane: a control-bus write in the segment is read by `InCtl`
/// on the next block — no OSC command involved.
#[test]
fn segment_control_buses_feed_the_engine_directly() {
    use clausters::node::{AddAction, ROOT_NODE_ID};
    use clausters::server::engine::Cmd;
    use clausters::synthdef::instance::UGenSynth;
    use clausters::synthdef::{SynthDefSpec, compile};

    let segment = Segment::in_memory();
    let (mut engine, mut handle) = engine_pair_full(
        SR,
        2,
        0,
        Some(Arc::clone(&segment)),
        128,
        1024,
        clausters::dsp::Limits::default(),
    );

    let spec: SynthDefSpec = serde_json::from_value(serde_json::json!({
        "name": "ctlreader",
        "ugens": [
            {"kind": "InCtl", "inputs": [{"const": 7.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }))
    .unwrap();
    let synth = Box::new(UGenSynth::new(
        Arc::new(compile(spec).unwrap()),
        SR,
        SEED_STRIDE,
    ));
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth,
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    engine.process_block(&mut out); // plug in; bus 7 still 0.0
    assert!(out.iter().all(|s| *s == 0.0));

    // What an external process would do: write the mapped atomic.
    segment.control_buses().set(7, 0.625);
    engine.process_block(&mut out);
    assert_eq!(out[0], 0.625, "InCtl must read the segment atomic");

    // And the handle's /c_get path sees external writes too (same memory).
    assert_eq!(handle.control_buses().get(7), 0.625);
}

/// The sync scientific call of the embed C ABI, exercised as a plain Rust
/// function (`cargo test --features embed --test ipc`).
#[cfg(feature = "embed")]
#[test]
fn embed_render_returns_flat_samples() {
    use clausters::embed::{clausters_free_samples, clausters_render};

    // A minimal score: /s_new default at t = 0, /n_free at t = 0.1.
    let s_new = encode(
        "/s_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    let n_free = encode("/n_free", vec![OscType::Int(1000)]);
    let bundle = |secs: f64, inner: &[u8]| -> Vec<u8> {
        let mut b = b"#bundle\0".to_vec();
        b.extend_from_slice(&(secs as u32).to_be_bytes());
        b.extend_from_slice(&(((secs.fract()) * 2f64.powi(32)) as u32).to_be_bytes());
        b.extend_from_slice(&(inner.len() as u32).to_be_bytes());
        b.extend_from_slice(inner);
        b
    };
    let mut score = Vec::new();
    for packet in [bundle(0.0, &s_new), bundle(0.1, &n_free)] {
        score.extend_from_slice(&(packet.len() as u32).to_be_bytes());
        score.extend_from_slice(&packet);
    }

    let mut frames = 0u64;
    let mut events = 0u64;
    let mut err = vec![0u8; 256];
    let ptr = unsafe {
        clausters_render(
            score.as_ptr(),
            score.len(),
            48_000.0,
            1,
            0,
            SEED_STRIDE,
            &mut frames,
            &mut events,
            err.as_mut_ptr(),
            err.len(),
        )
    };
    assert!(
        !ptr.is_null(),
        "render failed: {}",
        String::from_utf8_lossy(&err)
    );
    assert_eq!(frames, 4800, "0.1 s at 48 kHz");
    assert!(events > 0, "the score's events are reported back");
    let samples = unsafe { std::slice::from_raw_parts(ptr, frames as usize) };
    assert!(samples.iter().any(|s| *s != 0.0), "the default def sounds");
    unsafe { clausters_free_samples(ptr, frames) };

    // Error path: garbage score → NULL + message.
    let ptr = unsafe {
        clausters_render(
            b"garbage".as_ptr(),
            7,
            48_000.0,
            1,
            0,
            SEED_STRIDE,
            &mut frames,
            &mut events,
            err.as_mut_ptr(),
            err.len(),
        )
    };
    assert!(ptr.is_null());
    assert_ne!(err[0], 0, "error message must be written");
}
