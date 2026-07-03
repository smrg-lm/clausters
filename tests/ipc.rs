//! M14: the IPC segment — ring transport, data plane, versioning, and the
//! embedded C ABI render (with `--features embed`).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
    // these offsets): changing it requires bumping ABI_VERSION.
    assert_eq!(SEGMENT_SIZE, 135_360);
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
    let synth = Box::new(UGenSynth::new(Arc::new(compile(spec).unwrap())));
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
    let mut err = vec![0u8; 256];
    let ptr = unsafe {
        clausters_render(
            score.as_ptr(),
            score.len(),
            48_000.0,
            1,
            0,
            &mut frames,
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
            &mut frames,
            err.as_mut_ptr(),
            err.len(),
        )
    };
    assert!(ptr.is_null());
    assert_ne!(err[0], 0, "error message must be written");
}
