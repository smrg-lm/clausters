//! M5 tests: the buffer pool, the NRT thread (alloc / WAV read / write /
//! zero / free, in submission order) and the `PlayBuf`/`BufRd` UGens, plus
//! the `/b_*` OSC round trip with a manually ticked engine.

#![cfg(feature = "synth")]

use std::sync::Arc;
use std::time::Duration;

use clausters::dsp::buffer::Buffer;
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::server::nrt::{NrtAction, NrtJob, NrtRequest, NrtThread};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use serde_json::json;

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
const NRT_DEADLINE: Duration = Duration::from_secs(10);

/// A throwaway path in the test temp dir.
fn tmp_wav(name: &str) -> String {
    tmp_path(name, "wav")
}

/// Same, with an arbitrary extension (used to force the non-WAV read path).
fn tmp_path(name: &str, ext: &str) -> String {
    let path = std::env::temp_dir().join(format!("clausters_{name}_{}.{ext}", std::process::id()));
    path.to_str().unwrap().to_string()
}

fn run_nrt(job: NrtJob) -> Result<NrtAction, String> {
    let nrt = NrtThread::spawn();
    nrt.submit(NrtRequest {
        cmd: "/b_test",
        index: 0,
        client: clausters::osc::ClientId::Udp("127.0.0.1:1".parse().unwrap()),
        job,
    })
    .ok()
    .unwrap();
    nrt.recv_result_timeout(NRT_DEADLINE)
        .expect("NRT job must finish")
        .outcome
}

fn installed(action: Result<NrtAction, String>) -> Arc<Buffer> {
    match action.expect("job must succeed") {
        NrtAction::Install(buffer) => buffer,
        _ => panic!("expected an installed buffer"),
    }
}

/// `PlayBuf(bufnum, chan, rate, loop)` to the given output bus.
fn playbuf_spec(bufnum: f32, chan: f32, rate: f32, looping: f32, out_bus: f32) -> UGenSynth {
    spec_synth(json!({
        "name": "player",
        "ugens": [
            {"kind": "PlayBuf", "inputs": [
                {"const": bufnum}, {"const": chan}, {"const": rate}, {"const": looping}
            ]},
            {"kind": "Out", "inputs": [{"const": out_bus}, {"ugen": 0}]}
        ]
    }))
}

fn spec_synth(spec: serde_json::Value) -> UGenSynth {
    let spec: SynthDefSpec = serde_json::from_value(spec).unwrap();
    UGenSynth::new(Arc::new(clausters::synthdef::compile(spec).unwrap()))
}

fn add_synth(id: i32, synth: UGenSynth) -> Cmd {
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth: Box::new(synth),
        usage: Default::default(),
    }
}

/// Renders `blocks` blocks and returns the chosen interleaved channel.
fn render_channel(engine: &mut Engine, blocks: usize, channel: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        buf.extend(out.iter().skip(channel).step_by(CHANNELS).copied());
    }
    buf
}

/// A mono buffer whose samples are a recognizable ramp `i / 1000`.
fn ramp_buffer(frames: usize) -> Arc<Buffer> {
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / 1000.0).collect();
    Arc::new(Buffer::new(data, 1, frames, SR as f64))
}

// ---- engine + UGens ----

#[test]
fn playbuf_plays_a_buffer_once_then_goes_silent() {
    let frames = 100;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(frames)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(1000, playbuf_spec(0.0, 0.0, 1.0, 0.0, 0.0)))
        .ok()
        .unwrap();

    let left = render_channel(&mut engine, 4, 0); // 256 samples > 100 frames
    for (i, s) in left.iter().enumerate() {
        let expected = if i < frames { i as f32 / 1000.0 } else { 0.0 };
        assert_eq!(*s, expected, "sample {i}: rate-1 playback must be exact");
    }
}

#[test]
fn playbuf_loops_exactly() {
    let frames = 24;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 3,
            buffer: Some(ramp_buffer(frames)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(1000, playbuf_spec(3.0, 0.0, 1.0, 1.0, 0.0)))
        .ok()
        .unwrap();

    let left = render_channel(&mut engine, 8, 0);
    for (i, s) in left.iter().enumerate() {
        assert_eq!(*s, (i % frames) as f32 / 1000.0, "sample {i}");
    }
}

#[test]
fn playbuf_reads_each_channel_of_an_interleaved_buffer() {
    // Stereo buffer: left = 0.25, right = -0.5 everywhere.
    let frames = 200;
    let data: Vec<f32> = (0..frames).flat_map(|_| [0.25, -0.5]).collect();
    let buffer = Arc::new(Buffer::new(data, 2, frames, SR as f64));

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(buffer),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(1000, playbuf_spec(0.0, 0.0, 1.0, 0.0, 0.0)))
        .ok()
        .unwrap();
    handle
        .send(add_synth(1001, playbuf_spec(0.0, 1.0, 1.0, 0.0, 1.0)))
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine.process_block(&mut out);
    assert!(out.chunks(2).all(|f| f == [0.25, -0.5]), "{:?}", &out[..4]);
}

#[test]
fn playbuf_with_no_buffer_is_silent() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_synth(1000, playbuf_spec(7.0, 0.0, 1.0, 1.0, 0.0)))
        .ok()
        .unwrap();
    let left = render_channel(&mut engine, 4, 0);
    assert!(left.iter().all(|s| *s == 0.0));
}

#[test]
fn bufrd_interpolates_wraps_and_clamps() {
    let frames = 8;
    let bufrd = |phase: f32, looping: f32, out_bus: f32| {
        spec_synth(json!({
            "name": "rd",
            "ugens": [
                {"kind": "BufRd", "inputs": [
                    {"const": 0.0}, {"const": 0.0}, {"const": phase}, {"const": looping}
                ]},
                {"kind": "Out", "inputs": [{"const": out_bus}, {"ugen": 0}]}
            ]
        }))
    };
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(frames)), // data[i] = i/1000
        })
        .ok()
        .unwrap();
    // Linear interpolation between frames 5 and 6.
    handle
        .send(add_synth(1000, bufrd(5.25, 0.0, 0.0)))
        .ok()
        .unwrap();
    // Out-of-range phase: wraps when looping (9.0 → 1.0)...
    handle
        .send(add_synth(1001, bufrd(9.0, 1.0, 1.0)))
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine.process_block(&mut out);
    assert_eq!(out[0], 5.25 / 1000.0);
    assert_eq!(out[1], 1.0 / 1000.0);

    // ...and clamps to the last frame when not looping.
    let (mut engine2, mut handle2) = engine_pair(SR, CHANNELS);
    handle2
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(frames)),
        })
        .ok()
        .unwrap();
    handle2
        .send(add_synth(1000, bufrd(100.0, 0.0, 0.0)))
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine2.process_block(&mut out);
    assert_eq!(out[0], 7.0 / 1000.0);
}

#[test]
fn buf_info_ugens_report_shape_and_rate_scale() {
    // A 24 kHz, 100-frame mono buffer played on a 48 kHz server: BufRateScale
    // must report 0.5 (file_sr / server_sr) so PlayBuf can correct the pitch.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let data: Vec<f32> = vec![0.0; 100];
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(Buffer::new(data, 1, 100, 24_000.0))),
        })
        .ok()
        .unwrap();

    let info = |kind: &str, out_bus: f32| {
        spec_synth(json!({
            "name": "info",
            "ugens": [
                {"kind": kind, "inputs": [{"const": 0.0}]},
                {"kind": "Out", "inputs": [{"const": out_bus}, {"ugen": 0}]}
            ]
        }))
    };
    for (i, kind) in ["BufRateScale", "BufSampleRate", "BufFrames", "BufDur"]
        .iter()
        .enumerate()
    {
        // Out bus 0 -> left channel, 1 -> right; reuse two channels twice.
        handle
            .send(add_synth(1000 + i as i32, info(kind, (i % 2) as f32)))
            .ok()
            .unwrap();
        if i % 2 == 1 {
            let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
            engine.process_block(&mut out);
            // i-1 went to bus 0, i to bus 1.
            let prev = out[0];
            let cur = out[1];
            match i {
                1 => {
                    assert_eq!(prev, 0.5, "BufRateScale = 24000/48000");
                    assert_eq!(cur, 24_000.0, "BufSampleRate");
                }
                3 => {
                    assert_eq!(prev, 100.0, "BufFrames");
                    assert_eq!(cur, 100.0 / 24_000.0, "BufDur = frames/file_sr");
                }
                _ => unreachable!(),
            }
            // Clear the nodes before the next pair so the buses are clean.
            handle
                .send(Cmd::FreeNode {
                    id: 1000 + i as i32 - 1,
                })
                .ok()
                .unwrap();
            handle
                .send(Cmd::FreeNode {
                    id: 1000 + i as i32,
                })
                .ok()
                .unwrap();
            render_channel(&mut engine, 1, 0);
        }
    }

    // BufChannels on a stereo buffer.
    handle
        .send(Cmd::SetBuffer {
            index: 1,
            buffer: Some(Arc::new(Buffer::new(vec![0.0; 8], 2, 4, 48_000.0))),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(
            2000,
            spec_synth(json!({
                "name": "ch",
                "ugens": [
                    {"kind": "BufChannels", "inputs": [{"const": 1.0}]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
                ]
            })),
        ))
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine.process_block(&mut out);
    assert_eq!(out[0], 2.0, "BufChannels");
}

#[test]
fn replaced_buffer_leaves_through_the_garbage_fifo() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(10)),
        })
        .ok()
        .unwrap();
    render_channel(&mut engine, 1, 0);
    assert_eq!(
        handle.collect_garbage(),
        0,
        "first install replaces nothing"
    );

    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(20)),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: None,
        })
        .ok()
        .unwrap();
    render_channel(&mut engine, 1, 0);
    assert_eq!(handle.collect_garbage(), 2, "replace + free ship both out");
}

// ---- NRT thread: WAV round trips ----

#[test]
fn nrt_allocates_zeroed_buffers() {
    let buffer = installed(run_nrt(NrtJob::Alloc {
        frames: 64,
        channels: 2,
        sample_rate: 44_100.0,
    }));
    assert_eq!((buffer.frames(), buffer.channels()), (64, 2));
    assert_eq!(buffer.sample_rate(), 44_100.0);
    assert!(buffer.data().iter().all(|s| *s == 0.0));
}

#[test]
fn wav_write_then_alloc_read_round_trips_float_exactly() {
    let path = tmp_wav("roundtrip");
    let frames = 300;
    let data: Vec<f32> = (0..frames * 2).map(|i| (i as f32).sin() * 0.5).collect();
    let original = Arc::new(Buffer::new(data, 2, frames, 22_050.0));

    let outcome = run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::clone(&original),
    });
    assert!(matches!(outcome, Ok(NrtAction::None)), "{outcome:?}");

    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: 0,
    }));
    assert_eq!(read.frames(), frames);
    assert_eq!(read.channels(), 2);
    assert_eq!(read.sample_rate(), 22_050.0);
    assert_eq!(read.data(), original.data(), "float WAV must be lossless");
    std::fs::remove_file(&path).ok();
}

#[test]
fn alloc_read_slices_the_file() {
    let path = tmp_wav("slice");
    let data: Vec<f32> = (0..100).map(|i| i as f32 / 1000.0).collect();
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::new(Buffer::new(data, 1, 100, 48_000.0)),
    })
    .unwrap();

    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 10,
        num_frames: 5,
    }));
    assert_eq!(read.frames(), 5);
    let expected: Vec<f32> = (10..15).map(|i| i as f32 / 1000.0).collect();
    assert_eq!(read.data(), &expected[..]);
    std::fs::remove_file(&path).ok();
}

#[test]
fn read_overlays_a_file_keeping_the_buffer_shape() {
    let path = tmp_wav("overlay");
    let data: Vec<f32> = vec![0.5; 10];
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::new(Buffer::new(data, 1, 10, 48_000.0)),
    })
    .unwrap();

    let read = installed(run_nrt(NrtJob::Read {
        path: path.clone(),
        file_start: 0,
        num_frames: -1,
        buf_start: 5,
        current: Arc::new(Buffer::zeroed(20, 1, 48_000.0)),
    }));
    assert_eq!(read.frames(), 20, "/b_read keeps the buffer's shape");
    for (i, s) in read.data().iter().enumerate() {
        let expected = if (5..15).contains(&i) { 0.5 } else { 0.0 };
        assert_eq!(*s, expected, "frame {i}");
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn read_rejects_channel_mismatch_and_missing_files() {
    let path = tmp_wav("mismatch");
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::new(Buffer::zeroed(10, 2, 48_000.0)),
    })
    .unwrap();
    let err = run_nrt(NrtJob::Read {
        path: path.clone(),
        file_start: 0,
        num_frames: -1,
        buf_start: 0,
        current: Arc::new(Buffer::zeroed(10, 1, 48_000.0)),
    })
    .unwrap_err();
    assert!(err.contains("channel count mismatch"), "{err}");
    std::fs::remove_file(&path).ok();

    let err = run_nrt(NrtJob::AllocRead {
        path: "/nonexistent/clausters.wav".into(),
        file_start: 0,
        num_frames: 0,
    })
    .unwrap_err();
    assert!(err.contains("/nonexistent/clausters.wav"), "{err}");
}

#[test]
fn int16_write_quantizes_to_the_expected_grid() {
    let path = tmp_wav("int16");
    let original = Arc::new(Buffer::new(vec![0.5, -1.0, 1.0, 0.0], 1, 4, 48_000.0));
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "int16".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: original,
    })
    .unwrap();
    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: 0,
    }));
    // Write scales by 32767, read by 1/32768.
    let expected = [
        (0.5f32 * 32767.0).round() / 32768.0,
        -32767.0 / 32768.0,
        32767.0 / 32768.0,
        0.0,
    ];
    assert_eq!(read.data(), &expected[..]);
    std::fs::remove_file(&path).ok();
}

#[test]
fn diskout_records_then_diskin_streams_it_back() {
    // End-to-end streaming I/O: play a buffer into DiskOut (a float WAV on
    // disk), then stream that file back through DiskIn and check the samples.
    // The signal `(i+1)/1000` starts non-zero, so DiskIn output is
    // distinguishable from underrun silence.
    let path = tmp_path("diskio", "wav");
    let frames = 300;
    let signal: Vec<f32> = (0..frames).map(|i| (i + 1) as f32 / 1000.0).collect();

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(Buffer::new(signal.clone(), 1, frames, SR as f64))),
        })
        .ok()
        .unwrap();

    // PlayBuf (rate 1, no loop) -> DiskOut(float). No Out: the synth is silent
    // on the buses, it only records.
    let recorder = spec_synth(json!({
        "name": "rec",
        "ugens": [
            {"kind": "PlayBuf", "inputs": [
                {"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0}
            ]},
            {"kind": "DiskOut", "inputs": [{"ugen": 0}], "path": path, "format": "float"}
        ]
    }));
    handle.send(add_synth(1000, recorder)).ok().unwrap();
    // Render enough blocks to push the whole buffer through the ring.
    let blocks = frames.div_ceil(BLOCK_SIZE) + 2;
    render_channel(&mut engine, blocks, 0);

    // Free the recorder; dropping its synth (on garbage collection) joins the
    // writer thread, which drains the ring and finalizes the WAV header.
    handle.send(Cmd::FreeNode { id: 1000 }).ok().unwrap();
    let deadline = std::time::Instant::now() + NRT_DEADLINE;
    while handle.collect_garbage() == 0 {
        render_channel(&mut engine, 1, 0);
        assert!(std::time::Instant::now() < deadline, "recorder never freed");
    }

    // Sanity: the file holds the signal (read it straight back).
    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: frames as i64,
    }));
    assert_eq!(read.frames(), frames);
    assert_eq!(read.data(), &signal[..], "DiskOut must write the signal");

    // Now stream the same file back through DiskIn -> Out(bus 0) and look for
    // the signal in the left channel. The disk thread fills the ring
    // asynchronously, so poll (with a tiny yield) until data arrives.
    let player = spec_synth(json!({
        "name": "play",
        "ugens": [
            {"kind": "DiskIn", "inputs": [{"const": 0.0}], "path": path},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }));
    handle.send(add_synth(2000, player)).ok().unwrap();

    let mut collected: Vec<f32> = Vec::new();
    let deadline = std::time::Instant::now() + NRT_DEADLINE;
    let found = loop {
        collected.extend(render_channel(&mut engine, 1, 0));
        // The stream emits the file in order; find where signal[0] (0.001) is
        // followed by signal[1], signal[2], ... to confirm ordered streaming.
        if let Some(p) = collected
            .windows(8)
            .position(|w| (0..8).all(|k| (w[k] - signal[k]).abs() < 1e-9))
        {
            break Some(p);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let start = found.expect("DiskIn never produced the streamed signal");
    // Verify a longer ordered run from that point.
    for (k, expected) in signal.iter().enumerate().take(64) {
        assert_eq!(
            collected[start + k],
            *expected,
            "DiskIn frame {k} must stream in order"
        );
    }

    handle.send(Cmd::FreeNode { id: 2000 }).ok().unwrap();
    render_channel(&mut engine, 2, 0);
    while handle.collect_garbage() > 0 {}
    std::fs::remove_file(&path).ok();
}

#[test]
fn non_wav_extensions_decode_through_symphonia() {
    // Lossless float PCM written to a `.dat` file: `read_audio` routes it
    // through symphonia (not hound), which detects the container by content,
    // not the extension. Exercises the decode + interleave + slice path that
    // every compressed format (FLAC, OGG, MP3, ...) shares.
    let path = tmp_path("symphonia", "dat");
    let frames = 200;
    let data: Vec<f32> = (0..frames * 2)
        .map(|i| (i as f32 * 0.013).sin() * 0.5)
        .collect();
    let original = Arc::new(Buffer::new(data, 2, frames, 44_100.0));
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::clone(&original),
    })
    .unwrap();

    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: 0,
    }));
    assert_eq!((read.frames(), read.channels()), (frames, 2));
    assert_eq!(read.sample_rate(), 44_100.0);
    assert_eq!(
        read.data(),
        original.data(),
        "float PCM via symphonia must be lossless"
    );

    // The slice window (file_start / num_frames) applies on this path too.
    let sliced = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 10,
        num_frames: 5,
    }));
    assert_eq!(sliced.frames(), 5);
    assert_eq!(sliced.data(), &original.data()[10 * 2..15 * 2]);
    std::fs::remove_file(&path).ok();
}

// ---- OSC round trip: /b_* against a live server, engine ticked manually ----

mod osc {
    use super::*;
    use std::net::UdpSocket;

    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

    #[test]
    fn buffer_lifecycle_over_osc() {
        let (mut engine, engine_handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        let addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client.set_read_timeout(Some(NRT_DEADLINE)).unwrap();

        let send = |addr_str: &str, args: Vec<OscType>| {
            let packet = OscPacket::Message(OscMessage {
                addr: addr_str.into(),
                args,
            });
            client
                .send_to(&encoder::encode(&packet).unwrap(), addr)
                .unwrap();
        };
        let recv_until = |want: &str| -> OscMessage {
            let mut buf = [0u8; 65536];
            for _ in 0..100 {
                let (len, _) = client.recv_from(&mut buf).expect("reply timed out");
                if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len]).unwrap()
                    && msg.addr == want
                {
                    return msg;
                }
            }
            panic!("never received {want}");
        };

        // A 50-frame mono WAV with a known ramp.
        let path = tmp_wav("osc");
        let data: Vec<f32> = (0..50).map(|i| i as f32 / 1000.0).collect();
        run_nrt(NrtJob::Write {
            path: path.clone(),
            sample_format: "float".into(),
            buf_start: 0,
            num_frames: -1,
            buffer: Arc::new(Buffer::new(data.clone(), 1, 50, SR as f64)),
        })
        .unwrap();

        // /b_allocRead → /done /b_allocRead 0, then /b_query → /b_info.
        send(
            "/b_allocRead",
            vec![OscType::Int(0), OscType::String(path.clone())],
        );
        let done = recv_until("/done");
        assert_eq!(done.args[0], OscType::String("/b_allocRead".into()));
        assert_eq!(done.args[1], OscType::Int(0));

        send("/b_query", vec![OscType::Int(0)]);
        let infos = recv_until("/b_info");
        assert_eq!(
            infos.args,
            vec![
                OscType::Int(0),
                OscType::Int(50),
                OscType::Int(1),
                OscType::Float(SR),
            ]
        );

        // Play it through a /d_recv'd PlayBuf def, looping; the engine is
        // ticked from here, so the output is deterministic.
        let def = json!({
            "name": "player",
            "ugens": [
                {"kind": "PlayBuf", "inputs": [
                    {"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 1.0}
                ]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
            ]
        })
        .to_string();
        send("/d_recv", vec![OscType::Blob(def.into_bytes())]);
        recv_until("/done");
        send(
            "/s_new",
            vec![
                OscType::String("player".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
            ],
        );
        // The /s_new and the buffer install race our ticking: poll until the
        // loop comes through.
        let mut ok = false;
        for _ in 0..100 {
            let left = render_channel(&mut engine, 2, 0);
            ok = left.iter().enumerate().all(|(i, s)| *s == data[i % 50]);
            if ok && left[1] != 0.0 {
                break;
            }
        }
        assert!(ok, "looped playback must reproduce the file exactly");

        // /b_zero keeps the shape but silences the playback.
        send("/b_zero", vec![OscType::Int(0)]);
        let done = recv_until("/done");
        assert_eq!(done.args[0], OscType::String("/b_zero".into()));
        let mut silent = false;
        for _ in 0..100 {
            silent = render_channel(&mut engine, 2, 0).iter().all(|s| *s == 0.0);
            if silent {
                break;
            }
        }
        assert!(silent, "zeroed buffer must play silence");
        send("/b_query", vec![OscType::Int(0)]);
        let infos = recv_until("/b_info");
        assert_eq!(infos.args[1], OscType::Int(50), "shape survives /b_zero");

        // /b_free empties the slot: /b_query reports zeros.
        send("/n_free", vec![OscType::Int(1000)]);
        send("/b_free", vec![OscType::Int(0)]);
        let done = recv_until("/done");
        assert_eq!(done.args[0], OscType::String("/b_free".into()));
        send("/b_query", vec![OscType::Int(0)]);
        let infos = recv_until("/b_info");
        assert_eq!(infos.args[1], OscType::Int(0));

        // Errors come back as /fail: unallocated read, bad index.
        send(
            "/b_read",
            vec![OscType::Int(0), OscType::String(path.clone())],
        );
        let fail = recv_until("/fail");
        assert_eq!(fail.args[0], OscType::String("/b_read".into()));
        send("/b_alloc", vec![OscType::Int(-1), OscType::Int(10)]);
        recv_until("/fail");

        // Keep ticking so freed nodes/buffers drain, then shut down.
        render_channel(&mut engine, 2, 0);
        send("/quit", vec![]);
        recv_until("/done");
        server_thread.join().unwrap().unwrap();
        std::fs::remove_file(&path).ok();
    }
}
