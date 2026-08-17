//! M14: the IPC segment — ring transport, data plane, versioning, and the
//! embedded C ABI render (with `--features embed`).

#![cfg(feature = "synth")]

use std::sync::Arc;

/// The peer tag these tests send under. One client, so any tag does — what
/// matters is that it comes back on the replies (ABI v7).
const PEER: u32 = 3;
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
            let msg = encode("/node_set", vec![OscType::Int(round * 32 + i)]);
            assert!(client.push(PEER, &msg), "push must succeed while drained");
        }
        for i in 0..32 {
            let (peer, len) = server.try_pop(&mut buf).expect("packet must be there");
            assert_eq!(peer, PEER, "the frame's tag says who authored it");
            let expected = encode("/node_set", vec![OscType::Int(round * 32 + i)]);
            assert_eq!(&buf[..len], &expected[..], "FIFO order preserved");
        }
        assert!(server.try_pop(&mut buf).is_none(), "ring drained");
    }

    // Backpressure: an unbounded burst eventually reports full, loses nothing.
    let msg = encode("/server_status", vec![]);
    let mut pushed = 0;
    while client.push(PEER, &msg) {
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
    assert!(client.push(PEER, &huge));
    let mut small = vec![0u8; 256];
    assert!(server.try_pop(&mut small).is_none(), "oversized = dropped");
    // The ring keeps working afterwards.
    assert!(client.push(PEER, &encode("/server_status", vec![])));
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
    // This is the default-count instance of the layout: header + rings + 16384
    // default control buses, then the audio-bus region (128 buses × two words:
    // the bus -> tap directory and the block levels), aligned to 64, plus 8
    // default taps × (64-byte cursor line + 16384 × f32 ring). The counts
    // travel in the header, so a non-default boot changes the size, never the
    // offsets' derivation. v6 added the transport clock inside the header's
    // reserved space, so the size and every offset are unchanged from v5 --
    // which is what reserved space is for. v9 appends the **buffer directory**
    // as the tail -- 4096 default rows of 24 bytes -- which is why the size
    // moved and no offset did: the header had no reserved space left for a row
    // count, so the count is what remains of the mapped length.
    assert_eq!(SEGMENT_SIZE, 722_624 + 4096 * 24);
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

/// M31(b): two independent clients over **one** segment.
///
/// The regression this exists for: every ring packet used to arrive as a single
/// `ClientId::Ring`, so `/bus_stream` — "one subscription per client, replaced
/// on each call" — could not tell two peers apart. A script and a GUI host
/// sharing one page took the stream from each other, and the loss was permanent
/// in one direction, since the host only re-subscribes when its own widget set
/// changes. With the frame's peer tag they are two clients, exactly as a native
/// host and a script on two sockets are.
#[test]
fn two_ring_peers_keep_their_own_subscriptions() {
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
    let mut server = OscServer::headless(info, handle, 0.0);
    server
        .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
        .unwrap();
    let client = IpcPeer::new(Arc::clone(&segment), Role::Client);

    const SCRIPT: u32 = 1;
    const HOST: u32 = 2;

    // Distinct control-bus values, so a snapshot says which subscription it is.
    segment.control_buses().set(0, 0.25);
    segment.control_buses().set(1, 0.75);

    // The script subscribes to bus 0, then the host to bus 1 -- the order that
    // used to leave the script silent.
    assert!(client.push(
        SCRIPT,
        &encode("/bus_stream", vec![OscType::Int(20), OscType::Int(0)]),
    ));
    assert!(client.push(
        HOST,
        &encode("/bus_stream", vec![OscType::Int(20), OscType::Int(1)]),
    ));

    // Drive the server and collect the snapshots each peer is sent.
    let mut buf = vec![0u8; 65536];
    let mut to_script = Vec::new();
    let mut to_host = Vec::new();
    for _ in 0..200 {
        server.step();
        engine.process_block(&mut vec![0.0f32; BLOCK_SIZE * 2]);
        while let Some((to, len)) = client.try_pop(&mut buf) {
            let Ok(OscPacket::Message(msg)) = clausters::osc::decode_packet(&buf[..len]) else {
                continue;
            };
            if msg.addr != "/bus_stream.reply" {
                continue;
            }
            match to {
                SCRIPT => to_script.push(msg),
                HOST => to_host.push(msg),
                other => panic!("a snapshot addressed to nobody: peer {other}"),
            }
        }
        if to_script.len() >= 2 && to_host.len() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    assert!(
        to_script.len() >= 2,
        "the script kept its stream: {} snapshot(s)",
        to_script.len()
    );
    assert!(
        to_host.len() >= 2,
        "the host kept its stream: {} snapshot(s)",
        to_host.len()
    );
    // And each got its own bus, not the other's.
    for msg in &to_script {
        assert_eq!(msg.args[0], OscType::Int(0), "the script asked for bus 0");
    }
    for msg in &to_host {
        assert_eq!(msg.args[0], OscType::Int(1), "the host asked for bus 1");
    }
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
        assert!(client.push(PEER, packet));
        for _ in 0..500 {
            if let Some((to, len)) = client.try_pop(&mut buf) {
                assert_eq!(to, PEER, "a reply is addressed to the peer that asked");
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

    let status = request(&encode("/server_status", vec![]));
    assert_eq!(status.addr, "/server_status.reply");

    // A synth via the ring must reach the engine and make sound.
    assert!(client.push(
        PEER,
        &encode(
            "/synth_new",
            vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(1),
                OscType::Int(0),
            ],
        )
    ));
    std::thread::sleep(Duration::from_millis(50)); // let the server forward it
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut heard = false;
    for _ in 0..50 {
        engine.process_block(&mut out);
        heard |= out.iter().any(|s| *s != 0.0);
    }
    assert!(heard, "the ring /synth_new must be audible");

    // The clock mirror in the segment is block-accurate.
    let clock = segment.clock().load(Ordering::Acquire);
    assert!(clock >= 50 * BLOCK_SIZE as u64, "clock = {clock}");

    let bad = request(&encode("/zzz", vec![]));
    assert_eq!(bad.addr, "/fail", "errors route back through the ring");

    assert!(client.push(PEER, &encode("/server_quit", vec![])));
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

    // And the handle's /bus_get path sees external writes too (same memory).
    assert_eq!(handle.control_buses().get(7), 0.625);
}

/// The sync scientific call of the embed C ABI, exercised as a plain Rust
/// function (`cargo test --features embed --test ipc`).
#[cfg(feature = "embed")]
#[test]
fn embed_render_returns_flat_samples() {
    use clausters::embed::{clausters_free_samples, clausters_render};

    // A minimal score: /synth_new default at t = 0, /node_free at t = 0.1.
    let s_new = encode(
        "/synth_new",
        vec![
            OscType::String("default".into()),
            OscType::Int(1000),
            OscType::Int(1),
            OscType::Int(0),
        ],
    );
    let n_free = encode("/node_free", vec![OscType::Int(1000)]);
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
    let mut seed = 0u64;
    let mut err = vec![0u8; 256];
    // NULL seed: a fresh take, and the seed it drew comes back in `seed`.
    let ptr = unsafe {
        clausters_render(
            score.as_ptr(),
            score.len(),
            48_000.0,
            1,
            0,
            std::ptr::null(),
            &mut frames,
            &mut events,
            &mut seed,
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
    assert_ne!(seed, 0, "the seed the render used is reported back");
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
            &SEED_STRIDE,
            &mut frames,
            &mut events,
            &mut seed,
            err.as_mut_ptr(),
            err.len(),
        )
    };
    assert!(ptr.is_null());
    assert_ne!(err[0], 0, "error message must be written");
}

/// **The material a peer maps, and the three things that make it safe.**
///
/// A pool buffer's samples live in a region beside the segment (S19), so a
/// local peer draws and edits them with no message at all. What this pins is
/// not the speed but the rules: a peer finds the buffer by *number*, writes
/// cells the server reads back, and a freed buffer's mapping stays valid while
/// telling the peer it is history.
#[test]
fn a_peer_maps_a_buffer_by_number_and_writes_what_the_server_reads() {
    use clausters::dsp::buffer::Buffer;
    use clausters::dsp::region::Region;
    use std::sync::Arc;

    let path = std::env::temp_dir().join(format!(
        "clausters-region-test-{}-{}",
        std::process::id(),
        line!()
    ));
    let segment = Segment::create(&path).expect("segment");

    // The server's side: publish the row, create the region, install a buffer
    // whose cells *are* that region.
    let generation = segment.publish_buffer(3, 4, 2, 48_000.0).expect("a row");
    assert!(!generation.is_multiple_of(2), "a live slot is odd");
    let region_path = Region::path_for(&path, 3, generation);
    let region = Arc::new(Region::create(&region_path, 8).expect("region"));
    let served = Buffer::shared(Arc::clone(&region), 2, 4, 48_000.0);
    served.set_at(5, 0.25);

    // The peer's side: the directory says what it is, and the map is the same
    // memory rather than a copy of it.
    let (mapped_generation, mapped) = segment.map_buffer(&path, 3).expect("a peer maps it");
    assert_eq!(mapped_generation, generation);
    assert_eq!(
        (mapped.frames(), mapped.channels(), mapped.sample_rate()),
        (4, 2, 48_000.0)
    );
    assert_eq!(mapped.at(5), 0.25, "the server's sample, with nothing sent");
    mapped.set_at(1, -0.5);
    assert_eq!(served.at(1), -0.5, "and the peer's, read straight back");

    // Freed: the row goes even, the name is gone, and the mapping the peer is
    // holding stays valid -- which is what lets a buffer be freed while
    // somebody is still drawing it.
    segment.retire_buffer(3);
    Region::unlink(&region_path);
    assert!(segment.buffer_info(3).is_none(), "the slot reads empty");
    assert!(segment.map_buffer(&path, 3).is_none(), "and maps no more");
    assert_eq!(mapped.at(5), 0.25, "what was mapped is still readable");

    // And the next allocation takes a new generation, so no stale mapping can
    // ever be aliased onto new material.
    let next = segment.publish_buffer(3, 4, 2, 48_000.0).expect("a row");
    assert!(next > generation && !next.is_multiple_of(2));
    assert_ne!(Region::path_for(&path, 3, next), region_path);

    Region::unlink(&Region::path_for(&path, 3, next));
    let _ = std::fs::remove_file(&path);
}

/// **End to end, over a real server**: a `/buffer_alloc` that arrives through
/// the ring reaches the pool, and a peer with nothing but the segment's path
/// maps the material by number.
///
/// This is the property S19 exists for — the editor's samples stop being
/// messages — and it is asserted the only way that means anything: the peer
/// writes a sample, and the server's own buffer reads it back.
#[test]
fn a_buffer_the_server_allocated_is_mapped_by_a_peer() {
    let path = std::env::temp_dir().join(format!(
        "clausters-shm-alloc-{}-{}",
        std::process::id(),
        line!()
    ));
    let segment = Segment::create(&path).expect("segment");
    let (mut engine, handle) = engine_pair_full(
        SR,
        2,
        0,
        Some(Arc::clone(&segment)),
        128,
        1024,
        clausters::dsp::Limits::default(),
    );
    let mut server = OscServer::headless(
        ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        },
        handle,
        0.0,
    );
    server
        .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
        .unwrap();
    server.share_buffers_at(path.clone());

    let client = IpcPeer::new(Arc::clone(&segment), Role::Client);
    assert!(client.push(
        0,
        &encode(
            "/buffer_alloc",
            vec![OscType::Int(2), OscType::Int(64), OscType::Int(1)],
        )
    ));

    // The allocation is an NRT job, so the loop has to come round again.
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..400 {
        server.step();
        engine.process_block(&mut out);
        if segment.buffer_info(2).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let (_, mapped) = segment
        .map_buffer(&path, 2)
        .expect("the peer maps what the server allocated");
    assert_eq!((mapped.frames(), mapped.channels()), (64, 1));
    mapped.set_at(7, 0.75);
    assert_eq!(
        mapped.at(7),
        0.75,
        "the peer writes the material, with nothing sent"
    );

    let _ = std::fs::remove_file(&path);
}

/// **The arrangement's own test**: a second server attaches to a segment that
/// already holds material, plays the owner's very cells, and takes none of it
/// with it when it goes.
///
/// This is what makes "separate processes" a claim rather than a diagram. Both
/// servers are built here in one process — what is under test is the
/// *ownership* rules, not the process boundary, and running them apart is the
/// example's job (`examples/editor_processes.sh`).
#[test]
fn a_second_server_attaches_to_the_material_and_owns_none_of_it() {
    let path = std::env::temp_dir().join(format!(
        "clausters-shm-attach-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    // The owner: it creates the segment, claims the command plane, and puts
    // every buffer it installs into a region beside it.
    let (owner_segment, created) = Segment::open_or_create_full(&path, 1024, 2, 1024).unwrap();
    assert!(created, "nothing was there, so it was created");
    assert!(owner_segment.claim_control(), "the first server in owns it");
    let owner = shared_server(&owner_segment, Some(path.clone()));
    let (mut owner_server, mut owner_engine) = owner;

    let client = IpcPeer::new(Arc::clone(&owner_segment), Role::Client);
    assert!(client.push(
        0,
        &encode(
            "/buffer_alloc",
            vec![OscType::Int(3), OscType::Int(32), OscType::Int(1)],
        )
    ));
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..400 {
        owner_server.step();
        owner_engine.process_block(&mut out);
        if owner_segment.buffer_info(3).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(owner_segment.buffer_info(3).is_some(), "the take exists");

    // The player: it attaches to what is there. **It gets no claim**, because
    // the rings are SPSC and draining them from two processes would lose half
    // the commands to whichever popped first.
    let (player_segment, created) = Segment::open_or_create_full(&path, 1024, 2, 1024).unwrap();
    assert!(
        !created,
        "a segment that exists is adopted, never truncated"
    );
    assert!(
        !player_segment.claim_control(),
        "the command plane is taken, and a second server must find that out"
    );
    let (mut player_server, _player_engine) = shared_server(&player_segment, None);
    player_server.attach_segment(Arc::clone(&player_segment));
    let found = player_server.attach_material_at(path.clone());
    assert_eq!(found, 1, "it maps the take that was already there");

    // One material, two servers: the owner's write is what the player reads.
    let (_, owners_view) = owner_segment.map_buffer(&path, 3).unwrap();
    let (_, players_view) = player_segment.map_buffer(&path, 3).unwrap();
    owners_view.set_at(5, -0.5);
    assert_eq!(
        players_view.at(5),
        -0.5,
        "the player plays the very cells the owner edits"
    );

    // And the player owns none of it: freeing a buffer through it retires
    // nothing, because the row and the region are the owner's.
    drop(player_server);
    assert!(
        owner_segment.buffer_info(3).is_some(),
        "the material outlives the player, which is the whole point of separating them"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(clausters::dsp::region::Region::path_for(&path, 3, 1));
}

/// A claim whose owner is gone is stale, not a lock: killing the RT server
/// must leave a segment the next one can serve, or "killable" would mean
/// "restart the machine".
#[test]
fn a_dead_owners_claim_is_taken_over() {
    let path = std::env::temp_dir().join(format!(
        "clausters-shm-claim-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);
    let (segment, _) = Segment::open_or_create_full(&path, 64, 0, 1024).unwrap();
    assert!(segment.claim_control());
    assert_eq!(segment.control_owner(), Some(std::process::id()));
    // A live holder refuses whoever asks second, this process included: one
    // ring pair, one drainer.
    let (twin, _) = Segment::open_or_create_full(&path, 64, 0, 1024).unwrap();
    assert!(!twin.claim_control());
    segment.release_control();
    assert_eq!(segment.control_owner(), None);
    assert!(segment.claim_control(), "a free segment is claimable again");
    let _ = std::fs::remove_file(&path);
}

/// Builds a headless server + engine over `segment`, owning the material when
/// a path is given. The two tests above differ only in that.
fn shared_server(
    segment: &Arc<Segment>,
    own_material_at: Option<std::path::PathBuf>,
) -> (OscServer, clausters::server::engine::Engine) {
    let (engine, handle) = engine_pair_full(
        SR,
        2,
        0,
        Some(Arc::clone(segment)),
        128,
        segment.control_bus_count(),
        clausters::dsp::Limits::default(),
    );
    let mut server = OscServer::headless(
        ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        },
        handle,
        0.0,
    );
    if let Some(path) = own_material_at {
        server
            .attach_ipc(IpcPeer::new(Arc::clone(segment), Role::Server))
            .unwrap();
        server.share_buffers_at(path);
    }
    (server, engine)
}

/// **The editor's arrangement, end to end in one process**: the on-demand
/// session owns the segment and the material, an RT-shaped server attaches to
/// play it, and the session never touches the clocks.
///
/// The three roles are the phase's whole design — the session computes, the
/// player holds the devices, and whoever edits writes the cells directly —
/// so this asserts the two rules that make them safe to run at once.
#[test]
fn a_session_owns_the_material_and_a_player_attaches_to_it() {
    use clausters::server::nrtsession::{NrtSession, SessionConfig};

    let path = std::env::temp_dir().join(format!(
        "clausters-shm-session-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);

    let mut session = NrtSession::open(&SessionConfig {
        sample_rate: SR as f64,
        channels: 1,
        shm: Some(path.clone()),
        ..Default::default()
    })
    .expect("session");
    let segment = Arc::clone(session.segment());
    assert_eq!(
        segment.control_owner(),
        Some(std::process::id()),
        "the session serves the ring it is driven through"
    );

    session
        .send_msg(
            "/buffer_alloc",
            vec![OscType::Int(1), OscType::Int(64), OscType::Int(1)],
        )
        .unwrap();
    for _ in 0..200 {
        session.settle();
        if segment.buffer_info(1).is_some() {
            break;
        }
    }
    let (_, take) = segment
        .map_buffer(&path, 1)
        .expect("a session with a path publishes its material");
    take.set_at(9, 0.25);

    // The player: a server that attached, mapping what the session owns.
    let (player_segment, created) = Segment::open_or_create_full(&path, 1024, 2, 1024).unwrap();
    assert!(!created);
    assert!(!player_segment.claim_control());
    let (mut player, _engine) = shared_server(&player_segment, None);
    player.attach_segment(Arc::clone(&player_segment));
    assert_eq!(player.attach_material_at(path.clone()), 1);
    let (_, played) = player_segment.map_buffer(&path, 1).unwrap();
    assert_eq!(
        played.at(9),
        0.25,
        "the player plays the very cells the session owns"
    );

    // **The clocks belong to the device.** Running the session is an
    // operation, not time passing: a fade must not jog a playhead the player
    // is publishing.
    let before = segment.clock().load(Ordering::Relaxed);
    session.run(4096, |_| Ok(())).unwrap();
    assert_eq!(
        segment.clock().load(Ordering::Relaxed),
        before,
        "a session with no device publishes no time"
    );

    drop(session);
    assert_eq!(
        player_segment.control_owner(),
        None,
        "a session gives the command plane back on the way out"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(clausters::dsp::region::Region::path_for(&path, 1, 1));
}
